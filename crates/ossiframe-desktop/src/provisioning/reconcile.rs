//! Read-only evidence about a remote operation, and the pure decision that
//! turns it into a reconcile result.
//!
//! Nothing here launches, resumes, stops or kills anything. The probes only
//! read. A probe that fails or cannot be parsed is `Unknown` or `Err`, and
//! unknown evidence never counts as clear.
use crate::remote_lock::{Evidence, StaleEvidence};
use ossiframe_core::command::CommandSpec;
use ossiframe_core::executor::{CancelToken, ExecResult, Executor};
use ossiframe_core::provision::{self, PackageState, Plan, StepMode};
use serde::Serialize;
use std::collections::BTreeMap;
use std::time::Duration;

const SYSTEMCTL: &str = "/usr/bin/systemctl";
const SYSTEMD_RUN: &str = "/usr/bin/systemd-run";
const JOURNALCTL: &str = "/usr/bin/journalctl";
const DPKG_QUERY: &str = "/usr/bin/dpkg-query";
const PGREP: &str = "/usr/bin/pgrep";
const FUSER: &str = "/usr/bin/fuser";
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_SHOW_LEN: usize = 4096;
const MAX_VALUE_LEN: usize = 64;

/// The id of the step that only refreshes package lists. Its completion says
/// nothing about any later step, and it is never inferred from package state.
pub const APT_UPDATE_STEP: &str = "apt-update";

/// Unit name of a durable step: the full operation UUID and the step index.
pub fn unit_name(operation_id: &str, idx: u32) -> String {
    format!("ossiframe-op-{operation_id}-{idx}")
}

fn user(program: &str, args: &[&str]) -> CommandSpec {
    CommandSpec::new(program, args, PROBE_TIMEOUT).expect("fixed probe command is valid")
}

fn root(program: &str, args: &[&str], timeout: Duration) -> CommandSpec {
    CommandSpec::privileged(program, args, timeout).expect("fixed probe command is valid")
}

// ---- commands ---------------------------------------------------------------

/// Starts one durable step as a transient unit. `--collect` is not used: it
/// would unload a failed unit and lose its result before it is persisted.
/// The environment of the step is part of the step's own argv (the module
/// wraps it in `/usr/bin/env`), so no `-E` option is added here.
pub fn launch_command(unit: &str, step: &CommandSpec) -> CommandSpec {
    let unit_arg = format!("--unit={unit}");
    let mut args: Vec<&str> = vec![
        &unit_arg,
        "--service-type=exec",
        "--remain-after-exit",
        "--quiet",
        "--",
        step.program(),
    ];
    args.extend(step.args().iter().map(String::as_str));
    root(SYSTEMD_RUN, &args, LAUNCH_TIMEOUT)
}

pub fn show_command(unit: &str) -> CommandSpec {
    let full = format!("{unit}.service");
    user(
        SYSTEMCTL,
        &[
            "show",
            "--property=LoadState,ActiveState,SubState,Result,ExecMainStatus,MainPID",
            &full,
        ],
    )
}

/// Last 200 journal lines of the unit, plain text.
pub fn journal_command(unit: &str) -> CommandSpec {
    let full = format!("{unit}.service");
    root(
        JOURNALCTL,
        &["-u", &full, "-o", "cat", "--no-pager", "-n", "200"],
        PROBE_TIMEOUT,
    )
}

/// Clears a unit that is confirmed `failed`.
pub fn reset_failed_command(unit: &str) -> CommandSpec {
    let full = format!("{unit}.service");
    root(SYSTEMCTL, &["reset-failed", &full], PROBE_TIMEOUT)
}

/// Stops a unit that is confirmed `active/exited` with no process.
pub fn stop_command(unit: &str) -> CommandSpec {
    let full = format!("{unit}.service");
    root(SYSTEMCTL, &["stop", &full], PROBE_TIMEOUT)
}

// ---- unit state ---------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnitObs {
    NotFound,
    /// The process (or its start or stop) is still in progress.
    Running,
    /// `active/exited`, `Result=success`, `ExecMainStatus=0`.
    Succeeded,
    Failed {
        result: String,
    },
    /// Anything else, for example `inactive`. Never read as success or failure.
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitShow {
    pub obs: UnitObs,
    pub active: String,
    pub sub: String,
    pub exit_status: Option<i32>,
    pub main_pid: Option<u32>,
}

impl UnitShow {
    /// True only for an `active/exited` unit with no remaining process, the
    /// one state in which `systemctl stop` cannot end running work.
    pub fn is_idle_exited(&self) -> bool {
        self.obs == UnitObs::Succeeded && self.sub == "exited" && self.main_pid == Some(0)
    }
}

fn token(v: &str) -> Option<&str> {
    let ok = !v.is_empty()
        && v.len() <= MAX_VALUE_LEN
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'+'));
    ok.then_some(v)
}

/// Parses `systemctl show` output. `None` when it cannot be classified.
pub fn parse_unit_show(out: &str) -> Option<UnitShow> {
    if out.len() > MAX_SHOW_LEN {
        return None;
    }
    let (mut load, mut active, mut sub, mut result, mut exit, mut pid) =
        (None, None, None, None, None, None);
    for line in out.lines() {
        if let Some((k, v)) = line.split_once('=') {
            match k {
                "LoadState" => load = Some(v),
                "ActiveState" => active = Some(v),
                "SubState" => sub = Some(v),
                "Result" => result = Some(v),
                "ExecMainStatus" => exit = Some(v),
                "MainPID" => pid = Some(v),
                _ => {}
            }
        }
    }
    let active_s = token(active?).map(str::to_owned);
    let sub_s = token(sub.unwrap_or("-")).map(str::to_owned);
    let exit_status = exit.and_then(|v| v.parse::<i32>().ok());
    let main_pid = pid.and_then(|v| v.parse::<u32>().ok());
    let (active_s, sub_s) = (active_s?, sub_s?);
    let result_s = result.and_then(token).unwrap_or("").to_owned();
    let obs = match load? {
        "not-found" => UnitObs::NotFound,
        "loaded" => match (active_s.as_str(), sub_s.as_str()) {
            ("failed", _) => UnitObs::Failed { result: result_s },
            ("active", "exited") if result_s == "success" && exit_status == Some(0) => {
                UnitObs::Succeeded
            }
            ("active", "exited") => UnitObs::Failed { result: result_s },
            ("active" | "activating" | "reloading" | "deactivating", _) => UnitObs::Running,
            (other, _) => UnitObs::Other(other.to_owned()),
        },
        _ => return None,
    };
    Some(UnitShow {
        obs,
        active: active_s,
        sub: sub_s,
        exit_status,
        main_pid,
    })
}

// ---- probes -------------------------------------------------------------------

/// What the probes saw. Errors are text for the evidence report.
#[derive(Debug, Clone)]
pub struct EvidenceSet {
    /// State of the recorded unit, when a step was launched.
    pub unit: Option<Result<UnitShow, String>>,
    /// `Ok(true)` when no package is in an interrupted or unsettled dpkg state
    /// (derived from `dpkg-query -W`, no root needed).
    pub dpkg_clean: Result<bool, String>,
    pub apt_processes: Evidence,
    pub lock_files: Evidence,
    pub step_units: Evidence,
    pub packages: Result<BTreeMap<String, PackageState>, String>,
    /// Unit name and `systemctl is-active` word. Informational only.
    pub services: Vec<(String, String)>,
}

impl EvidenceSet {
    pub fn stale(&self) -> StaleEvidence {
        StaleEvidence {
            step_units: self.step_units,
            apt_processes: self.apt_processes,
            lock_files: self.lock_files,
        }
    }

    /// No package management is active and dpkg is not interrupted.
    pub fn package_management_clear(&self) -> bool {
        self.dpkg_clean == Ok(true)
            && self.apt_processes == Evidence::Clear
            && self.lock_files == Evidence::Clear
    }
}

fn word(e: Evidence) -> &'static str {
    match e {
        Evidence::Clear => "none",
        Evidence::Present => "present",
        Evidence::Unknown => "could not be determined",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceItem {
    pub label: String,
    pub value: String,
}

/// Evidence as shown to the user and stored with the decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceReport {
    pub items: Vec<EvidenceItem>,
}

impl EvidenceReport {
    fn push(&mut self, label: &str, value: impl Into<String>) {
        self.items.push(EvidenceItem {
            label: label.to_owned(),
            value: value.into(),
        });
    }

    /// One plain-text line, for events.
    pub fn render(&self) -> String {
        self.items
            .iter()
            .map(|i| format!("{}: {}", i.label, i.value))
            .collect::<Vec<_>>()
            .join("; ")
    }

    pub fn from_set(e: &EvidenceSet) -> Self {
        let mut r = Self::default();
        match &e.unit {
            None => r.push("recorded unit", "no step was launched"),
            Some(Err(m)) => r.push("recorded unit", format!("not readable ({m})")),
            Some(Ok(u)) => r.push(
                "recorded unit",
                format!("{:?} ({}/{})", u.obs, u.active, u.sub),
            ),
        }
        match &e.dpkg_clean {
            Ok(true) => r.push("dpkg package states", "settled"),
            Ok(false) => r.push("dpkg package states", "interrupted or unsettled packages"),
            Err(m) => r.push("dpkg package states", format!("unavailable ({m})")),
        }
        r.push("apt/dpkg processes", word(e.apt_processes));
        r.push("apt/dpkg lock files held", word(e.lock_files));
        r.push("active ossiframe-op units", word(e.step_units));
        match &e.packages {
            Ok(p) => {
                let bad = p
                    .iter()
                    .filter(|(_, s)| !matches!(s, PackageState::Installed { .. }))
                    .count();
                r.push(
                    "plan packages",
                    format!("{} checked, {} not installed", p.len(), bad),
                );
            }
            Err(m) => r.push("plan packages", format!("unavailable ({m})")),
        }
        for (u, s) in &e.services {
            r.push(&format!("service {u}"), s.clone());
        }
        r
    }
}

fn text_ok(r: &ExecResult) -> bool {
    r.exit.success() && !r.truncated
}

fn classify_pgrep(r: Result<ExecResult, ossiframe_core::ExecError>) -> Evidence {
    match r {
        Ok(r) if r.exit == ossiframe_core::ExitStatus::Code(1) && r.stdout.trim().is_empty() => {
            Evidence::Clear
        }
        Ok(r) if text_ok(&r) && !r.stdout.trim().is_empty() => Evidence::Present,
        _ => Evidence::Unknown,
    }
}

fn classify_fuser(r: Result<ExecResult, ossiframe_core::ExecError>) -> Evidence {
    match r {
        // fuser exits 1 and prints no process ids when nothing uses the files.
        Ok(r) if r.exit == ossiframe_core::ExitStatus::Code(1) && r.stdout.trim().is_empty() => {
            Evidence::Clear
        }
        Ok(r) if r.exit.success() => Evidence::Present,
        _ => Evidence::Unknown,
    }
}

/// `systemctl list-units` lines: UNIT LOAD ACTIVE SUB DESCRIPTION.
fn classify_units(r: Result<ExecResult, ossiframe_core::ExecError>) -> Evidence {
    let Ok(r) = r else {
        return Evidence::Unknown;
    };
    if !text_ok(&r) {
        return Evidence::Unknown;
    }
    let mut result = Evidence::Clear;
    for line in r.stdout.lines().filter(|l| !l.trim().is_empty()) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 4 {
            return Evidence::Unknown;
        }
        let busy = match (f[2], f[3]) {
            ("active", "exited") | ("inactive", _) | ("failed", _) => false,
            ("active" | "activating" | "deactivating" | "reloading", _) => true,
            _ => return Evidence::Unknown,
        };
        if busy {
            result = Evidence::Present;
        }
    }
    result
}

/// The three probes that gate a manual resolution or a stale-lock release.
pub async fn collect_stale(exec: &dyn Executor, cancel: &CancelToken) -> StaleEvidence {
    let (units, procs, locks) = probe_activity(exec, cancel).await;
    StaleEvidence {
        step_units: units,
        apt_processes: procs,
        lock_files: locks,
    }
}

async fn probe_activity(
    exec: &dyn Executor,
    cancel: &CancelToken,
) -> (Evidence, Evidence, Evidence) {
    let units = classify_units(
        exec.run(
            &user(
                SYSTEMCTL,
                &[
                    "list-units",
                    "--all",
                    "--plain",
                    "--no-legend",
                    "ossiframe-op-*",
                ],
            ),
            cancel,
        )
        .await,
    );
    let procs = classify_pgrep(
        exec.run(
            &user(PGREP, &["-x", "-l", "apt|apt-get|dpkg|unattended-upgr"]),
            cancel,
        )
        .await,
    );
    let locks = classify_fuser(
        exec.run(
            &root(
                FUSER,
                &[
                    "/var/lib/dpkg/lock",
                    "/var/lib/dpkg/lock-frontend",
                    "/var/lib/apt/lists/lock",
                    "/var/cache/apt/archives/lock",
                ],
                PROBE_TIMEOUT,
            ),
            cancel,
        )
        .await,
    );
    (units, procs, locks)
}

fn valid_package_name(n: &str) -> bool {
    !n.is_empty()
        && !n.starts_with(['-', '.', '+'])
        && n.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'+' | b'-')
        })
}

fn valid_unit(n: &str) -> bool {
    !n.is_empty()
        && !n.starts_with('-')
        && n.len() <= 128
        && n.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'@' | b':'))
}

async fn probe_packages(
    exec: &dyn Executor,
    cancel: &CancelToken,
    plan: &Plan,
) -> Result<BTreeMap<String, PackageState>, String> {
    let names: Vec<&str> = plan.packages.iter().map(|p| p.name.as_str()).collect();
    if names.iter().any(|n| !valid_package_name(n)) {
        return Err("invalid package name in plan".into());
    }
    if names.is_empty() {
        return Ok(BTreeMap::new());
    }
    let mut args = vec![
        "-W",
        "-f",
        "${Package}\t${db:Status-Abbrev}\t${Version}\n",
        "--",
    ];
    args.extend(&names);
    let r = exec
        .run(&user(DPKG_QUERY, &args), cancel)
        .await
        .map_err(|e| e.to_string())?;
    // Exit 1 means some package is unknown to dpkg; the others are listed.
    let code_ok = matches!(r.exit, ossiframe_core::ExitStatus::Code(0 | 1));
    if !code_ok || r.truncated {
        return Err("dpkg-query failed".into());
    }
    let mut out: BTreeMap<String, PackageState> = names
        .iter()
        .map(|n| ((*n).to_owned(), PackageState::Absent))
        .collect();
    for line in r.stdout.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() != 3 {
            continue;
        }
        let Some(slot) = out.get_mut(f[0]) else {
            continue;
        };
        let status = f[1].trim();
        *slot = match status {
            "ii" => PackageState::Installed {
                version: f[2].chars().take(64).collect(),
            },
            "un" | "" => PackageState::Absent,
            other => PackageState::Other {
                status: other.chars().take(8).collect(),
            },
        };
    }
    Ok(out)
}

/// Collects all reconcile evidence. Every probe is read-only.
pub async fn collect(
    exec: &dyn Executor,
    cancel: &CancelToken,
    plan: &Plan,
    recorded_unit: Option<&str>,
) -> EvidenceSet {
    let unit = match recorded_unit {
        None => None,
        Some(u) => Some(match exec.run(&show_command(u), cancel).await {
            Ok(r) if r.exit.success() => {
                parse_unit_show(&r.stdout).ok_or_else(|| "unit state not classifiable".to_owned())
            }
            Ok(_) => Err("systemctl show failed".to_owned()),
            Err(e) => Err(e.to_string()),
        }),
    };
    let dpkg_clean = match exec
        .run(
            &user(DPKG_QUERY, &["-W", "-f", provision::DPKG_HEALTH_FORMAT]),
            cancel,
        )
        .await
    {
        Ok(r) if r.exit.success() && !r.truncated => provision::dpkg_states_settled(&r.stdout),
        Ok(_) => Err("dpkg-query failed".to_owned()),
        Err(e) => Err(e.to_string()),
    };
    let (step_units, apt_processes, lock_files) = probe_activity(exec, cancel).await;
    let packages = probe_packages(exec, cancel, plan).await;
    let mut services = Vec::new();
    for s in plan.services.iter().take(16) {
        if !valid_unit(&s.unit) {
            continue;
        }
        let state = match exec
            .run(&user(SYSTEMCTL, &["is-active", &s.unit]), cancel)
            .await
        {
            Ok(r) => token(r.stdout.trim()).unwrap_or("unknown").to_owned(),
            Err(_) => "unavailable".to_owned(),
        };
        services.push((s.unit.clone(), state));
    }
    EvidenceSet {
        unit,
        dpkg_clean,
        apt_processes,
        lock_files,
        step_units,
        packages,
        services,
    }
}

// ---- decision -------------------------------------------------------------------

/// One recorded step, reduced to what the decision needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepFacts {
    pub idx: u32,
    pub step_id: String,
    pub mode: StepMode,
    /// `pending`, `running`, `succeeded`, `failed`, `skipped` or `unknown`.
    pub status: String,
}

impl StepFacts {
    /// Launched and not finished: its outcome is what reconcile must find.
    pub fn in_flight(&self) -> bool {
        matches!(self.status.as_str(), "running" | "unknown")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Stay in UnknownOutcome. Nothing is guessed.
    Stay(String),
    /// Every step is recorded as succeeded. Verification decides.
    AllStepsRecorded,
    /// No step was ever launched. The first unlaunched step is `next`.
    NeverLaunched { next: u32 },
    /// The recorded unit finished successfully.
    UnitSucceeded { idx: u32, last: bool },
    /// The recorded unit finished with a failure.
    UnitFailed { idx: u32, result: String },
    /// The unit is gone, but package evidence shows the last step's work is
    /// complete. Verification decides.
    InferredComplete { idx: u32 },
}

/// Pure reconcile table. `steps` is every step of the plan in order.
pub fn decide(steps: &[StepFacts], e: &EvidenceSet, plan: &Plan) -> Decision {
    let in_flight = steps.iter().rev().find(|s| s.in_flight());
    let Some(s) = in_flight else {
        if let Some(p) = steps.iter().find(|s| s.status == "pending") {
            return Decision::NeverLaunched { next: p.idx };
        }
        if steps.iter().any(|s| s.status == "failed") {
            return Decision::Stay("a step is recorded as failed".into());
        }
        return Decision::AllStepsRecorded;
    };
    if s.mode != StepMode::Durable {
        return Decision::Stay(
            "the interrupted step ran over the connection, so its outcome cannot be read back"
                .into(),
        );
    }
    let last = steps
        .iter()
        .all(|o| o.idx == s.idx || o.status == "succeeded")
        && steps.last().is_some_and(|l| l.idx == s.idx);
    match &e.unit {
        None | Some(Err(_)) => {
            Decision::Stay("the state of the step's unit could not be read".into())
        }
        Some(Ok(u)) => match &u.obs {
            UnitObs::Running => Decision::Stay("the step is still running on the server".into()),
            UnitObs::Succeeded => Decision::UnitSucceeded { idx: s.idx, last },
            UnitObs::Failed { result } => Decision::UnitFailed {
                idx: s.idx,
                result: result.clone(),
            },
            UnitObs::Other(state) => {
                Decision::Stay(format!("the step's unit is in state '{state}'"))
            }
            UnitObs::NotFound => {
                // A missing unit proves nothing: it may have been collected or
                // the host may have rebooted. Only a clear package state for
                // the last non-update step lets verification decide.
                let installed = e.packages.as_ref().is_ok_and(|p| {
                    !plan.packages.is_empty()
                        && plan
                            .packages
                            .iter()
                            .all(|c| matches!(p.get(&c.name), Some(PackageState::Installed { .. })))
                });
                if last && s.step_id != APT_UPDATE_STEP && e.package_management_clear() && installed
                {
                    Decision::InferredComplete { idx: s.idx }
                } else {
                    Decision::Stay(
                        "the step's unit no longer exists and the evidence does not show whether \
                         the step ran to completion"
                            .into(),
                    )
                }
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn show(load: &str, active: &str, sub: &str, result: &str, exit: &str, pid: &str) -> String {
        format!(
            "LoadState={load}\nActiveState={active}\nSubState={sub}\nResult={result}\nExecMainStatus={exit}\nMainPID={pid}\n"
        )
    }

    #[test]
    fn unit_state_classification() {
        let p = |s: &str| parse_unit_show(s).map(|u| u.obs);
        assert_eq!(
            p(&show("loaded", "active", "exited", "success", "0", "0")),
            Some(UnitObs::Succeeded)
        );
        assert_eq!(
            p(&show("loaded", "active", "running", "success", "0", "42")),
            Some(UnitObs::Running)
        );
        assert_eq!(
            p(&show("loaded", "failed", "failed", "exit-code", "100", "0")),
            Some(UnitObs::Failed {
                result: "exit-code".into()
            })
        );
        // exited with a non-zero status is a failure, never a success.
        assert!(matches!(
            p(&show("loaded", "active", "exited", "exit-code", "1", "0")),
            Some(UnitObs::Failed { .. })
        ));
        assert_eq!(
            p(&show("not-found", "inactive", "dead", "success", "0", "0")),
            Some(UnitObs::NotFound)
        );
        assert!(matches!(
            p(&show("loaded", "inactive", "dead", "success", "0", "0")),
            Some(UnitObs::Other(_))
        ));
        assert_eq!(p("garbage"), None);
        assert_eq!(p(&show("loaded", "act ive;x", "a", "b", "0", "0")), None);
        assert_eq!(p(&"x".repeat(MAX_SHOW_LEN + 1)), None);
    }

    #[test]
    fn only_an_idle_exited_unit_may_be_stopped() {
        let ok = parse_unit_show(&show("loaded", "active", "exited", "success", "0", "0")).unwrap();
        assert!(ok.is_idle_exited());
        let busy =
            parse_unit_show(&show("loaded", "active", "exited", "success", "0", "77")).unwrap();
        assert!(!busy.is_idle_exited());
    }

    #[test]
    fn launch_command_has_fixed_shape_and_no_environment_option() {
        let step = CommandSpec::privileged(
            "/usr/bin/env",
            &[
                "DEBIAN_FRONTEND=noninteractive",
                "/usr/bin/apt-get",
                "update",
            ],
            Duration::from_secs(60),
        )
        .unwrap();
        let line = launch_command("ossiframe-op-x-0", &step).to_command_line();
        assert!(line.starts_with("'/usr/bin/sudo' '-n' '--' '/usr/bin/systemd-run' '--unit=ossiframe-op-x-0' '--service-type=exec' '--remain-after-exit' '--quiet' '--' '/usr/bin/env'"));
        assert!(!line.contains("'-E'") && !line.contains("--collect"));
    }

    #[test]
    fn activity_classification_never_reads_unknown_as_clear() {
        use ossiframe_core::ExitStatus;
        let r = |code, out: &str| {
            Ok(ExecResult {
                exit: ExitStatus::Code(code),
                stdout: out.into(),
                stderr: String::new(),
                truncated: false,
            })
        };
        assert_eq!(classify_pgrep(r(1, "")), Evidence::Clear);
        assert_eq!(classify_pgrep(r(0, "12 apt-get\n")), Evidence::Present);
        assert_eq!(classify_pgrep(r(2, "")), Evidence::Unknown);
        assert_eq!(classify_fuser(r(1, "")), Evidence::Clear);
        assert_eq!(
            classify_fuser(r(0, "/var/lib/dpkg/lock: 12")),
            Evidence::Present
        );
        assert_eq!(classify_fuser(r(1, "12")), Evidence::Unknown);
        assert_eq!(classify_fuser(r(127, "")), Evidence::Unknown);
        let u = "ossiframe-op-a-0.service loaded active exited x\n";
        assert_eq!(classify_units(r(0, u)), Evidence::Clear);
        let u = "ossiframe-op-a-0.service loaded active running x\n";
        assert_eq!(classify_units(r(0, u)), Evidence::Present);
        assert_eq!(classify_units(r(0, "")), Evidence::Clear);
        assert_eq!(classify_units(r(0, "weird\n")), Evidence::Unknown);
        assert_eq!(classify_units(r(1, "")), Evidence::Unknown);
    }
}
