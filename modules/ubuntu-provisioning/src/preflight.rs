//! Preflight: read-only fact collection, server classification and the list
//! of conflicts. Collection runs fixed commands with absolute paths through
//! the `Executor`; the only privileged command is the sudo probe. Evaluation
//! is a pure function of the collected `Facts`, so every rule is testable
//! without a server.
//!
//! Missing evidence is never read as absence of a conflict: every fact is an
//! `Evidence` that is `Err(reason)` when the probe failed, timed out, was
//! cut at a cap or could not be parsed, and each rule that needs it then
//! produces a Blocker.
//!
//! The "previously verified Ossiframe-managed" evidence comes from the
//! caller in [`PreflightInput::prior_operations`]. This module never reads
//! SQLite.

use crate::components::{self, COMPONENTS, ComponentDef, DEFAULT_SELECTION};
use crate::sources;
use ossiframe_core::provision;
use ossiframe_core::{
    CancelToken, CommandSpec, ExecError, ExecResult, Executor, ExitStatus, OperationError,
    PackageState, PreflightReport, ServerClass, Severity, SudoMode, ValidationError,
};
use ossiframe_core::{Conflict, PlanError};
use ossiframe_mod_linux_inventory as inventory;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_LIST_LINES: usize = 200;
const MAX_SOURCE_FILES: usize = 50;
const MAX_UNITS_SHOWN: usize = 5;
const MIN_RAM_MYSQL_MB: u64 = 900;
const WARN_RAM_MYSQL_MB: u64 = 1843;
const DISK_HEADROOM_MB: u64 = 1024;

const SOURCES_LIST: &str = "/etc/apt/sources.list";
const SOURCES_DIR: &str = "/etc/apt/sources.list.d";

/// Ports whose listeners must belong to a selected component:
/// (port, owning component, process name `ss` shows).
const MANAGED_PORTS: [(u16, &str, &str); 5] = [
    (80, "nginx", "nginx"),
    (443, "nginx", "nginx"),
    (3306, "mysql", "mysqld"),
    (33060, "mysql", "mysqld"),
    (6379, "redis", "redis-server"),
];
const FOREIGN_PACKAGES: [&str; 3] = ["apache2", "mariadb-server", "postgresql"];
const FOREIGN_UNITS: [&str; 3] = ["apache2.service", "mariadb.service", "postgresql.service"];

/// A fact, or the reason it could not be established.
pub type Evidence<T> = Result<T, String>;

/// Earlier operations for the same identity snapshot, defined in Core so the
/// desktop engine can pass them through the `Provisioner` contract.
pub use ossiframe_core::PriorOperation;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreflightInput {
    /// Component ids; dependencies are added. Unknown ids are rejected.
    pub selection: Vec<String>,
    /// RFC 3339.
    pub observed_at: String,
    pub prior_operations: Vec<PriorOperation>,
}

impl PreflightInput {
    /// Default selection and no prior records. A server with service-bearing
    /// packages is then classified ExistingUnmanaged (fails closed).
    pub fn without_records(observed_at: String) -> Self {
        Self {
            selection: DEFAULT_SELECTION.iter().map(|s| (*s).to_owned()).collect(),
            observed_at,
            prior_operations: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SudoProbe {
    pub mode: SudoMode,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listener {
    pub port: u16,
    /// `None` when `ss` did not show the owner.
    pub process: Option<String>,
}

/// Hosts of enabled sources and the entries that are not acceptable.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceScan {
    pub hosts: Vec<String>,
    pub rejected: Vec<String>,
}

/// Everything observed on the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    pub observed_at: String,
    /// (id, version), for example ("ubuntu", "24.04").
    pub os: Evidence<(String, String)>,
    pub arch: Evidence<String>,
    pub mem_total_mb: Evidence<u64>,
    pub free_disk_mb: Evidence<u64>,
    /// `id -u`; advisory.
    pub uid: Evidence<u32>,
    pub sudo: SudoProbe,
    /// State of every component package.
    pub packages: Evidence<BTreeMap<String, PackageState>>,
    /// Package name to its installation candidate version, `None` when it
    /// has none. Covers every component package.
    pub candidates: Evidence<BTreeMap<String, Option<String>>>,
    /// True when any package is half-installed, unpacked, half-configured,
    /// has triggers pending or awaited, or is flagged reinst-required
    /// (derived from `dpkg-query -W`; see `provision::dpkg_states_settled`).
    pub dpkg_broken: Evidence<bool>,
    pub apt_busy: Evidence<bool>,
    pub reboot_pending: Evidence<bool>,
    /// Entries of nginx and apache `sites-enabled`.
    pub sites: Evidence<Vec<String>>,
    /// Installed package names (inventory `software`).
    pub installed_software: Evidence<BTreeSet<String>>,
    /// Active service units (inventory `services`).
    pub active_units: Evidence<BTreeSet<String>>,
    pub listeners: Evidence<Vec<Listener>>,
    /// Lines for `ossiframe-op-*` and lock units. Advisory.
    pub foreign_units: Evidence<Vec<String>>,
    pub sources: Evidence<SourceScan>,
}

// ---------------------------------------------------------------------------
// Running commands
// ---------------------------------------------------------------------------

enum Ran {
    Done(ExecResult),
    Failed(String),
}

/// Cancel and lost connection abort the preflight. Every other failure is
/// returned as text so the dependent rule can block.
async fn ran(
    exec: &dyn Executor,
    cancel: &CancelToken,
    spec: Result<CommandSpec, ValidationError>,
) -> Result<Ran, OperationError> {
    let spec = match spec {
        Ok(s) => s,
        Err(e) => return Ok(Ran::Failed(e.to_string())),
    };
    match exec.run(&spec, cancel).await {
        Ok(r) => Ok(Ran::Done(r)),
        Err(ExecError::Timeout) => Ok(Ran::Failed(
            "timed out; the command may still be running on the server".to_owned(),
        )),
        Err(ExecError::Channel(m)) => Ok(Ran::Failed(sources::clean(&m))),
        Err(ExecError::PrivilegeDenied) => {
            Ok(Ran::Failed("privileged command was not allowed".to_owned()))
        }
        Err(e @ (ExecError::Cancelled | ExecError::ConnectionLost)) => Err(OperationError::Exec(e)),
    }
}

fn user(program: &str, args: &[&str]) -> Result<CommandSpec, ValidationError> {
    CommandSpec::new(program, args, PROBE_TIMEOUT)
}

fn first_stderr_line(r: &ExecResult) -> String {
    sources::clean(r.stderr.lines().next().unwrap_or(""))
}

/// Successful, complete stdout. `ok_codes` lists exit codes that count as
/// success (some probes use exit 1 for "none").
async fn stdout_of(
    exec: &dyn Executor,
    cancel: &CancelToken,
    spec: Result<CommandSpec, ValidationError>,
    ok_codes: &[i32],
) -> Result<Evidence<String>, OperationError> {
    Ok(match ran(exec, cancel, spec).await? {
        Ran::Failed(m) => Err(m),
        Ran::Done(r) => match r.exit {
            _ if r.truncated => Err("output was cut at the size limit".to_owned()),
            ExitStatus::Code(c) if ok_codes.contains(&c) => Ok(r.stdout),
            ref other => Err(format!("exit {other:?}: {}", first_stderr_line(&r))),
        },
    })
}

fn only_lines_containing(stderr: &str, needle: &str) -> bool {
    let mut any = false;
    for line in stderr.lines().filter(|l| !l.trim().is_empty()) {
        if !line.contains(needle) {
            return false;
        }
        any = true;
    }
    any
}

// ---------------------------------------------------------------------------
// Parsers
// ---------------------------------------------------------------------------

/// Classifies the result of `sudo -n -- true`. The exit code decides whether
/// sudo works; English text only picks the message. A non-zero exit is never
/// `Passwordless`.
pub fn classify_sudo(exit: &ExitStatus, stderr: &str) -> SudoMode {
    match exit {
        ExitStatus::Code(0) => SudoMode::Passwordless,
        ExitStatus::Code(126 | 127) => SudoMode::Unavailable,
        _ => {
            let s = stderr.to_ascii_lowercase();
            if s.contains("password is required") || s.contains("terminal is required") {
                SudoMode::PasswordRequired
            } else {
                // "not in the sudoers", "may not run sudo" and anything
                // unrecognized: sudo cannot be used.
                SudoMode::NotPermitted
            }
        }
    }
}

/// Parses `dpkg-query -W -f '${Package}\t${db:Status-Abbrev}\t${Version}\n'`.
/// Requested packages missing from the output are `Absent`. `Err` on a line
/// that is not three tab-separated fields.
pub fn parse_package_states(
    stdout: &str,
    requested: &[String],
) -> Result<BTreeMap<String, PackageState>, String> {
    let mut states: BTreeMap<String, PackageState> = requested
        .iter()
        .map(|p| (p.clone(), PackageState::Absent))
        .collect();
    for line in stdout.lines().filter(|l| !l.trim().is_empty()) {
        let fields: Vec<&str> = line.split('\t').collect();
        let [name, status, version] = fields[..] else {
            return Err(format!(
                "unrecognized package line: {}",
                sources::clean(line)
            ));
        };
        let Some(slot) = states.get_mut(name) else {
            continue;
        };
        let status = status.trim();
        *slot = match status {
            "ii" => PackageState::Installed {
                version: sources::clean(version),
            },
            "un" => PackageState::Absent,
            other => PackageState::Other {
                status: sources::clean(other),
            },
        };
    }
    Ok(states)
}

/// Parses `apt-cache policy <pkgs>` into the candidate version per package.
/// Packages without an entry have no candidate.
pub fn parse_candidates(stdout: &str, requested: &[String]) -> BTreeMap<String, Option<String>> {
    let mut out: BTreeMap<String, Option<String>> =
        requested.iter().map(|p| (p.clone(), None)).collect();
    let mut current: Option<String> = None;
    for line in stdout.lines() {
        if !line.starts_with(' ') {
            current = line.strip_suffix(':').map(str::to_owned);
        } else if let (Some(name), Some(c)) = (&current, line.trim().strip_prefix("Candidate:"))
            && let Some(slot) = out.get_mut(name)
        {
            let v = sources::clean(c.trim());
            *slot = (v != "(none)" && !v.is_empty()).then_some(v);
        }
    }
    out
}

/// True when a `sites-enabled` entry makes the server non-fresh: any Apache
/// site, any NGINX entry other than `default` and `ossiframe-*`, and any
/// entry in an unexpected directory.
pub fn is_foreign_site(path: &str) -> bool {
    if let Some(name) = path.strip_prefix("/etc/nginx/sites-enabled/") {
        !(name == "default" || name.starts_with("ossiframe-"))
    } else {
        true
    }
}

/// Process names of `listening` sockets on the managed ports.
fn listeners_of(info: &inventory::sockets::SocketsInfo) -> Vec<Listener> {
    let mut v: Vec<Listener> = info
        .sockets
        .iter()
        .filter(|s| MANAGED_PORTS.iter().any(|(p, _, _)| *p == s.port))
        .map(|s| Listener {
            port: s.port,
            process: s.process.as_ref().map(|p| p.name.clone()),
        })
        .collect();
    v.sort_by(|a, b| (a.port, &a.process).cmp(&(b.port, &b.process)));
    v
}

/// PHP minor versions other than 8.3 among installed `phpX.Y-fpm|cli`.
fn other_php_minors(installed: &BTreeSet<String>) -> Vec<String> {
    installed
        .iter()
        .filter(|n| {
            let Some(rest) = n.strip_prefix("php") else {
                return false;
            };
            let Some((ver, _)) = rest.split_once('-') else {
                return false;
            };
            let is_ver = ver.split_once('.').is_some_and(|(a, b)| {
                !a.is_empty()
                    && !b.is_empty()
                    && a.bytes().all(|c| c.is_ascii_digit())
                    && b.bytes().all(|c| c.is_ascii_digit())
            });
            is_ver && ver != "8.3"
        })
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// Collection
// ---------------------------------------------------------------------------

fn unavailable(what: &str, probes: &[inventory::ProbeError]) -> String {
    match probes.first() {
        Some(p) => format!("{what}: {}", sources::clean(&p.message)),
        None => format!("{what} not available"),
    }
}

async fn read_sources(
    exec: &dyn Executor,
    cancel: &CancelToken,
) -> Result<Evidence<SourceScan>, OperationError> {
    // (path, deb822?)
    let mut files: Vec<(String, bool)> = vec![(SOURCES_LIST.to_owned(), false)];
    let listing = match ran(
        exec,
        cancel,
        user(
            "/usr/bin/find",
            &[SOURCES_DIR, "-mindepth", "1", "-maxdepth", "1"],
        ),
    )
    .await?
    {
        Ran::Failed(m) => return Ok(Err(format!("source directory: {m}"))),
        Ran::Done(r) if r.truncated => {
            return Ok(Err("source directory listing was cut".to_owned()));
        }
        Ran::Done(r) => match r.exit {
            ExitStatus::Code(0) => r.stdout,
            ExitStatus::Code(1) if only_lines_containing(&r.stderr, "No such file") => {
                String::new()
            }
            ref other => {
                return Ok(Err(format!(
                    "source directory: exit {other:?}: {}",
                    first_stderr_line(&r)
                )));
            }
        },
    };
    for line in listing.lines().filter(|l| !l.trim().is_empty()) {
        let Some(name) = line.strip_prefix("/etc/apt/sources.list.d/") else {
            return Ok(Err(format!(
                "unexpected source file path: {}",
                sources::clean(line)
            )));
        };
        let safe = !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-+".contains(&b));
        if !safe {
            return Ok(Err(format!(
                "unusual source file name: {}",
                sources::clean(name)
            )));
        }
        // apt reads only these two suffixes.
        if name.ends_with(".list") {
            files.push((line.to_owned(), false));
        } else if name.ends_with(".sources") {
            files.push((line.to_owned(), true));
        }
    }
    if files.len() > MAX_SOURCE_FILES {
        return Ok(Err("too many source files to inspect".to_owned()));
    }
    let mut scan = SourceScan::default();
    for (path, deb822) in files {
        let text = match ran(exec, cancel, user("/usr/bin/cat", &[&path])).await? {
            Ran::Failed(m) => return Ok(Err(format!("{path}: {m}"))),
            Ran::Done(r) if r.truncated => return Ok(Err(format!("{path}: file was cut"))),
            Ran::Done(r) => match r.exit {
                ExitStatus::Code(0) => r.stdout,
                // Only the optional main file may be missing.
                ExitStatus::Code(1)
                    if path == SOURCES_LIST && r.stderr.contains("No such file") =>
                {
                    continue;
                }
                ref other => {
                    return Ok(Err(format!(
                        "{path}: exit {other:?}: {}",
                        first_stderr_line(&r)
                    )));
                }
            },
        };
        let parsed = if deb822 {
            sources::parse_deb822(&text)
        } else {
            sources::parse_list(&text)
        };
        match parsed {
            Ok(p) => {
                scan.hosts.extend(p.hosts);
                scan.rejected
                    .extend(p.rejected.into_iter().map(|r| format!("{path}: {r}")));
            }
            Err(e) => return Ok(Err(format!("{path}: {e}"))),
        }
    }
    scan.hosts.sort();
    scan.hosts.dedup();
    Ok(Ok(scan))
}

/// Collects all facts. Cancel and a lost connection return an error and no
/// facts.
pub async fn collect_facts(
    exec: &dyn Executor,
    cancel: &CancelToken,
    observed_at: String,
) -> Result<Facts, OperationError> {
    // Inventory facts.
    let info = inventory::collect(exec, cancel, observed_at.clone())
        .await
        .map_err(OperationError::Exec)?;
    let os = match info.os.as_ref().map(|o| (&o.id, &o.version_id)) {
        Some((Some(id), Some(v))) => Ok((id.clone(), v.clone())),
        _ => Err(unavailable("os-release", &info.probe_errors)),
    };
    let arch = info
        .arch
        .clone()
        .ok_or_else(|| unavailable("architecture", &info.probe_errors));
    let mem_total_mb = info
        .memory_total_kb
        .map(|kb| kb / 1024)
        .ok_or_else(|| unavailable("memory", &info.probe_errors));
    let free_disk_mb = info
        .root_fs
        .as_ref()
        .map(|d| d.available_bytes / (1024 * 1024))
        .ok_or_else(|| unavailable("root filesystem", &info.probe_errors));

    let sw = inventory::software::collect(exec, cancel, observed_at.clone())
        .await
        .map_err(OperationError::Exec)?;
    let installed_software = if sw.available && !sw.truncated {
        Ok(sw.packages.iter().map(|p| p.name.clone()).collect())
    } else {
        Err(if sw.truncated {
            "package list is incomplete".to_owned()
        } else {
            unavailable("package list", &sw.probe_errors)
        })
    };

    let svc = inventory::services::collect(exec, cancel, observed_at.clone())
        .await
        .map_err(OperationError::Exec)?;
    let active_units = if svc.available && !svc.truncated {
        Ok(svc
            .services
            .iter()
            .filter(|s| matches!(s.active.as_str(), "active" | "activating" | "reloading"))
            .map(|s| s.unit.clone())
            .collect())
    } else {
        Err(if svc.truncated {
            "service list is incomplete".to_owned()
        } else {
            unavailable("service list", &svc.probe_errors)
        })
    };

    let sock = inventory::sockets::collect(exec, cancel, observed_at.clone())
        .await
        .map_err(OperationError::Exec)?;
    let listeners = if sock.available && !sock.truncated && sock.unrecognized_lines == 0 {
        Ok(listeners_of(&sock))
    } else if sock.available {
        Err("socket list is incomplete or has unrecognized lines".to_owned())
    } else {
        Err(unavailable("socket list", &sock.probe_errors))
    };

    // Own probes.
    let uid = stdout_of(exec, cancel, user("/usr/bin/id", &["-u"]), &[0])
        .await?
        .and_then(|s| {
            s.trim()
                .parse::<u32>()
                .map_err(|_| "unrecognized output".to_owned())
        });

    let sudo = match ran(
        exec,
        cancel,
        CommandSpec::privileged("/usr/bin/true", &[], PROBE_TIMEOUT),
    )
    .await?
    {
        Ran::Done(r) => SudoProbe {
            mode: classify_sudo(&r.exit, &r.stderr),
            detail: first_stderr_line(&r),
        },
        Ran::Failed(m) => SudoProbe {
            mode: SudoMode::Unavailable,
            detail: m,
        },
    };

    let all_packages: Vec<String> = COMPONENTS
        .iter()
        .flat_map(|c| c.packages.iter().map(|p| (*p).to_owned()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut dpkg_args: Vec<&str> = vec![
        "-W",
        "-f",
        "${Package}\\t${db:Status-Abbrev}\\t${Version}\\n",
    ];
    dpkg_args.extend(all_packages.iter().map(String::as_str));
    let packages = match ran(exec, cancel, user("/usr/bin/dpkg-query", &dpkg_args)).await? {
        Ran::Failed(m) => Err(m),
        Ran::Done(r) if r.truncated => Err("output was cut at the size limit".to_owned()),
        Ran::Done(r) => match r.exit {
            ExitStatus::Code(0) => parse_package_states(&r.stdout, &all_packages),
            ExitStatus::Code(1)
                if only_lines_containing(&r.stderr, "dpkg-query: no packages found matching ") =>
            {
                parse_package_states(&r.stdout, &all_packages)
            }
            ref other => Err(format!("exit {other:?}: {}", first_stderr_line(&r))),
        },
    };

    // Candidate versions of every component package, for the missing-candidate
    // warning and for showing "update available". One read-only command.
    let candidates = if packages.is_err() {
        Ok(BTreeMap::new())
    } else {
        let mut args: Vec<&str> = vec!["policy"];
        args.extend(all_packages.iter().map(String::as_str));
        stdout_of(exec, cancel, user("/usr/bin/apt-cache", &args), &[0])
            .await?
            .map(|s| parse_candidates(&s, &all_packages))
    };

    // dpkg health without root: every package state from `dpkg-query -W`
    // (`dpkg --audit` needs root). Missing or unreadable output is missing
    // evidence, never "healthy".
    let dpkg_broken = match ran(
        exec,
        cancel,
        user(
            "/usr/bin/dpkg-query",
            &["-W", "-f", provision::DPKG_HEALTH_FORMAT],
        ),
    )
    .await?
    {
        Ran::Failed(m) => Err(m),
        Ran::Done(r) if r.truncated => Err("output was cut at the size limit".to_owned()),
        Ran::Done(r) if !r.exit.success() => {
            Err(format!("exit {:?}: {}", r.exit, first_stderr_line(&r)))
        }
        Ran::Done(r) => provision::dpkg_states_settled(&r.stdout).map(|settled| !settled),
    };

    let pgrep = stdout_of(
        exec,
        cancel,
        user(
            "/usr/bin/pgrep",
            &["-x", "-l", "apt|apt-get|dpkg|unattended-upgr"],
        ),
        &[0, 1],
    )
    .await?
    .map(|s| !s.trim().is_empty());
    let timers = stdout_of(
        exec,
        cancel,
        user(
            "/usr/bin/systemctl",
            &[
                "is-active",
                "apt-daily.service",
                "apt-daily-upgrade.service",
            ],
        ),
        &[0, 3],
    )
    .await?
    .and_then(|s| {
        if s.trim().is_empty() {
            Err("no output".to_owned())
        } else {
            Ok(s.lines()
                .any(|l| matches!(l.trim(), "active" | "activating" | "reloading")))
        }
    });
    let apt_busy = match (pgrep, timers) {
        (Ok(a), Ok(b)) => Ok(a || b),
        (Err(e), _) | (_, Err(e)) => Err(e),
    };

    let reboot_pending = match ran(
        exec,
        cancel,
        user("/usr/bin/test", &["-f", "/var/run/reboot-required"]),
    )
    .await?
    {
        Ran::Failed(m) => Err(m),
        Ran::Done(r) => match r.exit {
            ExitStatus::Code(0) => Ok(true),
            ExitStatus::Code(1) => Ok(false),
            other => Err(format!("exit {other:?}")),
        },
    };

    let sites = match ran(
        exec,
        cancel,
        user(
            "/usr/bin/find",
            &[
                "/etc/nginx/sites-enabled",
                "/etc/apache2/sites-enabled",
                "-maxdepth",
                "1",
                "-mindepth",
                "1",
            ],
        ),
    )
    .await?
    {
        Ran::Failed(m) => Err(m),
        Ran::Done(r) if r.truncated => Err("output was cut at the size limit".to_owned()),
        Ran::Done(r) => match r.exit {
            // A missing directory (no nginx or apache) is the normal case.
            ExitStatus::Code(0) => Ok(r.stdout),
            ExitStatus::Code(1) if only_lines_containing(&r.stderr, "No such file") => Ok(r.stdout),
            ref other => Err(format!("exit {other:?}: {}", first_stderr_line(&r))),
        },
    }
    .and_then(|s| {
        let v: Vec<String> = s
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| sources::clean(l.trim()))
            .collect();
        if v.len() > MAX_LIST_LINES {
            Err("too many site entries to inspect".to_owned())
        } else {
            Ok(v)
        }
    });

    let foreign_units = stdout_of(
        exec,
        cancel,
        user(
            "/usr/bin/systemctl",
            &[
                "list-units",
                "--all",
                "--plain",
                "--no-legend",
                "ossiframe-op-*",
                "ossiframe-provision-lock*",
            ],
        ),
        &[0],
    )
    .await?
    .map(|s| {
        s.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| sources::clean(l.trim()))
            .collect()
    });

    let sources = read_sources(exec, cancel).await?;

    Ok(Facts {
        observed_at,
        os,
        arch,
        mem_total_mb,
        free_disk_mb,
        uid,
        sudo,
        packages,
        candidates,
        dpkg_broken,
        apt_busy,
        reboot_pending,
        sites,
        installed_software,
        active_units,
        listeners,
        foreign_units,
        sources,
    })
}

// ---------------------------------------------------------------------------
// Evaluation
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Conflicts(Vec<Conflict>);

impl Conflicts {
    fn push(&mut self, severity: Severity, id: impl Into<String>, message: impl Into<String>) {
        self.0.push(Conflict {
            id: id.into(),
            severity,
            message: message.into(),
        });
    }
    fn block(&mut self, id: impl Into<String>, message: impl Into<String>) {
        self.push(Severity::Blocker, id, message);
    }
    fn warn(&mut self, id: impl Into<String>, message: impl Into<String>) {
        self.push(Severity::Warning, id, message);
    }
}

fn service_packages() -> Vec<&'static str> {
    COMPONENTS
        .iter()
        .filter(|c| !c.units.is_empty())
        .flat_map(|c| c.packages.iter().copied())
        .collect()
}

/// True when a listener on `comp`'s port is explained by the component itself:
/// every package of the component is installed and every one of its systemd
/// units is active. Without root, `ss` cannot show the owner of a socket held
/// by another user (nginx, mysqld and redis run under their own users), so
/// the active unit of the installed component is the evidence. The caller
/// also requires a verified Ossiframe operation for this server. Missing
/// evidence is never an explanation.
fn component_explains_listener(facts: &Facts, comp: &str) -> bool {
    let Some(def) = components::find(comp) else {
        return false;
    };
    let (Ok(packages), Ok(units)) = (&facts.packages, &facts.active_units) else {
        return false;
    };
    !def.units.is_empty()
        && def
            .packages
            .iter()
            .all(|p| matches!(packages.get(*p), Some(PackageState::Installed { .. })))
        && def
            .units
            .iter()
            .all(|u| units.contains(&format!("{u}.service")))
}

/// Pure rules. `prior` is the caller's evidence for this identity snapshot.
pub fn evaluate(
    facts: &Facts,
    selected: &[&'static ComponentDef],
    prior: &[PriorOperation],
) -> PreflightReport {
    let mut c = Conflicts::default();
    let selected_ids: BTreeSet<&str> = selected.iter().map(|d| d.id).collect();
    let mysql_selected = selected_ids.contains("mysql");

    // OS and architecture.
    match &facts.os {
        Err(e) => c.block("os-unavailable", format!("Operating system unknown: {e}")),
        Ok((id, version)) if id == "ubuntu" => match version.as_str() {
            "24.04" => {}
            "26.04" => c.block(
                "os-pending-verification",
                "Ubuntu 26.04 is pending verification and is not supported yet.",
            ),
            other => c.block(
                "os-unsupported",
                format!(
                    "Ubuntu {} is not supported. Supported: 24.04.",
                    sources::clean(other)
                ),
            ),
        },
        Ok((id, _)) => c.block(
            "os-unsupported",
            format!(
                "{} is not supported. Only Ubuntu 24.04 is supported.",
                sources::clean(id)
            ),
        ),
    }
    match &facts.arch {
        Err(e) => c.block("arch-unavailable", format!("Architecture unknown: {e}")),
        Ok(a) if a == "x86_64" || a == "aarch64" => {}
        Ok(a) => c.block(
            "arch-unsupported",
            format!(
                "Architecture {} is not supported (x86_64 and aarch64 only).",
                sources::clean(a)
            ),
        ),
    }
    if let Err(e) = &facts.uid {
        c.warn("uid-unavailable", format!("User id not determined: {e}"));
    }

    // Sudo.
    match facts.sudo.mode {
        SudoMode::Passwordless => {}
        SudoMode::PasswordRequired => c.block(
            "sudo-password-required",
            "sudo needs a password. Ossiframe uses only passwordless sudo (sudo -n). \
             Configure NOPASSWD for this user, knowing it is root-equivalent.",
        ),
        SudoMode::NotPermitted => c.block(
            "sudo-not-permitted",
            "This user may not run sudo. Use a user that has passwordless sudo.",
        ),
        SudoMode::Unavailable => c.block(
            "sudo-unavailable",
            "sudo is not available or could not be run on this server.",
        ),
    }

    // Packages.
    let packages_ok = facts.packages.as_ref().ok();
    let installed = |name: &str| {
        packages_ok.is_some_and(|m| matches!(m.get(name), Some(PackageState::Installed { .. })))
    };
    match &facts.packages {
        Err(e) => c.block(
            "package-evidence",
            format!("Package states could not be read: {e}"),
        ),
        Ok(states) => {
            for pkg in selected.iter().flat_map(|d| d.packages.iter()) {
                if let Some(PackageState::Other { status }) = states.get(*pkg) {
                    c.block(
                        format!("package-state-{pkg}"),
                        format!("Package {pkg} is in dpkg state '{status}'. Repair it first."),
                    );
                }
            }
        }
    }
    match &facts.dpkg_broken {
        Err(e) => c.block(
            "dpkg-evidence",
            format!("dpkg health could not be checked: {e}"),
        ),
        Ok(true) => c.block(
            "dpkg-broken",
            "dpkg reports packages in an interrupted or unsettled state (half-installed, \
             unpacked, half-configured, triggers pending or reinstall required). Inspect them \
             on the server first; Ossiframe does not run dpkg --configure -a.",
        ),
        Ok(false) => {}
    }

    // Resources.
    match &facts.mem_total_mb {
        Err(e) => {
            if mysql_selected {
                c.block(
                    "memory-unavailable",
                    format!("Memory unknown, required for MySQL: {e}"),
                );
            }
        }
        Ok(mb) if mysql_selected && *mb < MIN_RAM_MYSQL_MB => c.block(
            "memory-low",
            format!("MySQL needs at least {MIN_RAM_MYSQL_MB} MB RAM; this server has {mb} MB."),
        ),
        Ok(mb) if mysql_selected && *mb < WARN_RAM_MYSQL_MB => c.warn(
            "memory-tight",
            format!("This server has {mb} MB RAM. MySQL may be unstable below 1.8 GB."),
        ),
        Ok(_) => {}
    }
    let need_mb: u64 = selected
        .iter()
        .filter(|d| packages_ok.is_none_or(|_| d.packages.iter().any(|p| !installed(p))))
        .map(|d| u64::from(d.est_disk_mb))
        .sum::<u64>()
        + DISK_HEADROOM_MB;
    match &facts.free_disk_mb {
        Err(e) => c.block("disk-unavailable", format!("Free disk space unknown: {e}")),
        Ok(mb) if *mb < need_mb => c.block(
            "disk-low",
            format!("Free disk is {mb} MB; at least {need_mb} MB is needed."),
        ),
        Ok(_) => {}
    }

    // Foreign software.
    match &facts.installed_software {
        Err(e) => c.block(
            "software-evidence",
            format!("Installed software could not be listed: {e}"),
        ),
        Ok(set) => {
            for name in FOREIGN_PACKAGES.iter().filter(|n| set.contains(**n)) {
                c.block(
                    format!("foreign-{name}"),
                    format!(
                        "{name} is installed. Ossiframe does not take over existing infrastructure."
                    ),
                );
            }
            let minors = other_php_minors(set);
            if !minors.is_empty() {
                c.block(
                    "php-other-version",
                    format!("Another PHP version is installed ({}).", minors.join(", ")),
                );
            }
        }
    }
    match &facts.active_units {
        Err(e) => c.block(
            "services-evidence",
            format!("Service list could not be read: {e}"),
        ),
        Ok(units) => {
            for u in FOREIGN_UNITS.iter().filter(|u| units.contains(**u)) {
                c.block(
                    format!("foreign-active-{u}"),
                    format!("{u} is active. Ossiframe does not take over existing infrastructure."),
                );
            }
        }
    }

    // Web configuration.
    let foreign_sites: Vec<&String> = facts
        .sites
        .as_ref()
        .map(|v| v.iter().filter(|p| is_foreign_site(p)).collect())
        .unwrap_or_default();
    match &facts.sites {
        Err(e) => c.block(
            "sites-evidence",
            format!("Web server sites could not be listed: {e}"),
        ),
        Ok(_) if !foreign_sites.is_empty() => c.block(
            "foreign-sites",
            format!(
                "Existing web server sites found: {}.",
                foreign_sites
                    .iter()
                    .take(MAX_UNITS_SHOWN)
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ),
        Ok(_) => {}
    }

    // Listeners. Unknown ownership is never counted as owned by itself. A
    // listener counts as owned only when its component is selected and `ss`
    // names the expected process, or when the component is already installed
    // by Ossiframe (see `component_explains_listener`).
    let managed = prior.iter().any(|p| {
        matches!(
            p,
            PriorOperation::Succeeded {
                all_essential_passed: true
            }
        )
    });
    match &facts.listeners {
        Err(e) => c.block(
            "listeners-evidence",
            format!("Listening sockets could not be fully read: {e}"),
        ),
        Ok(list) => {
            let mut reported = BTreeSet::new();
            for l in list {
                let Some((_, comp, proc_name)) =
                    MANAGED_PORTS.iter().find(|(p, _, _)| *p == l.port)
                else {
                    continue;
                };
                let explained = managed && component_explains_listener(facts, comp);
                let named = l.process.as_deref() == Some(proc_name);
                let owned = match &l.process {
                    Some(_) => named && (selected_ids.contains(comp) || explained),
                    None => explained,
                };
                if !owned && reported.insert(l.port) {
                    let who = match &l.process {
                        Some(p) => format!("process '{}'", sources::clean(p)),
                        None => "a process that could not be identified".to_owned(),
                    };
                    c.block(
                        format!("listener-{}", l.port),
                        format!(
                            "Port {} is in use by {who}, which is not a component Ossiframe \
                             installed or is installing.",
                            l.port
                        ),
                    );
                }
            }
        }
    }

    // Foreign operations (advisory; the remote lock is authoritative).
    match &facts.foreign_units {
        Err(e) => c.warn(
            "operation-units-unchecked",
            format!("Operation units could not be scanned: {e}. The remote lock still applies."),
        ),
        Ok(lines) if !lines.is_empty() => c.block(
            "foreign-operation",
            format!(
                "Ossiframe operation or lock units exist on the server: {}",
                lines
                    .iter()
                    .take(MAX_UNITS_SHOWN)
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        ),
        Ok(_) => {}
    }

    // Package sources.
    match &facts.sources {
        Err(e) => c.block(
            "sources-evidence",
            format!("Package sources could not be read or parsed: {e}"),
        ),
        Ok(scan) => {
            let mut bad: Vec<String> = scan.rejected.clone();
            bad.extend(
                scan.hosts
                    .iter()
                    .filter(|h| !sources::host_allowed(h))
                    .map(|h| format!("host not on the allowlist: {}", sources::clean(h))),
            );
            bad.sort();
            bad.dedup();
            if !bad.is_empty() {
                c.block(
                    "sources-not-allowed",
                    format!(
                        "Only the official Ubuntu archive is allowed as a package source. \
                         Found: {}. Ossiframe does not change sources.",
                        bad.iter()
                            .take(MAX_UNITS_SHOWN)
                            .map(String::as_str)
                            .collect::<Vec<_>>()
                            .join("; ")
                    ),
                );
            }
        }
    }

    // Warnings.
    let selected_missing: BTreeSet<&str> = selected
        .iter()
        .flat_map(|d| d.packages.iter().copied())
        .filter(|p| !installed(p))
        .collect();
    match &facts.apt_busy {
        Ok(true) => c.warn(
            "apt-busy",
            "apt or dpkg appears to be running. The install waits for the lock.",
        ),
        Err(e) => c.warn(
            "apt-busy-unknown",
            format!("apt activity could not be checked: {e}"),
        ),
        Ok(false) => {}
    }
    if facts.reboot_pending == Ok(true) {
        c.warn("reboot-pending", "A reboot is pending on this server.");
    }
    match &facts.candidates {
        Ok(map) => {
            // Only packages that would be installed need a candidate.
            for pkg in map
                .iter()
                .filter(|(p, v)| v.is_none() && selected_missing.contains(p.as_str()))
                .map(|(p, _)| p)
            {
                c.warn(
                    format!("candidate-missing-{pkg}"),
                    format!("No installation candidate for {pkg} yet; package lists may be stale until apt-get update runs."),
                );
            }
        }
        Err(e) => c.warn(
            "candidates-unknown",
            format!("Package candidates could not be checked: {e}"),
        ),
    }

    // Classification.
    let svc_pkgs = service_packages();
    let present_services: Vec<&str> = svc_pkgs.iter().copied().filter(|p| installed(p)).collect();
    let mut signals: Vec<String> = present_services.iter().map(|p| (*p).to_owned()).collect();
    if let Ok(set) = &facts.installed_software {
        signals.extend(
            FOREIGN_PACKAGES
                .iter()
                .filter(|n| set.contains(**n))
                .map(|n| (*n).to_owned()),
        );
        signals.extend(other_php_minors(set));
    }
    signals.extend(foreign_sites.iter().map(|s| (*s).clone()));
    if let Ok(list) = &facts.listeners {
        signals.extend(list.iter().map(|l| format!("listener on port {}", l.port)));
    }
    signals.sort();
    signals.dedup();
    let incomplete = facts.packages.is_err()
        || facts.sites.is_err()
        || facts.listeners.is_err()
        || facts.installed_software.is_err()
        || facts.active_units.is_err();
    let class = if managed {
        ServerClass::OssiframeManaged
    } else if incomplete {
        ServerClass::ExistingUnmanaged
    } else if !prior.is_empty() && !present_services.is_empty() {
        ServerClass::Partial
    } else if !signals.is_empty() {
        ServerClass::ExistingUnmanaged
    } else {
        ServerClass::Fresh
    };
    match class {
        ServerClass::ExistingUnmanaged if incomplete => c.block(
            "existing-unmanaged",
            "The server state could not be fully determined, so it is not treated as fresh.",
        ),
        ServerClass::ExistingUnmanaged => c.block(
            "existing-unmanaged",
            format!(
                "Existing setup without an Ossiframe record: {}. Use Doctor to inspect it; \
                 adoption is not supported.",
                signals
                    .iter()
                    .take(MAX_UNITS_SHOWN)
                    .map(|s| sources::clean(s))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ),
        ServerClass::Partial => c.warn(
            "partial-previous-operation",
            "An earlier operation on this server did not complete cleanly. \
             Installed packages are skipped.",
        ),
        ServerClass::Fresh | ServerClass::OssiframeManaged => {}
    }

    PreflightReport {
        observed_at: facts.observed_at.clone(),
        os: match &facts.os {
            Ok((id, v)) => format!("{} {}", sources::clean(id), sources::clean(v)),
            Err(_) => "unknown".to_owned(),
        },
        sudo: facts.sudo.mode,
        server_class: class,
        packages: facts.packages.clone().unwrap_or_default(),
        total_mem_mb: facts.mem_total_mb.clone().ok(),
        free_disk_mb: facts.free_disk_mb.clone().ok(),
        package_sources: facts
            .sources
            .as_ref()
            .map(|s| s.hosts.clone())
            .unwrap_or_default(),
        sites: facts
            .sites
            .as_ref()
            .map(|v| {
                let mut v = v.clone();
                v.sort();
                v
            })
            .unwrap_or_default(),
        candidate_versions: facts
            .candidates
            .as_ref()
            .map(|m| {
                m.iter()
                    .filter_map(|(p, v)| v.clone().map(|v| (p.clone(), v)))
                    .collect()
            })
            .unwrap_or_default(),
        conflicts: c.0,
    }
}

/// Collects facts and evaluates them. Returns the facts too, so the plan
/// builder can reuse them. `Err` for an unknown component id, cancel or a
/// lost connection.
pub async fn preflight_with(
    exec: &dyn Executor,
    cancel: &CancelToken,
    input: &PreflightInput,
) -> Result<(Facts, PreflightReport), OperationError> {
    let selected = components::resolve(&input.selection).map_err(|e| match e {
        PlanError::UnknownComponent(id) => {
            OperationError::InvalidArguments(format!("unknown component: {}", sources::clean(&id)))
        }
        other => OperationError::InvalidArguments(other.to_string()),
    })?;
    let facts = collect_facts(exec, cancel, input.observed_at.clone()).await?;
    let report = evaluate(&facts, &selected, &input.prior_operations);
    Ok((facts, report))
}

/// RFC 3339 UTC timestamp for seconds since the Unix epoch.
pub fn rfc3339_utc(secs: u64) -> String {
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
#[path = "preflight_tests.rs"]
pub(crate) mod tests;
