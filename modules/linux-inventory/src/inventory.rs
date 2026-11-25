//! Read-only server inventory built from predefined commands.
//!
//! This module depends only on `ossiframe-core`, so it can later move into a
//! reusable `linux-inventory` module unchanged.

use crate::parse;

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

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
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
    pub swap_total_kb: Option<u64>,
    pub swap_free_kb: Option<u64>,
    /// Busy percent over a ~1 s sample; `None` when it could not be measured.
    pub cpu_usage_percent: Option<f64>,
    /// Derived from `filesystems` (the entry mounted at `/`).
    pub root_fs: Option<DiskInfo>,
    pub filesystems: Vec<DiskInfo>,
    pub filesystems_truncated: bool,
    pub limitations: Vec<String>,
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
    ("filesystems", "df", &DF_ARGS),
];

/// Pseudo and temporary filesystem types that are never meaningful here.
const DF_ARGS: [&str; 24] = [
    "-P",
    "-B1",
    "-x",
    "tmpfs",
    "-x",
    "devtmpfs",
    "-x",
    "squashfs",
    "-x",
    "overlay",
    "-x",
    "efivarfs",
    "-x",
    "proc",
    "-x",
    "sysfs",
    "-x",
    "cgroup2",
    "-x",
    "devpts",
    "-x",
    "ramfs",
    "-x",
    "fuse.lxcfs",
];

/// Time between the two `/proc/stat` samples.
const CPU_SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

/// Outcome of one probe's command.
pub(crate) enum Probe {
    Output(String),
    Failed(String),
    /// Stop the whole collection: timeout, cancel or lost connection.
    Abort(ExecError),
}

async fn probe(exec: &dyn Executor, program: &str, args: &[&str], cancel: &CancelToken) -> Probe {
    probe_with(exec, program, args, cancel, false).await
}

/// Like `probe`, but output cut at the executor limit is a failure. Used for
/// structured output (JSON) that cannot be parsed when cut.
pub(crate) async fn probe_strict(
    exec: &dyn Executor,
    program: &str,
    args: &[&str],
    cancel: &CancelToken,
) -> Probe {
    probe_with(exec, program, args, cancel, true).await
}

async fn probe_with(
    exec: &dyn Executor,
    program: &str,
    args: &[&str],
    cancel: &CancelToken,
    reject_truncated: bool,
) -> Probe {
    let spec = match CommandSpec::new(program, args, PROBE_TIMEOUT) {
        Ok(s) => s,
        Err(e) => return Probe::Failed(e.to_string()),
    };
    match exec.run(&spec, cancel).await {
        Ok(r) if r.exit.success() && reject_truncated && r.truncated => {
            Probe::Failed("output was cut at the size limit".into())
        }
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
            "memory" => {
                if let Some((t, f)) = parse::swap(&out) {
                    info.swap_total_kb = t;
                    info.swap_free_kb = f;
                }
                parse::meminfo(&out).map(|(t, a)| {
                    info.memory_total_kb = t;
                    info.memory_available_kb = a;
                })
            }
            "filesystems" => parse::df_all(&out).map(|(v, truncated)| {
                info.root_fs = v.iter().find(|d| d.mount == "/").cloned();
                info.filesystems = v;
                info.filesystems_truncated = truncated;
                if truncated {
                    info.limitations.push(format!(
                        "Only the first {} filesystems are listed.",
                        parse::MAX_FILESYSTEMS
                    ));
                }
            }),
            _ => None,
        };
        if parsed.is_none() {
            info.probe_errors.push(ProbeError {
                probe: name,
                message: "output not recognized".into(),
            });
        }
    }
    if info.root_fs.is_none() {
        // Keep the message the Overview looks up for the root filesystem.
        info.probe_errors.push(ProbeError {
            probe: "rootFs",
            message: "root filesystem not available".into(),
        });
    }
    info.cpu_usage_percent = cpu_usage(exec, cancel, &mut info.probe_errors).await?;
    Ok(info)
}

/// Two `/proc/stat` samples with a cancel-aware sleep between them. `Ok(None)`
/// (plus a probe error) when a sample is missing or unusable; never estimated.
async fn cpu_usage(
    exec: &dyn Executor,
    cancel: &CancelToken,
    errors: &mut Vec<ProbeError>,
) -> Result<Option<f64>, ExecError> {
    let mut fail = |message: &str| {
        errors.push(ProbeError {
            probe: "cpuUsage",
            message: message.into(),
        });
        Ok(None)
    };
    let first = match probe(exec, "cat", &["/proc/stat"], cancel).await {
        Probe::Output(o) => parse::proc_stat_cpu(&o),
        Probe::Failed(m) => return fail(&m),
        Probe::Abort(e) => return Err(e),
    };
    let Some(first) = first else {
        return fail("first sample not recognized");
    };
    tokio::select! {
        () = tokio::time::sleep(CPU_SAMPLE_INTERVAL) => {}
        () = cancel.cancelled() => return Err(ExecError::Cancelled),
    }
    let second = match probe(exec, "cat", &["/proc/stat"], cancel).await {
        Probe::Output(o) => parse::proc_stat_cpu(&o),
        Probe::Failed(m) => return fail(&m),
        Probe::Abort(e) => return Err(e),
    };
    match second.and_then(|s| parse::cpu_usage(first, s)) {
        Some(p) => Ok(Some(p)),
        None => fail("not measurable from two samples"),
    }
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
                ("cat", Some("/proc/meminfo")) => ok(
                    "MemTotal:  2014560 kB\nMemFree: 1 kB\nMemAvailable: 1500000 kB\nSwapTotal: 1000 kB\nSwapFree: 400 kB\n",
                ),
                ("cat", Some("/proc/stat")) => ok("cpu  1 0 1 8 0 0 0 0 0 0\n"),
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
        assert_eq!(info.filesystems.len(), 1);
        assert_eq!(info.swap_total_kb, Some(1000));
        assert_eq!(info.swap_free_kb, Some(400));
        // Fake returns the same /proc/stat twice: zero delta, so not measurable.
        assert_eq!(info.cpu_usage_percent, None);
        let failed: Vec<_> = info.probe_errors.iter().map(|e| e.probe).collect();
        assert_eq!(failed, vec!["arch", "cpuCount", "cpuModel", "cpuUsage"]);
        assert!(
            info.probe_errors[0]
                .message
                .contains("may still be running")
        );
    }

    #[tokio::test]
    async fn cancel_during_cpu_sleep_aborts() {
        let cancel = CancelToken::new();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            c2.cancel();
        });
        assert_eq!(
            collect(&Fake, &cancel, "t".into()).await.unwrap_err(),
            ExecError::Cancelled
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
