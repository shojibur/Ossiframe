//! The `software` operation: server software known to the dpkg database,
//! from the fixed command `dpkg-query -W -f '${Package}\t${Version}\t${db:Status-Abbrev}\n'`
//! with a fixed list of package names.
//!
//! The evidence is the package database only. Software installed outside
//! dpkg (nvm, phar files, source builds) is not detected. No version
//! commands such as `nginx -v` are run.
//!
//! Names containing `*` are patterns that `dpkg-query` matches itself
//! ("normal shell wildcard characters are allowed in package-name-pattern",
//! `dpkg-query(1)`). The executor single-quotes every argument, so the shell
//! never expands them. The parser also drops any returned name that matches
//! none of the fixed names, so unexpected output cannot widen the result.
//! Exit status 1 with partial stdout is normal when some names are not
//! installed.

use crate::inventory::{PROBE_TIMEOUT, Probe, ProbeError};

use ossiframe_core::{CancelToken, CommandSpec, ExecError, Executor, ExitStatus};
use serde::Serialize;

/// Fixed package names and patterns, in the order they are passed.
pub const PACKAGE_PATTERNS: [&str; 14] = [
    "nginx",
    "apache2",
    "php*-fpm",
    "php*-cli",
    "composer",
    "nodejs",
    "python3",
    "mysql-server",
    "mariadb-server",
    "postgresql",
    "redis-server",
    "docker.io",
    "docker-ce",
    "git",
];

/// Most packages returned. Truncation is recorded in `truncated`.
pub const MAX_PACKAGES: usize = 100;
/// Most lines read, to bound work on hostile or huge output.
const MAX_LINES: usize = 1000;
/// Longer lines are skipped, not cut.
const MAX_LINE: usize = 512;
const MAX_FIELD: usize = 128;

const FORMAT: &str = "${Package}\\t${Version}\\t${db:Status-Abbrev}\\n";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Package {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SoftwareInfo {
    pub observed_at: String,
    /// False when the command failed; `packages` is empty.
    pub available: bool,
    /// Only packages with dpkg status `ii`.
    pub packages: Vec<Package>,
    /// True when the list was cut or lines were not recognized.
    pub truncated: bool,
    pub limitations: Vec<String>,
    pub probe_errors: Vec<ProbeError>,
}

/// Matches `name` against a fixed name or a pattern with `*` wildcards.
fn matches_pattern(pattern: &str, name: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or("");
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    if !pattern.contains('*') {
        return rest.is_empty();
    }
    let mut parts: Vec<&str> = parts.collect();
    let last = parts.pop().unwrap_or("");
    for mid in parts {
        match rest.find(mid) {
            Some(i) => rest = &rest[i + mid.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

fn is_wanted(name: &str) -> bool {
    PACKAGE_PATTERNS.iter().any(|p| matches_pattern(p, name))
}

fn clean_field(s: &str) -> Option<String> {
    if s.is_empty() || s.len() > MAX_FIELD || s.chars().any(char::is_control) {
        return None;
    }
    Some(s.to_owned())
}

enum Line {
    Installed(Package),
    /// A well-formed line whose status is not `ii`, or a name not in the list.
    Ignored,
    Garbage,
}

fn parse_line(line: &str) -> Line {
    if line.len() > MAX_LINE {
        return Line::Garbage;
    }
    let fields: Vec<&str> = line.split('\t').collect();
    let [name, version, status] = fields[..] else {
        return Line::Garbage;
    };
    let (Some(name), Some(version)) = (clean_field(name), clean_field(version)) else {
        return Line::Garbage;
    };
    // `${db:Status-Abbrev}` is three characters (want, status, error flag),
    // for example "ii ". Only exactly `ii` counts as installed: "rc", "hi",
    // "un" and "iiR" (reinstall required) do not.
    if status.is_empty() || status.len() > 3 {
        return Line::Garbage;
    }
    if status.trim_end() != "ii" {
        return Line::Ignored;
    }
    if !is_wanted(&name) {
        return Line::Ignored;
    }
    Line::Installed(Package { name, version })
}

struct Parsed {
    packages: Vec<Package>,
    truncated: bool,
    skipped: usize,
}

fn parse(s: &str) -> Parsed {
    let mut parsed = Parsed {
        packages: Vec::new(),
        truncated: false,
        skipped: 0,
    };
    for (i, line) in s.lines().enumerate() {
        if i >= MAX_LINES {
            parsed.truncated = true;
            break;
        }
        if line.trim().is_empty() {
            continue;
        }
        match parse_line(line) {
            // Same name twice (for example two architectures): keep the first.
            Line::Installed(p) if parsed.packages.iter().any(|q| q.name == p.name) => {}
            Line::Installed(p) if parsed.packages.len() < MAX_PACKAGES => parsed.packages.push(p),
            Line::Installed(_) => {
                parsed.truncated = true;
                break;
            }
            Line::Ignored => {}
            Line::Garbage => parsed.skipped += 1,
        }
    }
    parsed
}

/// True when stderr is non-empty and every line is dpkg-query's
/// "no packages found matching ..." notice, the normal reason for exit 1.
fn only_no_match_notices(stderr: &str) -> bool {
    let mut any = false;
    for line in stderr.lines().filter(|l| !l.trim().is_empty()) {
        if !line
            .trim()
            .starts_with("dpkg-query: no packages found matching ")
        {
            return false;
        }
        any = true;
    }
    any
}

/// Runs `dpkg-query -W`. Exit 1 is normal only when stderr consists solely of
/// "no packages found matching" lines; other stderr (permission denied, a
/// dpkg database error) makes the probe fail even if stdout is empty.
async fn run_dpkg(exec: &dyn Executor, args: &[&str], cancel: &CancelToken) -> Probe {
    let spec = match CommandSpec::new("dpkg-query", args, PROBE_TIMEOUT) {
        Ok(s) => s,
        Err(e) => return Probe::Failed(e.to_string()),
    };
    match exec.run(&spec, cancel).await {
        Ok(r) if r.truncated => Probe::Failed("output was cut at the size limit".into()),
        Ok(r) if r.exit.success() => Probe::Output(r.stdout),
        Ok(r) if r.exit == ExitStatus::Code(1) && only_no_match_notices(&r.stderr) => {
            Probe::Output(r.stdout)
        }
        Ok(r) => {
            let first = r
                .stderr
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(200)
                .collect::<String>();
            Probe::Failed(format!("exit {:?}: {first}", r.exit))
        }
        Err(ExecError::Timeout) => {
            Probe::Failed("timed out; the command may still be running on the server".into())
        }
        Err(ExecError::Channel(m)) => Probe::Failed(m.chars().take(200).collect()),
        Err(e) => Probe::Abort(e),
    }
}

/// Runs `dpkg-query -W`. Exit 1 with partial output is accepted. Any other
/// failure gives `available = false`. Cancel or a lost connection returns
/// the error.
pub async fn collect(
    exec: &dyn Executor,
    cancel: &CancelToken,
    observed_at: String,
) -> Result<SoftwareInfo, ExecError> {
    let mut args: Vec<&str> = vec!["-W", "-f", FORMAT];
    args.extend(PACKAGE_PATTERNS);
    let output = match run_dpkg(exec, &args, cancel).await {
        Probe::Output(o) => o,
        Probe::Failed(message) => {
            return Ok(SoftwareInfo {
                observed_at,
                probe_errors: vec![ProbeError {
                    probe: "software",
                    message,
                }],
                ..Default::default()
            });
        }
        Probe::Abort(e) => return Err(e),
    };
    let parsed = parse(&output);
    let mut limitations = vec![
        "Detection uses the dpkg package database only. Software installed outside dpkg \
         (nvm, phar files, source builds) is not detected."
            .to_owned(),
    ];
    if parsed.truncated {
        limitations.push(format!(
            "The package list is incomplete: at most {MAX_PACKAGES} packages are listed."
        ));
    }
    if parsed.skipped > 0 {
        limitations.push(format!(
            "{} line(s) of package output were not recognized and are not listed.",
            parsed.skipped
        ));
    }
    Ok(SoftwareInfo {
        observed_at,
        available: true,
        packages: parsed.packages,
        truncated: parsed.truncated || parsed.skipped > 0,
        limitations,
        probe_errors: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use ossiframe_core::ExecResult;

    fn names(p: &Parsed) -> Vec<&str> {
        p.packages.iter().map(|p| p.name.as_str()).collect()
    }

    #[test]
    fn only_ii_status_counts_as_installed() {
        let out = "nginx\t1.24.0-2ubuntu7\tii \n\
                   apache2\t2.4.58-1\trc \n\
                   git\t1:2.43.0-1\thi \n\
                   composer\t2.7.1-2\tun \n\
                   nodejs\t18.19.1\tiiR\n\
                   redis-server\t5:7.0.15-1\tii \n";
        let p = parse(out);
        assert_eq!(names(&p), ["nginx", "redis-server"]);
        assert_eq!(p.packages[0].version, "1.24.0-2ubuntu7");
        assert_eq!((p.truncated, p.skipped), (false, 0));
    }

    #[test]
    fn globs_match_only_fixed_patterns() {
        let out = "php8.3-fpm\t8.3.6-0ubuntu0.24.04.2\tii \n\
                   php8.3-cli\t8.3.6-0ubuntu0.24.04.2\tii \n\
                   php8.3-common\t8.3.6\tii \n\
                   php-fpm\t2:8.3+93\tii \n\
                   nginx-common\t1.24.0\tii \n\
                   python3\t3.12.3-0\tii \n";
        assert_eq!(
            names(&parse(out)),
            ["php8.3-fpm", "php8.3-cli", "php-fpm", "python3"]
        );
        assert!(matches_pattern("php*-fpm", "php8.3-fpm"));
        assert!(!matches_pattern("php*-fpm", "php8.3-fpm-extra"));
        assert!(!matches_pattern("nginx", "nginx-common"));
        assert!(!matches_pattern("php*-cli", "ph"));
    }

    #[test]
    fn empty_and_garbage_output() {
        let p = parse("");
        assert!(p.packages.is_empty() && !p.truncated && p.skipped == 0);
        let p = parse("garbage\n\nnginx\tonly-two\nnginx\t1\tii \textra\nnginx\t\tii \n");
        assert!(p.packages.is_empty());
        assert_eq!(p.skipped, 4);
        let p = parse(&format!("nginx\t{}\tii \n", "9".repeat(MAX_FIELD + 1)));
        assert_eq!((p.packages.len(), p.skipped), (0, 1));
        let p = parse(&format!("nginx\t1\tii \t{}\n", "x".repeat(MAX_LINE)));
        assert_eq!(p.skipped, 1);
        let p = parse("nginx\t1\t\n");
        assert_eq!(p.skipped, 1);
    }

    #[test]
    fn duplicates_are_dropped_and_list_is_capped() {
        let p = parse("nginx\t1\tii \nnginx\t2\tii \n");
        assert_eq!(p.packages.len(), 1);
        assert_eq!(p.packages[0].version, "1");
        let out: String = (0..MAX_PACKAGES + 1)
            .map(|i| format!("php{i}-fpm\t1\tii \n"))
            .collect();
        let p = parse(&out);
        assert_eq!(p.packages.len(), MAX_PACKAGES);
        assert!(p.truncated);
    }

    const NO_MATCH: &str = "dpkg-query: no packages found matching apache2\n";

    struct Fake {
        stdout: &'static str,
        exit: ExitStatus,
        stderr: &'static str,
    }

    #[async_trait]
    impl Executor for Fake {
        async fn run(&self, spec: &CommandSpec, _: &CancelToken) -> Result<ExecResult, ExecError> {
            assert_eq!(spec.program(), "dpkg-query");
            assert_eq!(
                spec.args()[..3],
                [
                    "-W",
                    "-f",
                    "${Package}\\t${Version}\\t${db:Status-Abbrev}\\n"
                ]
            );
            assert_eq!(spec.args()[3..], PACKAGE_PATTERNS);
            Ok(ExecResult {
                exit: self.exit.clone(),
                stdout: self.stdout.into(),
                stderr: self.stderr.into(),
                truncated: false,
            })
        }
    }

    #[tokio::test]
    async fn collect_accepts_exit_1_with_partial_output() {
        let cancel = CancelToken::new();
        let fake = Fake {
            stdout: "nginx\t1.24.0\tii \ngit\t1:2.43\trc \n",
            exit: ExitStatus::Code(1),
            stderr: NO_MATCH,
        };
        let info = collect(&fake, &cancel, "t".into()).await.unwrap();
        assert!(info.available && !info.truncated);
        assert_eq!(info.packages.len(), 1);
        assert!(info.limitations[0].contains("outside dpkg"));

        let none = Fake {
            stdout: "",
            exit: ExitStatus::Code(1),
            stderr: NO_MATCH,
        };
        let info = collect(&none, &cancel, "t".into()).await.unwrap();
        assert!(info.available && info.packages.is_empty());
    }

    #[tokio::test]
    async fn collect_reports_other_failures_as_unavailable() {
        let cancel = CancelToken::new();
        for exit in [
            ExitStatus::Code(2),
            ExitStatus::Code(127),
            ExitStatus::Unknown,
        ] {
            let fake = Fake {
                stdout: "",
                exit,
                stderr: NO_MATCH,
            };
            let info = collect(&fake, &cancel, "t".into()).await.unwrap();
            assert!(!info.available && info.packages.is_empty());
            assert_eq!(info.probe_errors[0].probe, "software");
        }
    }

    #[tokio::test]
    async fn exit_1_is_normal_only_for_no_match_notices() {
        let cancel = CancelToken::new();
        let many =
            "dpkg-query: no packages found matching a\ndpkg-query: no packages found matching b\n";
        for stderr in [
            many,
            "  dpkg-query: no packages found matching php*-fpm  \n\n",
        ] {
            let fake = Fake {
                stdout: "",
                exit: ExitStatus::Code(1),
                stderr,
            };
            let info = collect(&fake, &cancel, "t".into()).await.unwrap();
            assert!(info.available, "{stderr:?}");
        }
        for stderr in [
            "",
            "dpkg-query: error: cannot access archive: Permission denied\n",
            "dpkg-query: no packages found matching a\ndpkg: error: database is locked\n",
            "dpkg-query: error: no packages found matching x\n",
        ] {
            let fake = Fake {
                stdout: "",
                exit: ExitStatus::Code(1),
                stderr,
            };
            let info = collect(&fake, &cancel, "t".into()).await.unwrap();
            assert!(!info.available, "{stderr:?}");
            assert_eq!(info.probe_errors[0].probe, "software");
        }
    }
}
