//! Rules for the `resources`, `services` and `network` steps. Every rule is a
//! pure function of already-collected inventory data. Missing data is never a
//! pass, and a truncated list is never reported as complete.
use crate::model::{Category, CheckResult, Outcome, Severity};
use crate::sshd::{Gap, Setting, SshdConfig, evidence_line};
use ossiframe_mod_linux_inventory::ServerInfo;
use ossiframe_mod_linux_inventory::services::ServicesInfo;
use ossiframe_mod_linux_inventory::sockets::{BindingClass, ListeningSocket, SocketsInfo};

/// Memory: available below this share of total is `medium`.
pub const MEM_MEDIUM_BELOW_PERCENT: u128 = 10;
/// Memory: available below this share of total is `high`.
pub const MEM_HIGH_BELOW_PERCENT: u128 = 5;
/// Disk: used share at or above these values gives the severity.
pub const DISK_MEDIUM_PERCENT: u128 = 80;
pub const DISK_HIGH_PERCENT: u128 = 90;
pub const DISK_CRITICAL_PERCENT: u128 = 95;
/// Disk: less available space than this is `high`.
pub const DISK_MIN_AVAILABLE_BYTES: u64 = 100 * 1024 * 1024;
/// Disk: the absolute available-space rule applies only to filesystems with
/// at least this total size. Percentage thresholds apply to all.
pub const DISK_MIN_SIZE_FOR_ABSOLUTE_RULE: u64 = 1024 * 1024 * 1024;
/// Most `svc.failed` findings per scan.
pub const MAX_FAILED_FINDINGS: usize = 20;
/// Ports treated as potential database listeners.
pub const DATABASE_PORTS: [u16; 6] = [3306, 5432, 6379, 27017, 11211, 9200];

const SNAPSHOT_NOTE: &str =
    "This is a single snapshot, not evidence of sustained memory exhaustion.";

fn rank(s: Severity) -> u8 {
    match s {
        Severity::Info => 0,
        Severity::Low => 1,
        Severity::Medium => 2,
        Severity::High => 3,
        Severity::Critical => 4,
    }
}

fn pct_floor(part: u64, whole: u64) -> u64 {
    if whole == 0 {
        return 0;
    }
    (u128::from(part) * 100 / u128::from(whole)) as u64
}

/// `mem.low-available`: `MemAvailable` below 10% of total is medium, below 5%
/// is high. Swap is evidence only.
pub fn mem_low_available(info: &ServerInfo, observed_at: &str) -> CheckResult {
    let title = "Available memory";
    let (Some(total), Some(avail)) = (info.memory_total_kb, info.memory_available_kb) else {
        return CheckResult::new(
            "mem.low-available",
            Category::Resources,
            Outcome::Unavailable,
            title,
            "Not assessed: memory totals could not be read.",
            observed_at,
        );
    };
    if total == 0 {
        return CheckResult::new(
            "mem.low-available",
            Category::Resources,
            Outcome::Unavailable,
            title,
            "Not assessed: total memory was reported as zero.",
            observed_at,
        );
    }
    let (t, a) = (u128::from(total), u128::from(avail));
    let severity = if a * 100 < t * MEM_HIGH_BELOW_PERCENT {
        Some(Severity::High)
    } else if a * 100 < t * MEM_MEDIUM_BELOW_PERCENT {
        Some(Severity::Medium)
    } else {
        None
    };
    let mut evidence = vec![format!(
        "MemAvailable {avail} kB of MemTotal {total} kB ({}%)",
        pct_floor(avail, total)
    )];
    if let (Some(st), Some(sf)) = (info.swap_total_kb, info.swap_free_kb) {
        evidence.push(format!("Swap free {sf} kB of {st} kB"));
    }
    let check = match severity {
        Some(sev) => CheckResult::finding(
            "mem.low-available",
            Category::Resources,
            sev,
            "Low available memory",
            format!(
                "Available memory was {}% of total when observed. {SNAPSHOT_NOTE}",
                pct_floor(avail, total)
            ),
            observed_at,
        )
        .recommendation(
            "Check which processes use memory and whether the server needs more RAM or swap.",
        ),
        None => CheckResult::new(
            "mem.low-available",
            Category::Resources,
            Outcome::Passed,
            title,
            format!(
                "Available memory was {}% of total when observed. {SNAPSHOT_NOTE}",
                pct_floor(avail, total)
            ),
            observed_at,
        ),
    };
    check
        .evidence(evidence)
        .limitation("Memory use changes over time; this is one observation.")
}

/// `disk.low-space`: one result per meaningful filesystem with the highest
/// applicable severity. A truncated filesystem list adds an `incomplete`
/// result.
pub fn disk_low_space(info: &ServerInfo, observed_at: &str) -> Vec<CheckResult> {
    if info.filesystems.is_empty() {
        return vec![CheckResult::new(
            "disk.low-space",
            Category::Resources,
            Outcome::Unavailable,
            "Disk space",
            "Not assessed: no filesystem data could be read.",
            observed_at,
        )];
    }
    let mut out: Vec<CheckResult> = info
        .filesystems
        .iter()
        .map(|fs| {
            let denom = fs.used_bytes.saturating_add(fs.available_bytes);
            let title = "Disk space";
            if denom == 0 {
                return CheckResult::new(
                    "disk.low-space",
                    Category::Resources,
                    Outcome::Unavailable,
                    title,
                    "Not assessed: the filesystem reported no usable size.",
                    observed_at,
                )
                .resource(&fs.mount);
            }
            let (u, d) = (u128::from(fs.used_bytes), u128::from(denom));
            let by_use = if u * 100 >= d * DISK_CRITICAL_PERCENT {
                Some(Severity::Critical)
            } else if u * 100 >= d * DISK_HIGH_PERCENT {
                Some(Severity::High)
            } else if u * 100 >= d * DISK_MEDIUM_PERCENT {
                Some(Severity::Medium)
            } else {
                None
            };
            // The absolute rule only applies to filesystems large enough for
            // 100 MiB to be a meaningful margin (not e.g. a 105 MB /boot/efi).
            let by_free = (fs.total_bytes >= DISK_MIN_SIZE_FOR_ABSOLUTE_RULE
                && fs.available_bytes < DISK_MIN_AVAILABLE_BYTES)
                .then_some(Severity::High);
            let severity = [by_use, by_free]
                .into_iter()
                .flatten()
                .max_by_key(|s| rank(*s));
            let used_pct = pct_floor(fs.used_bytes, denom);
            let evidence = format!(
                "{} used {used_pct}%, {} MiB available",
                fs.mount,
                fs.available_bytes / (1024 * 1024)
            );
            let check = match severity {
                Some(sev) => CheckResult::finding(
                    "disk.low-space",
                    Category::Resources,
                    sev,
                    "Low disk space",
                    format!(
                        "Filesystem {} is {used_pct}% used with {} MiB available.",
                        fs.mount,
                        fs.available_bytes / (1024 * 1024)
                    ),
                    observed_at,
                )
                .recommendation("Free space or extend the filesystem before it fills up."),
                None => CheckResult::new(
                    "disk.low-space",
                    Category::Resources,
                    Outcome::Passed,
                    title,
                    format!("Filesystem {} is {used_pct}% used.", fs.mount),
                    observed_at,
                ),
            };
            check
                .resource(&fs.mount)
                .evidence([evidence])
                .limitation("Used share is used / (used + available), as df reports it.")
        })
        .collect();
    if info.filesystems_truncated {
        out.push(
            CheckResult::new(
                "disk.low-space",
                Category::Resources,
                Outcome::Incomplete,
                "Disk space",
                "The filesystem list is truncated, so other filesystems were not assessed.",
                observed_at,
            )
            .limitation("Only the first filesystems are listed."),
        );
    }
    out
}

/// `svc.failed`: one finding per unit with active state `failed` (at most 20).
/// Passed only on a complete list with no failed unit.
pub fn svc_failed(info: &ServicesInfo, observed_at: &str) -> Vec<CheckResult> {
    let title = "Failed services";
    if !info.available {
        return vec![CheckResult::new(
            "svc.failed",
            Category::Services,
            Outcome::Unavailable,
            title,
            "Not assessed: the systemd service list could not be read.",
            observed_at,
        )];
    }
    let failed: Vec<_> = info
        .services
        .iter()
        .filter(|s| s.active == "failed")
        .collect();
    let mut out: Vec<CheckResult> = failed
        .iter()
        .take(MAX_FAILED_FINDINGS)
        .map(|s| {
            CheckResult::finding(
                "svc.failed",
                Category::Services,
                Severity::Medium,
                "Failed service",
                format!("Unit {} is in the failed state.", s.unit),
                observed_at,
            )
            .resource(&s.unit)
            .evidence([format!(
                "{} load={} active={} sub={} enablement={}",
                s.unit, s.load, s.active, s.sub, s.enablement
            )])
            .recommendation("Check the unit's status and logs to find out why it failed.")
        })
        .collect();
    if failed.len() > MAX_FAILED_FINDINGS {
        let more = failed.len() - MAX_FAILED_FINDINGS;
        if let Some(last) = out.last_mut() {
            last.limitations.push(format!(
                "{more} more failed unit(s) are not listed (limit {MAX_FAILED_FINDINGS})."
            ));
        }
    }
    if info.truncated {
        out.push(
            CheckResult::new(
                "svc.failed",
                Category::Services,
                Outcome::Incomplete,
                title,
                "The service list is truncated, so units that were not listed may have failed.",
                observed_at,
            )
            .limitation("Only part of the service list was read."),
        );
    } else if failed.is_empty() {
        out.push(CheckResult::new(
            "svc.failed",
            Category::Services,
            Outcome::Passed,
            title,
            "No service unit was in the failed state in the complete list.",
            observed_at,
        ));
    }
    out
}

fn expected_process(port: u16, name: &str) -> bool {
    let names: &[&str] = match port {
        3306 => &["mysqld", "mariadbd", "mariadb"],
        5432 => &["postgres", "postmaster"],
        6379 => &["redis-server"],
        27017 => &["mongod"],
        11211 => &["memcached"],
        9200 => &["elasticsearch", "opensearch"],
        _ => &[],
    };
    names.contains(&name)
}

fn db_severity(s: &ListeningSocket) -> Option<Severity> {
    match s.binding {
        BindingClass::Loopback => None,
        BindingClass::Wildcard => Some(Severity::Medium),
        BindingClass::Specific if s.private => Some(Severity::Low),
        BindingClass::Specific => Some(Severity::Medium),
    }
}

/// `net.database-exposed`: a potential database port on a non-loopback
/// address. Never `high`, never says "publicly reachable".
pub fn net_database_exposed(info: &SocketsInfo, observed_at: &str) -> Vec<CheckResult> {
    let title = "Database ports";
    if !info.available {
        return vec![CheckResult::new(
            "net.database-exposed",
            Category::Network,
            Outcome::Unavailable,
            title,
            "Not assessed: the listening socket list could not be read.",
            observed_at,
        )];
    }
    let mut out = Vec::new();
    for s in info
        .sockets
        .iter()
        .filter(|s| DATABASE_PORTS.contains(&s.port))
    {
        let Some(sev) = db_severity(s) else { continue };
        let confirmed = s
            .process
            .as_ref()
            .is_some_and(|p| expected_process(s.port, &p.name));
        let process = s.process.as_ref().map_or_else(
            || "process not visible".to_owned(),
            |p| format!("process {}", p.name),
        );
        let proto = format!("{:?}", s.protocol).to_ascii_lowercase();
        let mut check = CheckResult::finding(
            "net.database-exposed",
            Category::Network,
            sev,
            "Potential database port on a non-loopback address",
            format!(
                "Port {} ({proto}) listens on a non-loopback address ({}). \
                 Firewalls and cloud rules were not inspected, so this does not show \
                 whether it can be reached from other networks.",
                s.port, s.address
            ),
            observed_at,
        )
        .resource(format!("{}:{}", s.address, s.port))
        .evidence([format!("{proto} {}:{} ({process})", s.address, s.port)])
        .recommendation(
            "If remote access is not needed, bind the service to 127.0.0.1 or restrict the port in the firewall.",
        );
        if !confirmed {
            check = check
                .inferred()
                .limitation("The service is inferred from the port number.");
        }
        out.push(check);
    }
    if info.truncated || info.unrecognized_lines > 0 {
        let (summary, limitation) = if info.truncated {
            (
                "The socket list is truncated, so other listeners were not assessed.".to_owned(),
                "Only part of the socket list was read.".to_owned(),
            )
        } else {
            (
                format!(
                    "{} socket line(s) were not recognized, so those listeners were not assessed.",
                    info.unrecognized_lines
                ),
                "Some socket output lines could not be read.".to_owned(),
            )
        };
        out.push(
            CheckResult::new(
                "net.database-exposed",
                Category::Network,
                Outcome::Incomplete,
                title,
                summary,
                observed_at,
            )
            .limitation(limitation),
        );
    } else if out.is_empty() {
        let mut passed = CheckResult::new(
            "net.database-exposed",
            Category::Network,
            Outcome::Passed,
            title,
            "No known database port listens on a non-loopback address.",
            observed_at,
        )
        .limitation("Only the listed ports were checked; firewalls were not inspected.");
        for l in &info.limitations {
            passed = passed.limitation(l.clone());
        }
        out.push(passed);
    }
    out
}

const SSH_GLOBAL_NOTE: &str = "This describes the global configuration on disk, not per-user \
     settings and not the running daemon.";

/// What one SSH setting value means.
#[derive(Clone, Copy)]
enum Judgement {
    Finding(Severity, &'static str),
    /// Passed, with a restriction note for the limitation list (may be empty).
    Pass(&'static str),
}

fn judge_password(value: &str) -> Option<Judgement> {
    match value {
        "yes" => Some(Judgement::Finding(
            Severity::Medium,
            "PasswordAuthentication is yes in the global configuration.",
        )),
        "no" => Some(Judgement::Pass(
            "Global setting only; per-user and Match settings not assessed.",
        )),
        _ => None,
    }
}

fn judge_root(value: &str) -> Option<Judgement> {
    match value {
        "yes" => Some(Judgement::Finding(
            Severity::High,
            "PermitRootLogin is yes in the global configuration.",
        )),
        "no" => Some(Judgement::Pass("")),
        "prohibit-password" | "without-password" => {
            Some(Judgement::Pass("Root login allowed with public key only."))
        }
        "forced-commands-only" => Some(Judgement::Pass(
            "Root login allowed for forced commands only.",
        )),
        _ => None,
    }
}

struct SshRule {
    id: &'static str,
    title: &'static str,
    keyword: &'static str,
    judge: fn(&str) -> Option<Judgement>,
    /// Value OpenSSH uses when the directive is absent from fully read files.
    default: (&'static str, Judgement),
    recommendation: &'static str,
}

fn ssh_check(
    rule: &SshRule,
    setting: Option<&Setting>,
    cfg: &SshdConfig,
    observed_at: &str,
) -> CheckResult {
    let plain = |outcome: Outcome, why: String| {
        CheckResult::new(
            rule.id,
            Category::Security,
            outcome,
            rule.title,
            why,
            observed_at,
        )
    };
    // The outcome before limits: a judged value, or why there is none.
    let (check, passed_note): (CheckResult, Option<&str>) = match setting {
        Some(s) => match (rule.judge)(&s.value.to_ascii_lowercase()) {
            Some(j) => (judged(rule, j, observed_at), None),
            None => (
                plain(
                    Outcome::Error,
                    format!(
                        "{} has a value that this reader does not recognize.",
                        rule.keyword
                    ),
                ),
                None,
            ),
        },
        None => match &cfg.gap {
            Some(Gap::Unreadable) => (
                plain(
                    Outcome::Unavailable,
                    "Not assessed: an SSH configuration file could not be read.".to_owned(),
                ),
                None,
            ),
            Some(Gap::Skipped) => (
                plain(
                    Outcome::Incomplete,
                    "Not fully assessed: part of the SSH configuration was not read.".to_owned(),
                ),
                None,
            ),
            Some(Gap::Failed(m)) => (plain(Outcome::Error, format!("Not assessed: {m}.")), None),
            None => {
                let j = rule.default.1;
                (judged(rule, j, observed_at), Some(rule.default.0))
            }
        },
    };
    let mut check = check.resource(rule.keyword);
    // Never a pass when part of the configuration was not assessed.
    if check.outcome == Outcome::Passed && (cfg.match_blocks || cfg.gap.is_some()) {
        check.outcome = Outcome::Incomplete;
        check.explanation = "Not fully assessed: Match blocks or unread configuration may \
             change the effective value. Only the global value is known."
            .to_owned();
    }
    if let Some(d) = passed_note {
        check = check.limitation(format!(
            "{} is not set; using the OpenSSH default {d}.",
            rule.keyword
        ));
    }
    check = check.evidence([evidence_line(rule.keyword, setting)]);
    if let Some(s) = setting {
        check = check.limitation(format!("First value found in {}.", s.file));
    }
    if cfg.match_blocks {
        check = check.limitation("Match blocks not fully assessed; global value only.");
    }
    for l in &cfg.limitations {
        check = check.limitation(l.clone());
    }
    check.limitation(SSH_GLOBAL_NOTE)
}

fn judged(rule: &SshRule, j: Judgement, observed_at: &str) -> CheckResult {
    match j {
        Judgement::Finding(sev, why) => CheckResult::finding(
            rule.id,
            Category::Security,
            sev,
            rule.title,
            why,
            observed_at,
        )
        .recommendation(rule.recommendation),
        Judgement::Pass(note) => {
            let mut c = CheckResult::new(
                rule.id,
                Category::Security,
                Outcome::Passed,
                rule.title,
                format!(
                    "{} is not set to a risky value in the global configuration.",
                    rule.keyword
                ),
                observed_at,
            );
            if !note.is_empty() {
                c = c.limitation(note);
            }
            c
        }
    }
}

/// `ssh.password-auth`: `yes` or absent (OpenSSH default yes) is medium. `no`
/// is a pass for the global setting only.
pub fn ssh_password_auth(cfg: &SshdConfig, observed_at: &str) -> CheckResult {
    let rule = SshRule {
        id: "ssh.password-auth",
        title: "SSH password authentication",
        keyword: "PasswordAuthentication",
        judge: judge_password,
        default: (
            "yes",
            Judgement::Finding(
                Severity::Medium,
                "PasswordAuthentication is not set, so the OpenSSH default yes applies.",
            ),
        ),
        recommendation: "Use key-based login and set PasswordAuthentication no once key access works.",
    };
    ssh_check(
        &rule,
        cfg.password_authentication.as_ref(),
        cfg,
        observed_at,
    )
}

/// `ssh.root-login`: `yes` is high. `no`, `prohibit-password`,
/// `without-password`, `forced-commands-only` and the default pass, with the
/// observed restriction as a limitation.
pub fn ssh_root_login(cfg: &SshdConfig, observed_at: &str) -> CheckResult {
    let rule = SshRule {
        id: "ssh.root-login",
        title: "SSH root login",
        keyword: "PermitRootLogin",
        judge: judge_root,
        default: (
            "prohibit-password",
            Judgement::Pass("Root login allowed with public key only."),
        ),
        recommendation: "Set PermitRootLogin to no or prohibit-password and log in as a normal user.",
    };
    ssh_check(&rule, cfg.permit_root_login.as_ref(), cfg, observed_at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ossiframe_mod_linux_inventory::DiskInfo;
    use ossiframe_mod_linux_inventory::services::Service;
    use ossiframe_mod_linux_inventory::sockets::{Family, Protocol, SocketProcess};

    const AT: &str = "2025-01-01T00:00:00Z";

    fn mem(total: u64, avail: u64) -> ServerInfo {
        ServerInfo {
            memory_total_kb: Some(total),
            memory_available_kb: Some(avail),
            swap_total_kb: Some(100),
            swap_free_kb: Some(90),
            ..Default::default()
        }
    }

    fn sev(c: &CheckResult) -> Option<Severity> {
        c.severity
    }

    #[test]
    fn memory_thresholds() {
        let r = |a| mem_low_available(&mem(1000, a), AT);
        assert_eq!(r(100).outcome, Outcome::Passed);
        assert_eq!(
            (r(99).outcome, sev(&r(99))),
            (Outcome::Finding, Some(Severity::Medium))
        );
        assert_eq!(sev(&r(50)), Some(Severity::Medium));
        assert_eq!(sev(&r(49)), Some(Severity::High));
        let c = r(49);
        assert!(c.explanation.contains("single snapshot"));
        assert!(c.evidence.iter().any(|e| e.contains("Swap")));
    }

    #[test]
    fn memory_missing_is_unavailable() {
        let c = mem_low_available(&ServerInfo::default(), AT);
        assert_eq!(c.outcome, Outcome::Unavailable);
        assert_eq!(
            mem_low_available(&mem(0, 0), AT).outcome,
            Outcome::Unavailable
        );
    }

    fn disk(used: u64, avail: u64) -> ServerInfo {
        ServerInfo {
            filesystems: vec![DiskInfo {
                mount: "/".to_owned(),
                total_bytes: used + avail,
                used_bytes: used,
                available_bytes: avail,
            }],
            ..Default::default()
        }
    }

    const GIB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn boot_efi_reference_numbers_pass() {
        let info = ServerInfo {
            filesystems: vec![DiskInfo {
                mount: "/boot/efi".to_owned(),
                total_bytes: 109_395_456,
                used_bytes: 6_399_488,
                available_bytes: 102_995_968,
            }],
            ..Default::default()
        };
        let r = disk_low_space(&info, AT).remove(0);
        assert_eq!(r.outcome, Outcome::Passed);
    }
    #[test]
    fn unrecognized_socket_lines_are_never_a_pass() {
        let info = SocketsInfo {
            available: true,
            unrecognized_lines: 1,
            ..Default::default()
        };
        let r = net_database_exposed(&info, AT);
        assert!(r.iter().all(|c| c.outcome == Outcome::Incomplete));
        assert!(!r.is_empty());
    }
    #[test]
    fn disk_thresholds_use_df_percentage() {
        let r = |used: u64| disk_low_space(&disk(used * GIB, (100 - used) * GIB), AT).remove(0);
        assert_eq!(r(79).outcome, Outcome::Passed);
        assert_eq!(sev(&r(80)), Some(Severity::Medium));
        assert_eq!(sev(&r(89)), Some(Severity::Medium));
        assert_eq!(sev(&r(90)), Some(Severity::High));
        assert_eq!(sev(&r(94)), Some(Severity::High));
        assert_eq!(sev(&r(95)), Some(Severity::Critical));
    }

    #[test]
    fn disk_low_available_is_high_and_highest_wins() {
        let min = DISK_MIN_AVAILABLE_BYTES;
        let mib = 1024 * 1024;
        // 10 GiB filesystem (e.g. most space reserved) with 200 MiB used, so
        // the percentage alone stays below every threshold.
        let at = |avail: u64| {
            let mut info = disk(200 * mib, avail);
            info.filesystems[0].total_bytes = 10 * GIB;
            disk_low_space(&info, AT).remove(0)
        };
        assert_eq!(at(min).outcome, Outcome::Passed);
        assert_eq!(sev(&at(99 * mib)), Some(Severity::High));
        // 105 MB vfat /boot/efi at 5% used: small, so only percentages apply.
        let small = |used_pct: u64| {
            let total = 105_000_000;
            let used = total * used_pct / 100;
            disk_low_space(&disk(used, total - used), AT).remove(0)
        };
        assert_eq!(small(5).outcome, Outcome::Passed);
        assert_eq!(sev(&small(96)), Some(Severity::Critical));
        // 99.9% used and nearly no space: critical beats high, one result.
        let both = disk_low_space(&disk(100 * GIB, 1024), AT);
        assert_eq!(both.len(), 1);
        assert_eq!(sev(&both[0]), Some(Severity::Critical));
    }

    #[test]
    fn disk_missing_or_truncated_is_never_passed() {
        let none = disk_low_space(&ServerInfo::default(), AT);
        assert_eq!(none[0].outcome, Outcome::Unavailable);
        let mut info = disk(GIB, 99 * GIB);
        info.filesystems_truncated = true;
        let out = disk_low_space(&info, AT);
        assert_eq!(out.last().unwrap().outcome, Outcome::Incomplete);
        let zero = disk_low_space(&disk(0, 0), AT);
        assert_eq!(zero[0].outcome, Outcome::Unavailable);
    }

    fn svc(unit: &str, active: &str) -> Service {
        Service {
            unit: unit.to_owned(),
            description: None,
            installed: true,
            load: "loaded".to_owned(),
            active: active.to_owned(),
            sub: "x".to_owned(),
            enablement: "unknown".to_owned(),
        }
    }

    fn svcs(list: Vec<Service>, truncated: bool) -> ServicesInfo {
        ServicesInfo {
            available: true,
            services: list,
            truncated,
            ..Default::default()
        }
    }

    #[test]
    fn only_failed_units_are_findings() {
        let info = svcs(
            vec![
                svc("a.service", "failed"),
                svc("b.service", "inactive"),
                svc("c.service", "unknown"),
                svc("d.service", "active"),
            ],
            false,
        );
        let out = svc_failed(&info, AT);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].outcome, Outcome::Finding);
        assert_eq!(out[0].resource.as_deref(), Some("a.service"));
        assert_eq!(sev(&out[0]), Some(Severity::Medium));
    }

    #[test]
    fn complete_list_without_failures_passes_once() {
        let out = svc_failed(&svcs(vec![svc("a.service", "active")], false), AT);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].outcome, Outcome::Passed);
    }

    #[test]
    fn truncated_or_unavailable_service_list_is_not_a_pass() {
        let t = svc_failed(&svcs(vec![svc("a.service", "active")], true), AT);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].outcome, Outcome::Incomplete);
        let u = svc_failed(&ServicesInfo::default(), AT);
        assert_eq!(u[0].outcome, Outcome::Unavailable);
        // Findings found before the limit are kept, and the cut is visible.
        let both = svc_failed(&svcs(vec![svc("a.service", "failed")], true), AT);
        assert_eq!(both[0].outcome, Outcome::Finding);
        assert_eq!(both[1].outcome, Outcome::Incomplete);
    }

    #[test]
    fn failed_findings_are_capped_with_a_limitation() {
        let list = (0..25)
            .map(|i| svc(&format!("u{i}.service"), "failed"))
            .collect();
        let out = svc_failed(&svcs(list, false), AT);
        assert_eq!(out.len(), MAX_FAILED_FINDINGS);
        assert!(out.iter().all(|c| c.outcome == Outcome::Finding));
        assert!(
            out.last()
                .unwrap()
                .limitations
                .iter()
                .any(|l| l.contains("5 more"))
        );
    }

    fn sock(port: u16, address: &str, binding: BindingClass, private: bool) -> ListeningSocket {
        ListeningSocket {
            protocol: Protocol::Tcp,
            address: address.to_owned(),
            port,
            family: Family::Ipv4,
            binding,
            private,
            process: None,
        }
    }

    fn socks(list: Vec<ListeningSocket>, truncated: bool) -> SocketsInfo {
        SocketsInfo {
            available: true,
            sockets: list,
            truncated,
            ..Default::default()
        }
    }

    #[test]
    fn database_severity_by_binding_class() {
        let info = socks(
            vec![
                sock(3306, "127.0.0.1", BindingClass::Loopback, false),
                sock(5432, "10.0.0.5", BindingClass::Specific, true),
                sock(6379, "0.0.0.0", BindingClass::Wildcard, false),
                sock(27017, "203.0.113.9", BindingClass::Specific, false),
                sock(8080, "0.0.0.0", BindingClass::Wildcard, false),
            ],
            false,
        );
        let out = net_database_exposed(&info, AT);
        let sevs: Vec<_> = out.iter().map(sev).collect();
        assert_eq!(
            sevs,
            [
                Some(Severity::Low),
                Some(Severity::Medium),
                Some(Severity::Medium)
            ]
        );
        assert!(out.iter().all(|c| c.outcome == Outcome::Finding));
    }

    #[test]
    fn database_findings_are_inferred_and_carefully_worded() {
        let mut known = sock(3306, "0.0.0.0", BindingClass::Wildcard, false);
        known.process = Some(SocketProcess {
            name: "mysqld".to_owned(),
            pid: 1,
        });
        let mut other = sock(3306, "0.0.0.0", BindingClass::Wildcard, false);
        other.process = Some(SocketProcess {
            name: "nginx".to_owned(),
            pid: 2,
        });
        let hidden = sock(5432, "0.0.0.0", BindingClass::Wildcard, false);
        let out = net_database_exposed(&socks(vec![known, other, hidden], false), AT);
        assert!(!out[0].inferred);
        assert!(out[1].inferred);
        assert!(out[2].inferred);
        for c in &out {
            let text = c.explanation.to_ascii_lowercase();
            assert!(text.contains("non-loopback address"));
            assert!(!text.contains("publicly reachable"));
            assert_ne!(c.severity, Some(Severity::High));
        }
    }

    #[test]
    fn loopback_only_passes_and_truncated_or_missing_does_not() {
        let ok = net_database_exposed(
            &socks(
                vec![sock(3306, "127.0.0.1", BindingClass::Loopback, false)],
                false,
            ),
            AT,
        );
        assert_eq!(ok.len(), 1);
        assert_eq!(ok[0].outcome, Outcome::Passed);
        let t = net_database_exposed(&socks(Vec::new(), true), AT);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].outcome, Outcome::Incomplete);
        let u = net_database_exposed(&SocketsInfo::default(), AT);
        assert_eq!(u[0].outcome, Outcome::Unavailable);
        let kept = net_database_exposed(
            &socks(
                vec![sock(3306, "0.0.0.0", BindingClass::Wildcard, false)],
                true,
            ),
            AT,
        );
        assert_eq!(kept[0].outcome, Outcome::Finding);
        assert_eq!(kept[1].outcome, Outcome::Incomplete);
    }

    fn setting(v: &str) -> Option<Setting> {
        Some(Setting {
            value: v.to_owned(),
            file: "/etc/ssh/sshd_config".to_owned(),
        })
    }

    fn cfg(pw: Option<&str>, root: Option<&str>) -> SshdConfig {
        SshdConfig {
            files: vec!["/etc/ssh/sshd_config".to_owned()],
            password_authentication: pw.and_then(setting),
            permit_root_login: root.and_then(setting),
            ..Default::default()
        }
    }

    fn pw(c: &SshdConfig) -> (Outcome, Option<Severity>) {
        let r = ssh_password_auth(c, AT);
        (r.outcome, r.severity)
    }

    fn root(c: &SshdConfig) -> CheckResult {
        ssh_root_login(c, AT)
    }

    #[test]
    fn ssh_password_auth_values() {
        assert_eq!(
            pw(&cfg(Some("yes"), None)),
            (Outcome::Finding, Some(Severity::Medium))
        );
        assert_eq!(pw(&cfg(Some("YES"), None)).0, Outcome::Finding);
        // Absent in fully read files: OpenSSH default yes.
        assert_eq!(
            pw(&cfg(None, None)),
            (Outcome::Finding, Some(Severity::Medium))
        );
        let r = ssh_password_auth(&cfg(Some("no"), None), AT);
        assert_eq!(r.outcome, Outcome::Passed);
        assert!(
            r.limitations
                .iter()
                .any(|l| l.contains("Global setting only"))
        );
        assert!(r.limitations.iter().any(|l| l.contains("not per-user")));
        assert_eq!(
            r.evidence,
            ["PasswordAuthentication no (/etc/ssh/sshd_config)"]
        );
    }

    #[test]
    fn ssh_unknown_value_is_an_error_never_a_default() {
        for v in ["maybe", "true", "no yes", "(unparsable)"] {
            assert_eq!(pw(&cfg(Some(v), None)).0, Outcome::Error, "{v}");
            assert_eq!(root(&cfg(None, Some(v))).outcome, Outcome::Error, "{v}");
        }
        assert_eq!(
            root(&cfg(None, Some("PROHIBIT-PASSWORD"))).outcome,
            Outcome::Passed
        );
    }

    #[test]
    fn ssh_root_login_values_and_wording() {
        let r = root(&cfg(None, Some("yes")));
        assert_eq!(
            (r.outcome, r.severity),
            (Outcome::Finding, Some(Severity::High))
        );
        assert_eq!(root(&cfg(None, Some("no"))).outcome, Outcome::Passed);
        for (v, text) in [
            (Some("prohibit-password"), "public key only"),
            (Some("without-password"), "public key only"),
            (None, "public key only"),
            (Some("forced-commands-only"), "forced commands only"),
        ] {
            let r = root(&cfg(None, v));
            assert_eq!(r.outcome, Outcome::Passed, "{v:?}");
            assert!(r.limitations.iter().any(|l| l.contains(text)), "{v:?}");
            let all = format!("{} {} {}", r.title, r.explanation, r.limitations.join(" "));
            assert!(!all.to_ascii_lowercase().contains("disabled"), "{all}");
        }
        let d = root(&cfg(None, None));
        assert_eq!(d.evidence, ["PermitRootLogin: default (not set)"]);
    }

    #[test]
    fn ssh_match_turns_a_pass_into_incomplete_but_keeps_findings() {
        let mut c = cfg(Some("no"), Some("yes"));
        c.match_blocks = true;
        assert_eq!(ssh_password_auth(&c, AT).outcome, Outcome::Incomplete);
        assert_eq!(root(&c).outcome, Outcome::Finding);
        let r = ssh_password_auth(&c, AT);
        assert!(
            r.limitations
                .iter()
                .any(|l| l.contains("Match blocks not fully assessed"))
        );
        // Absent root login with Match: would-be pass is incomplete too.
        assert_eq!(
            root(&SshdConfig {
                match_blocks: true,
                ..cfg(None, None)
            })
            .outcome,
            Outcome::Incomplete
        );
    }

    #[test]
    fn ssh_gaps_are_never_a_pass() {
        let gap = |g: Gap, pw_v: Option<&str>| SshdConfig {
            gap: Some(g),
            ..cfg(pw_v, None)
        };
        assert_eq!(pw(&gap(Gap::Unreadable, None)).0, Outcome::Unavailable);
        assert_eq!(pw(&gap(Gap::Skipped, None)).0, Outcome::Incomplete);
        assert_eq!(pw(&gap(Gap::Failed("x".into()), None)).0, Outcome::Error);
        // A value read before the gap: pass becomes incomplete, finding stays.
        assert_eq!(pw(&gap(Gap::Skipped, Some("no"))).0, Outcome::Incomplete);
        assert_eq!(pw(&gap(Gap::Unreadable, Some("yes"))).0, Outcome::Finding);
        // Root absent behind a gap must not use the default.
        assert_eq!(root(&gap(Gap::Skipped, None)).outcome, Outcome::Incomplete);
    }

    #[test]
    fn ssh_results_always_state_the_global_scope() {
        for c in [
            cfg(Some("no"), Some("no")),
            cfg(None, None),
            cfg(Some("x"), Some("x")),
        ] {
            for r in [ssh_password_auth(&c, AT), root(&c)] {
                assert!(
                    r.limitations
                        .iter()
                        .any(|l| l.contains("global configuration on disk"))
                );
                assert!(
                    r.limitations
                        .iter()
                        .any(|l| l.contains("not the running daemon"))
                );
            }
        }
    }
}
