//! Read-only server inventory built from predefined commands.
//!
//! This module depends only on `ossiframe-core`, so it can later move into a
//! reusable `linux-inventory` module unchanged.

pub mod parse;

use ossiframe_core::{CancelToken, CommandSpec, ExecError, Executor};
use serde::Serialize;
use std::time::Duration;

pub const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OsInfo {
    pub id: Option<String>,
    pub version_id: Option<String>,
    pub pretty_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskInfo {
    pub mount: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeError {
    pub probe: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub observed_at: String,
    pub hostname: Option<String>,
    pub os: Option<OsInfo>,
    pub kernel: Option<String>,
    pub arch: Option<String>,
    pub uptime_seconds: Option<u64>,
    pub cpu_count: Option<u32>,
    pub cpu_model: Option<String>,
    pub memory_total_kb: Option<u64>,
    pub memory_available_kb: Option<u64>,
    pub root_fs: Option<DiskInfo>,
    pub probe_errors: Vec<ProbeError>,
}

/// Fixed probes: (name, program, args). No input from the frontend.
const PROBES: &[(&str, &str, &[&str])] = &[
    ("hostname", "hostname", &[]),
    ("os", "cat", &["/etc/os-release"]),
    ("kernel", "uname", &["-r"]),
    ("arch", "uname", &["-m"]),
    ("uptime", "cat", &["/proc/uptime"]),
    ("cpuCount", "nproc", &[]),
    ("cpuModel", "lscpu", &[]),
    ("memory", "cat", &["/proc/meminfo"]),
    ("rootFs", "df", &["-P", "-B1", "/"]),
];

/// Outcome of one probe's command.
enum Probe {
    Output(String),
    Failed(String),
    /// Stop the whole collection: timeout, cancel or lost connection.
    Abort(ExecError),
}

async fn probe(exec: &dyn Executor, program: &str, args: &[&str], cancel: &CancelToken) -> Probe {
    let spec = match CommandSpec::new(program, args, PROBE_TIMEOUT) {
        Ok(s) => s,
        Err(e) => return Probe::Failed(e.to_string()),
    };
    match exec.run(&spec, cancel).await {
        Ok(r) if r.exit.success() => Probe::Output(r.stdout),
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

/// Runs every probe in order. A failed or unparsable probe leaves its fields
/// empty and adds a `probe_errors` entry. Cancellation or a lost connection
/// stops collection and returns the error.
pub async fn collect(
    exec: &dyn Executor,
    cancel: &CancelToken,
    observed_at: String,
) -> Result<ServerInfo, ExecError> {
    let mut info = ServerInfo {
        observed_at,
        ..Default::default()
    };
    for (name, program, args) in PROBES {
        let out = match probe(exec, program, args, cancel).await {
            Probe::Output(o) => o,
            Probe::Failed(message) => {
                info.probe_errors.push(ProbeError {
                    probe: name,
                    message,
                });
                continue;
            }
            Probe::Abort(e) => return Err(e),
        };
        let parsed = match *name {
            "hostname" => parse::first_line(&out).map(|v| info.hostname = Some(v)),
            "os" => parse::os_release(&out).map(|v| info.os = Some(v)),
            "kernel" => parse::first_line(&out).map(|v| info.kernel = Some(v)),
            "arch" => parse::first_line(&out).map(|v| info.arch = Some(v)),
            "uptime" => parse::uptime(&out).map(|v| info.uptime_seconds = Some(v)),
            "cpuCount" => parse::cpu_count(&out).map(|v| info.cpu_count = Some(v)),
            "cpuModel" => parse::lscpu_model(&out).map(|v| info.cpu_model = Some(v)),
            "memory" => parse::meminfo(&out).map(|(t, a)| {
                info.memory_total_kb = t;
                info.memory_available_kb = a;
            }),
            "rootFs" => parse::df_root(&out).map(|v| info.root_fs = Some(v)),
            _ => None,
        };
        if parsed.is_none() {
            info.probe_errors.push(ProbeError {
                probe: name,
                message: "output not recognized".into(),
            });
        }
    }
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use ossiframe_core::{ExecResult, ExitStatus};

    struct Fake;

    #[async_trait]
    impl Executor for Fake {
        async fn run(&self, spec: &CommandSpec, _: &CancelToken) -> Result<ExecResult, ExecError> {
            let ok = |s: &str| {
                Ok(ExecResult {
                    exit: ExitStatus::Code(0),
                    stdout: s.into(),
                    stderr: String::new(),
                    truncated: false,
                })
            };
            match (spec.program(), spec.args().first().map(String::as_str)) {
                ("hostname", _) => ok("web-01\n"),
                ("cat", Some("/etc/os-release")) => {
                    ok("ID=ubuntu\nVERSION_ID=\"24.04\"\nPRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\n")
                }
                ("uname", Some("-r")) => ok("6.8.0-45-generic\n"),
                ("uname", Some("-m")) => Err(ExecError::Timeout),
                ("cat", Some("/proc/uptime")) => ok("12345.67 54321.00\n"),
                ("nproc", _) => ok("garbage\n"),
                ("lscpu", _) => Ok(ExecResult {
                    exit: ExitStatus::Code(127),
                    stdout: String::new(),
                    stderr: "lscpu: not found\n".into(),
                    truncated: false,
                }),
                ("cat", Some("/proc/meminfo")) => {
                    ok("MemTotal:  2014560 kB\nMemFree: 1 kB\nMemAvailable: 1500000 kB\n")
                }
                ("df", _) => ok(
                    "Filesystem 1-blocks Used Available Capacity Mounted on\n/dev/vda1 41555521536 2779279360 38759464960 7% /\n",
                ),
                _ => ok(""),
            }
        }
    }

    #[tokio::test]
    async fn collects_what_it_can_and_reports_the_rest() {
        let info = collect(&Fake, &CancelToken::new(), "t".into())
            .await
            .unwrap();
        assert_eq!(info.hostname.as_deref(), Some("web-01"));
        assert_eq!(
            info.os.as_ref().unwrap().version_id.as_deref(),
            Some("24.04")
        );
        assert_eq!(info.kernel.as_deref(), Some("6.8.0-45-generic"));
        assert_eq!(info.arch, None);
        assert_eq!(info.uptime_seconds, Some(12345));
        assert_eq!(info.cpu_count, None);
        assert_eq!(info.cpu_model, None);
        assert_eq!(info.memory_total_kb, Some(2014560));
        assert_eq!(info.root_fs.as_ref().unwrap().total_bytes, 41555521536);
        let failed: Vec<_> = info.probe_errors.iter().map(|e| e.probe).collect();
        assert_eq!(failed, vec!["arch", "cpuCount", "cpuModel"]);
        assert!(
            info.probe_errors[0]
                .message
                .contains("may still be running")
        );
    }

    struct Lost;
    #[async_trait]
    impl Executor for Lost {
        async fn run(&self, _: &CommandSpec, _: &CancelToken) -> Result<ExecResult, ExecError> {
            Err(ExecError::ConnectionLost)
        }
    }

    #[tokio::test]
    async fn lost_connection_aborts() {
        assert_eq!(
            collect(&Lost, &CancelToken::new(), "t".into())
                .await
                .unwrap_err(),
            ExecError::ConnectionLost
        );
    }
}
