//! The `candidate-check` gate. Read-only: it runs after `apt-get update` and
//! before `apt-get install`. It verifies that every planned package has an
//! installation candidate from an allowlisted origin, and that the simulated
//! install (which includes dependencies and reflects apt pinning) pulls
//! only packages from allowlisted origins and removes nothing.
//!
//! `apt-get -s` prints release names, not hosts, so the origin of each
//! simulated package is taken from `apt-cache policy`.

use crate::plan::{INSTALL_FLAGS, STEP_CANDIDATE_CHECK};
use crate::sources;
use ossiframe_core::{CancelToken, CommandSpec, ExecError, Executor, ExitStatus, Plan, StepMode};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

const APT_CACHE: &str = "/usr/bin/apt-cache";
const APT_GET: &str = "/usr/bin/apt-get";
const TIMEOUT: Duration = Duration::from_secs(60);
const MAX_INSTALL_SET: usize = 500;
const MAX_SHOWN: usize = 5;

/// `apt-cache policy` data for one package.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PolicyEntry {
    pub candidate: Option<String>,
    /// URIs of the sources that provide the candidate version.
    pub candidate_origins: Vec<String>,
}

/// Parses `apt-cache policy <pkgs>`. A package without an entry is absent
/// from the map.
pub fn parse_policy(stdout: &str) -> BTreeMap<String, PolicyEntry> {
    let mut out: BTreeMap<String, PolicyEntry> = BTreeMap::new();
    let mut current: Option<String> = None;
    let mut in_table = false;
    let mut in_candidate = false;
    for line in stdout.lines() {
        if !line.starts_with(' ') {
            current = line.strip_suffix(':').map(str::to_owned);
            in_table = false;
            in_candidate = false;
            if let Some(name) = &current {
                out.entry(name.clone()).or_default();
            }
            continue;
        }
        let Some(name) = &current else { continue };
        let entry = out.entry(name.clone()).or_default();
        let t = line.trim();
        if let Some(c) = t.strip_prefix("Candidate:") {
            let c = c.trim();
            entry.candidate = (c != "(none)" && !c.is_empty()).then(|| c.to_owned());
        } else if t.starts_with("Version table:") {
            in_table = true;
        } else if in_table {
            let tokens: Vec<&str> = t.split_whitespace().collect();
            let tokens: &[&str] = if tokens.first() == Some(&"***") {
                &tokens[1..]
            } else {
                &tokens
            };
            match tokens {
                // A source line: "<priority> <uri-or-path> ...".
                [prio, uri, ..]
                    if prio.bytes().all(|b| b.is_ascii_digit()) && uri.contains('/') =>
                {
                    if in_candidate {
                        entry.candidate_origins.push((*uri).to_owned());
                    }
                }
                // A version line: "<version> <priority>".
                [version, prio] if prio.bytes().all(|b| b.is_ascii_digit()) => {
                    in_candidate = entry.candidate.as_deref() == Some(*version);
                }
                _ => {}
            }
        }
    }
    out
}

/// Why a package is not acceptable, or `None`.
fn origin_problem(name: &str, entry: Option<&PolicyEntry>) -> Option<String> {
    let Some(entry) = entry else {
        return Some(format!("{name}: no entry in the package lists"));
    };
    let Some(cand) = &entry.candidate else {
        return Some(format!("{name}: no installation candidate"));
    };
    let mut hosts = 0;
    for uri in &entry.candidate_origins {
        if uri == "/var/lib/dpkg/status" {
            continue;
        }
        match sources::uri_host(uri) {
            Ok(h) if sources::host_allowed(&h) => hosts += 1,
            Ok(h) => {
                return Some(format!(
                    "{name} {}: origin {} is not on the allowlist",
                    sources::clean(cand),
                    sources::clean(&h)
                ));
            }
            Err(e) => {
                return Some(format!(
                    "{name} {}: origin {} ({e})",
                    sources::clean(cand),
                    sources::clean(uri)
                ));
            }
        }
    }
    (hosts == 0).then(|| format!("{name} {}: no known origin", sources::clean(cand)))
}

/// Package names that `apt-get -s install` would install. `Err` when it
/// would remove or replace a package, or the output is not understood.
pub fn parse_simulation(stdout: &str) -> Result<BTreeSet<String>, String> {
    let mut names = BTreeSet::new();
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("Inst ") {
            let name = rest.split_whitespace().next().unwrap_or("");
            if !valid_name(name) {
                return Err(format!(
                    "unrecognized simulation line: {}",
                    sources::clean(line)
                ));
            }
            names.insert(name.to_owned());
        } else if line.starts_with("Remv ") || line.starts_with("Purg ") {
            return Err(format!(
                "the install would remove a package ({}); Ossiframe does not remove packages",
                sources::clean(line)
            ));
        }
    }
    Ok(names)
}

/// A Debian package name. Also rejects a leading `-`, so a name can never be
/// read as an option.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(['-', '+', '.'])
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"+-.".contains(&b))
}

fn fix_hint(problems: &[String]) -> String {
    let shown: Vec<&str> = problems
        .iter()
        .take(MAX_SHOWN)
        .map(String::as_str)
        .collect();
    let more = problems.len().saturating_sub(MAX_SHOWN);
    let mut s = shown.join("; ");
    if more > 0 {
        s.push_str(&format!("; and {more} more"));
    }
    s.push_str(
        ". Nothing was installed. Fix the package sources on the server, then make a new plan.",
    );
    s
}

async fn run_text(
    exec: &dyn Executor,
    cancel: &CancelToken,
    program: &str,
    args: &[&str],
) -> Result<String, String> {
    let spec = CommandSpec::new(program, args, TIMEOUT).map_err(|e| e.to_string())?;
    let r = exec.run(&spec, cancel).await.map_err(|e| match e {
        ExecError::Cancelled => "cancelled".to_owned(),
        ExecError::Timeout => "the package check timed out".to_owned(),
        ExecError::ConnectionLost => "the connection was lost".to_owned(),
        other => sources::clean(&other.to_string()),
    })?;
    if r.truncated {
        return Err("the package check output was cut at the size limit".to_owned());
    }
    match r.exit {
        ExitStatus::Code(0) => Ok(r.stdout),
        other => Err(format!(
            "{program} failed ({other:?}): {}",
            sources::clean(r.stderr.lines().next().unwrap_or(""))
        )),
    }
}

async fn policy_for(
    exec: &dyn Executor,
    cancel: &CancelToken,
    names: &[&str],
) -> Result<BTreeMap<String, PolicyEntry>, String> {
    let mut args: Vec<&str> = vec!["policy"];
    args.extend(names);
    run_text(exec, cancel, APT_CACHE, &args)
        .await
        .map(|s| parse_policy(&s))
}

/// Runs the `candidate-check` step. `Ok` is the exact install set.
pub async fn run(
    exec: &dyn Executor,
    cancel: &CancelToken,
    plan: &Plan,
    step_id: &str,
) -> Result<String, String> {
    let is_gate = plan
        .steps
        .iter()
        .any(|s| s.id == step_id && s.mode == StepMode::Gate);
    if step_id != STEP_CANDIDATE_CHECK || !is_gate {
        return Err(format!("unknown gate: {}", sources::clean(step_id)));
    }
    let requested: Vec<&str> = plan.packages.iter().map(|p| p.name.as_str()).collect();
    if requested.is_empty() {
        return Ok("No packages to install.".to_owned());
    }
    if let Some(bad) = requested.iter().find(|n| !valid_name(n)) {
        return Err(format!(
            "invalid package name in the plan: {}",
            sources::clean(bad)
        ));
    }

    // 1. Every planned package has a candidate from an allowed origin.
    let policy = policy_for(exec, cancel, &requested).await?;
    let problems: Vec<String> = requested
        .iter()
        .filter_map(|n| origin_problem(n, policy.get(*n)))
        .collect();
    if !problems.is_empty() {
        return Err(fix_hint(&problems));
    }

    // 2. The simulated install, including dependencies.
    let mut args: Vec<&str> = vec!["-s", "install"];
    args.extend(INSTALL_FLAGS);
    args.extend(&requested);
    let sim = run_text(exec, cancel, APT_GET, &args).await?;
    let set = parse_simulation(&sim)?;
    if let Some(missing) = requested.iter().find(|n| !set.contains(**n)) {
        return Err(format!(
            "{missing} is not part of the simulated install; the server changed since the plan. \
             Make a new plan."
        ));
    }
    if set.len() > MAX_INSTALL_SET {
        return Err(format!(
            "the install would add {} packages, more than the limit of {MAX_INSTALL_SET}",
            set.len()
        ));
    }

    // 3. Every extra (dependency) package comes from an allowed origin.
    let extras: Vec<&str> = set
        .iter()
        .map(String::as_str)
        .filter(|n| !requested.contains(n))
        .collect();
    if !extras.is_empty() {
        let extra_policy = policy_for(exec, cancel, &extras).await?;
        let problems: Vec<String> = extras
            .iter()
            .filter_map(|n| origin_problem(n, extra_policy.get(*n)))
            .collect();
        if !problems.is_empty() {
            return Err(fix_hint(&problems));
        }
    }

    let list: Vec<&str> = set.iter().map(String::as_str).collect();
    Ok(format!(
        "Install set ({} packages): {}",
        list.len(),
        list.join(" ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::plan as build_plan;
    use crate::preflight::tests::{code, fresh, ok};
    use ossiframe_core::{PreflightReport, ServerClass, SudoMode};

    const GOOD: &str = "git:\n  Installed: (none)\n  Candidate: 1:2.43.0-1\n  Version table:\n \
         1:2.43.0-1 500\n        500 http://archive.ubuntu.com/ubuntu noble-updates/main amd64 Packages\n        \
         500 http://security.ubuntu.com/ubuntu noble-security/main amd64 Packages\n";

    fn one_package_plan() -> Plan {
        let r = PreflightReport {
            observed_at: String::new(),
            os: "ubuntu 24.04".into(),
            sudo: SudoMode::Passwordless,
            server_class: ServerClass::Fresh,
            packages: BTreeMap::new(),
            total_mem_mb: Some(4000),
            free_disk_mb: Some(40_000),
            package_sources: Vec::new(),
            candidate_versions: BTreeMap::new(),
            sites: Vec::new(),
            conflicts: Vec::new(),
        };
        build_plan(&r, &["git".to_owned()]).unwrap()
    }

    async fn gate(fake: &crate::preflight::tests::Fake) -> Result<String, String> {
        run(
            fake,
            &CancelToken::new(),
            &one_package_plan(),
            STEP_CANDIDATE_CHECK,
        )
        .await
    }

    fn rig(policy_out: &str, sim_out: &str) -> crate::preflight::tests::Fake {
        fresh()
            .over("'/usr/bin/apt-get' '-s'", ok(sim_out))
            .over("'/usr/bin/apt-cache' 'policy'", ok(policy_out))
    }

    #[tokio::test]
    async fn passes_for_allowlisted_origins_and_returns_the_exact_set() {
        let sim = "Inst git-man (1:2.43.0-1 Ubuntu:24.04/noble-updates [all])\nInst git (1:2.43.0-1 Ubuntu:24.04/noble-updates [amd64])\nConf git (x)\n";
        let both = format!(
            "{GOOD}git-man:\n  Candidate: 1:2.43.0-1\n  Version table:\n     1:2.43.0-1 500\n        500 http://eu.archive.ubuntu.com/ubuntu noble/main amd64 Packages\n"
        );
        // The same canned policy answers both policy calls.
        let out = gate(&rig(&both, sim)).await.unwrap();
        assert_eq!(out, "Install set (2 packages): git git-man");
    }

    #[tokio::test]
    async fn fails_for_an_unknown_origin_or_a_missing_candidate() {
        let evil = GOOD
            .replace("archive.ubuntu.com", "ppa.example.org")
            .replace("security.ubuntu.com", "ppa.example.org");
        let e = gate(&rig(&evil, "Inst git (1)\n")).await.unwrap_err();
        assert!(
            e.contains("ppa.example.org") && e.contains("make a new plan"),
            "{e}"
        );

        let none = "git:\n  Installed: (none)\n  Candidate: (none)\n  Version table:\n";
        let e = gate(&rig(none, "")).await.unwrap_err();
        assert!(e.contains("no installation candidate"), "{e}");

        let e = gate(&rig("", "")).await.unwrap_err();
        assert!(e.contains("no entry"), "{e}");
    }

    #[tokio::test]
    async fn fails_when_a_dependency_comes_from_a_disallowed_origin_or_something_is_removed() {
        let policy = format!(
            "{GOOD}libx:\n  Candidate: 1\n  Version table:\n     1 500\n        500 http://deb.example.org/ubuntu noble/main amd64 Packages\n"
        );
        let e = gate(&rig(&policy, "Inst libx (1)\nInst git (1)\n"))
            .await
            .unwrap_err();
        assert!(e.contains("libx") && e.contains("deb.example.org"), "{e}");

        let e = gate(&rig(GOOD, "Remv apache2 [2]\nInst git (1)\n"))
            .await
            .unwrap_err();
        assert!(e.contains("remove"), "{e}");

        let e = gate(&rig(GOOD, "Inst other (1)\n")).await.unwrap_err();
        assert!(e.contains("not part of the simulated install"), "{e}");
    }

    #[tokio::test]
    async fn simulation_failure_and_unknown_step_fail() {
        let fake = fresh()
            .over(
                "'/usr/bin/apt-get' '-s'",
                code(100, "", "E: Unable to locate package"),
            )
            .over("'/usr/bin/apt-cache' 'policy'", ok(GOOD));
        assert!(gate(&fake).await.unwrap_err().contains("Unable to locate"));
        let r = run(
            &rig(GOOD, "Inst git (1)\n"),
            &CancelToken::new(),
            &one_package_plan(),
            "apt-install",
        )
        .await;
        assert!(r.unwrap_err().contains("unknown gate"));
    }

    #[tokio::test]
    async fn gate_commands_are_read_only_and_absolute() {
        let fake = rig(GOOD, "Inst git (1)\n");
        gate(&fake).await.unwrap();
        for c in fake.commands() {
            assert_eq!(c.privilege(), ossiframe_core::Privilege::User);
            assert!(c.program().starts_with('/'));
        }
    }

    #[test]
    fn simulation_and_names_are_validated() {
        assert!(valid_name("php8.3-fpm") && valid_name("libstdc++6"));
        assert!(!valid_name("-y") && !valid_name("a b") && !valid_name("A"));
        assert!(parse_simulation("Inst -bad (1)\n").is_err());
        assert!(parse_simulation("Purg x\n").is_err());
        assert!(parse_simulation("Conf x\n").unwrap().is_empty());
    }
}
