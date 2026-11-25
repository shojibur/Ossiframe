//! The `updates` step. It reads cached data only: the update-notifier text
//! file, the mtime of the apt success stamp and of the reboot marker, and the
//! server clock. It never runs apt, `apt-check` or any command that refreshes
//! or loads the package cache.
use crate::model::{Category, CheckResult, Outcome, Severity};
use crate::probe::{ProbeFailure, run_probe};
use ossiframe_core::{CancelToken, ExecError, Executor};

pub const UPDATES_AVAILABLE: &str = "/var/lib/update-notifier/updates-available";
pub const UPDATE_STAMP: &str = "/var/lib/apt/periodic/update-success-stamp";
pub const REBOOT_REQUIRED: &str = "/var/run/reboot-required";

/// Metadata older than this (strictly) is stale.
pub const STALE_AFTER_SECS: u64 = 7 * 24 * 60 * 60;
/// Largest number read from the text file. Larger values are not plausible.
const MAX_COUNT: u32 = 1_000_000;

const CACHED: &str = "Based on cached update-notifier and apt metadata, not live repository state.";

/// Counts from `updates-available`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub total: u32,
    pub security: u32,
    /// Extra security updates only available through ESM, if reported. Not
    /// included in `total` or `security`.
    pub esm_security: Option<u32>,
    /// The recognized lines, for evidence.
    pub lines: Vec<String>,
}

/// Raw evidence, one `Result` per probe.
pub struct Collected {
    pub pending: Result<Pending, ProbeFailure>,
    /// Age of the apt success stamp in seconds.
    pub age: Result<u64, ProbeFailure>,
    /// Modification time of the reboot marker. `Missing` means no marker.
    pub reboot: Result<u64, ProbeFailure>,
}

fn count(token: &str) -> Option<u32> {
    token.parse::<u32>().ok().filter(|n| *n <= MAX_COUNT)
}

/// Parses the localized English motd text written by update-notifier, for
/// example `3 updates can be applied immediately.` and
/// `1 of these updates is a standard security update.` Unrecognized text is
/// an error, never a pass. Absence of the security line means 0 security
/// updates.
pub fn parse_pending(text: &str) -> Result<Pending, &'static str> {
    let mut total = None;
    let mut security = None;
    let mut esm = None;
    let mut lines = Vec::new();
    for raw in text.lines().take(100) {
        let lower = raw.trim().trim_end_matches('.').to_ascii_lowercase();
        let words: Vec<&str> = lower.split_whitespace().collect();
        let Some(n) = words.first().and_then(|w| count(w)) else {
            continue;
        };
        let is_total = words
            .get(1)
            .is_some_and(|w| *w == "update" || *w == "updates")
            && lower.ends_with("can be applied immediately");
        let is_security = words.get(1) == Some(&"of")
            && words.get(2) == Some(&"these")
            && words.get(3).is_some_and(|w| w.starts_with("update"))
            && words.contains(&"security");
        let is_esm = lower.contains("additional security update") && lower.contains("esm");
        if is_total && total.is_none() {
            total = Some(n);
            lines.push(raw.trim().to_owned());
        } else if is_security && security.is_none() {
            security = Some(n);
            lines.push(raw.trim().to_owned());
        } else if is_esm && esm.is_none() {
            esm = Some(n);
            lines.push(raw.trim().to_owned());
        }
    }
    let Some(total) = total else {
        return Err("the update count line was not recognized");
    };
    let security = security.unwrap_or(0);
    if security > total {
        return Err("the security update count is larger than the total");
    }
    Ok(Pending {
        total,
        security,
        esm_security: esm,
        lines,
    })
}

fn parse_epoch(s: &str) -> Result<u64, ProbeFailure> {
    s.trim()
        .parse::<u64>()
        .map_err(|_| ProbeFailure::Error("timestamp was not a number".to_owned()))
}

/// Age of `stamp` at `now`. A stamp in the future is an error (clock skew).
pub fn age_secs(now: u64, stamp: u64) -> Result<u64, ProbeFailure> {
    now.checked_sub(stamp)
        .ok_or_else(|| ProbeFailure::Error("stamp is newer than the server clock".to_owned()))
}

/// Runs the read-only probes. `Err` only for cancel and connection loss.
pub async fn collect(exec: &dyn Executor, cancel: &CancelToken) -> Result<Collected, ExecError> {
    let pending = run_probe(exec, cancel, "cat", &[UPDATES_AVAILABLE])
        .await?
        .and_then(|t| parse_pending(&t).map_err(|m| ProbeFailure::Error(m.to_owned())));
    let stamp = run_probe(exec, cancel, "stat", &["-c", "%Y", UPDATE_STAMP])
        .await?
        .and_then(|t| parse_epoch(&t));
    let age = match stamp {
        Ok(stamp) => run_probe(exec, cancel, "date", &["-u", "+%s"])
            .await?
            .and_then(|t| parse_epoch(&t))
            .and_then(|now| age_secs(now, stamp)),
        Err(e) => Err(e),
    };
    let reboot = run_probe(exec, cancel, "stat", &["-c", "%Y", REBOOT_REQUIRED])
        .await?
        .and_then(|t| parse_epoch(&t));
    Ok(Collected {
        pending,
        age,
        reboot,
    })
}

/// "3 days 4 hours" style text for evidence and explanations.
pub fn format_age(secs: u64) -> String {
    let (d, h) = (secs / 86_400, secs % 86_400 / 3_600);
    match (d, h) {
        (0, 0) => format!("{} minutes", secs / 60),
        (0, h) => format!("{h} hours"),
        (d, h) => format!("{d} days {h} hours"),
    }
}

fn age_note(age: &Result<u64, ProbeFailure>) -> String {
    match age {
        Ok(a) => format!(
            "Cache age: apt metadata last refreshed {} ago.",
            format_age(*a)
        ),
        Err(_) => "Cache age is unknown.".to_owned(),
    }
}

/// `upd.security-pending`.
pub fn security_pending(
    pending: &Result<Pending, ProbeFailure>,
    age: &Result<u64, ProbeFailure>,
    observed_at: &str,
) -> CheckResult {
    let title = "Pending security updates";
    let rule = "upd.security-pending";
    let stale = match age {
        Ok(a) => *a > STALE_AFTER_SECS,
        Err(_) => true,
    };
    let p = match pending {
        Ok(p) => p,
        Err(f) => {
            return f
                .into_check(rule, Category::Updates, title, false, observed_at)
                .resource(UPDATES_AVAILABLE)
                .limitation(CACHED)
                .limitation(age_note(age));
        }
    };
    let mut c = if p.security > 0 {
        CheckResult::finding(
            rule,
            Category::Updates,
            Severity::Medium,
            title,
            format!("{} security update(s) are reported as pending.", p.security),
            observed_at,
        )
        .recommendation("Install pending security updates with apt, then reboot if required.")
    } else if p.total > 0 {
        CheckResult::finding(
            rule,
            Category::Updates,
            Severity::Info,
            title,
            format!(
                "{} update(s) are reported as pending, none of them security updates.",
                p.total
            ),
            observed_at,
        )
    } else if stale {
        CheckResult::new(
            rule,
            Category::Updates,
            Outcome::Incomplete,
            title,
            "Not fully assessed: the cached data reports no pending updates, but the apt \
             metadata is stale or its age is unknown, so pending updates may be missed.",
            observed_at,
        )
    } else {
        CheckResult::new(
            rule,
            Category::Updates,
            Outcome::Passed,
            title,
            "The cached data reports no pending updates.",
            observed_at,
        )
    };
    c = c
        .resource(UPDATES_AVAILABLE)
        .evidence(&p.lines)
        .limitation(CACHED)
        .limitation(age_note(age));
    if stale && p.total > 0 {
        c = c.limitation(
            "The cached metadata is stale or its age is unknown; counts may be outdated.",
        );
    }
    if let Some(n) = p.esm_security {
        c = c.limitation(format!(
            "{n} additional ESM security update(s) are reported and not included in the counts."
        ));
    }
    c
}

/// `upd.metadata-stale`.
pub fn metadata_stale(age: &Result<u64, ProbeFailure>, observed_at: &str) -> CheckResult {
    let title = "Apt metadata age";
    let rule = "upd.metadata-stale";
    let note = "update-success-stamp records the last successful apt update; the pending \
                update file is only as fresh as this stamp.";
    let c = match age {
        Err(f) => f
            .into_check(rule, Category::Updates, title, false, observed_at)
            .resource(UPDATE_STAMP),
        Ok(a) if *a > STALE_AFTER_SECS => CheckResult::finding(
            rule,
            Category::Updates,
            Severity::Low,
            title,
            format!(
                "Apt package metadata was last refreshed {} ago.",
                format_age(*a)
            ),
            observed_at,
        )
        .resource(UPDATE_STAMP)
        .evidence([format!("age: {}", format_age(*a))])
        .recommendation("Check that automatic apt updates run, or run apt update manually."),
        Ok(a) => CheckResult::new(
            rule,
            Category::Updates,
            Outcome::Passed,
            title,
            format!("Apt package metadata was refreshed {} ago.", format_age(*a)),
            observed_at,
        )
        .resource(UPDATE_STAMP)
        .evidence([format!("age: {}", format_age(*a))]),
    };
    c.limitation(CACHED).limitation(note)
}

/// `upd.reboot-required`. A missing marker is a pass.
pub fn reboot_required(
    reboot: &Result<u64, ProbeFailure>,
    observed_at: &str,
) -> (CheckResult, Option<bool>) {
    let title = "Reboot required";
    let rule = "upd.reboot-required";
    let (c, state) = match reboot {
        Ok(_) => (
            CheckResult::finding(
                rule,
                Category::Updates,
                Severity::Info,
                title,
                "The reboot-required marker exists.",
                observed_at,
            )
            .evidence([REBOOT_REQUIRED])
            .recommendation("Reboot at a suitable time so installed updates take effect."),
            Some(true),
        ),
        Err(ProbeFailure::Missing) => (
            CheckResult::new(
                rule,
                Category::Updates,
                Outcome::Passed,
                title,
                "The reboot-required marker does not exist.",
                observed_at,
            ),
            Some(false),
        ),
        Err(f) => (
            f.into_check(rule, Category::Updates, title, false, observed_at),
            None,
        ),
    };
    (
        c.resource(REBOOT_REQUIRED)
            .limitation("Marker file only; services that need a restart are not checked.")
            .limitation(CACHED),
        state,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: &str = "2025-01-01T00:00:00Z";
    const DAY: u64 = 86_400;

    fn p(total: u32, security: u32) -> Result<Pending, ProbeFailure> {
        Ok(Pending {
            total,
            security,
            esm_security: None,
            lines: vec!["x".to_owned()],
        })
    }

    #[test]
    fn parses_motd_text() {
        let t = " \n3 updates can be applied immediately.\n1 of these updates is a standard \
                 security update.\nTo see these additional updates run: apt list --upgradable\n";
        let r = parse_pending(t).unwrap();
        assert_eq!((r.total, r.security, r.esm_security), (3, 1, None));
        assert_eq!(r.lines.len(), 2);
        let r = parse_pending("1 update can be applied immediately.").unwrap();
        assert_eq!((r.total, r.security), (1, 0));
        let r = parse_pending("0 updates can be applied immediately.\n").unwrap();
        assert_eq!(r.total, 0);
        let r = parse_pending(
            "10 updates can be applied immediately.\n4 of these updates are standard security \
             updates.\n2 additional security updates can be applied with ESM Apps.\n",
        )
        .unwrap();
        assert_eq!((r.total, r.security, r.esm_security), (10, 4, Some(2)));
    }

    #[test]
    fn unparsable_text_is_an_error_not_zero() {
        for t in [
            "",
            "   \n",
            "Aktualisierungen verfügbar\n",
            "many updates can be applied immediately.",
            "-1 updates can be applied immediately.",
            "99999999999 updates can be applied immediately.",
            "2 of these updates are standard security updates.",
            "1 updates can be applied immediately.\n5 of these updates are standard security updates.",
        ] {
            assert!(parse_pending(t).is_err(), "{t:?}");
        }
    }

    #[test]
    fn stale_boundary_is_strictly_more_than_seven_days() {
        let at = metadata_stale(&Ok(7 * DAY), AT);
        assert_eq!(at.outcome, Outcome::Passed);
        let over = metadata_stale(&Ok(7 * DAY + 1), AT);
        assert_eq!(
            (over.outcome, over.severity),
            (Outcome::Finding, Some(Severity::Low))
        );
        assert!(over.limitations.iter().any(|l| l.contains("cached")));
        assert_eq!(age_secs(100, 40).unwrap(), 60);
        assert!(age_secs(40, 100).is_err());
    }

    #[test]
    fn metadata_stale_missing_stamp_is_unavailable() {
        let c = metadata_stale(&Err(ProbeFailure::Missing), AT);
        assert_eq!(c.outcome, Outcome::Unavailable);
        assert!(c.severity.is_none());
        let c = metadata_stale(&Err(ProbeFailure::Error("x".into())), AT);
        assert_eq!(c.outcome, Outcome::Error);
    }

    #[test]
    fn security_pending_severities() {
        let fresh = Ok(DAY);
        let c = security_pending(&p(3, 1), &fresh, AT);
        assert_eq!(
            (c.outcome, c.severity),
            (Outcome::Finding, Some(Severity::Medium))
        );
        let c = security_pending(&p(3, 0), &fresh, AT);
        assert_eq!(
            (c.outcome, c.severity),
            (Outcome::Finding, Some(Severity::Info))
        );
        let c = security_pending(&p(0, 0), &fresh, AT);
        assert_eq!(c.outcome, Outcome::Passed);
        assert!(!c.explanation.to_lowercase().contains("patched"));
    }

    #[test]
    fn zero_pending_with_stale_or_unknown_age_is_incomplete() {
        for age in [
            Ok(7 * DAY + 1),
            Err(ProbeFailure::Missing),
            Err(ProbeFailure::Error("x".into())),
        ] {
            let c = security_pending(&p(0, 0), &age, AT);
            assert_eq!(c.outcome, Outcome::Incomplete);
            assert!(
                c.limitations
                    .iter()
                    .any(|l| l.contains("not live repository"))
            );
        }
        // Findings are kept, with a stale-data limitation.
        let c = security_pending(&p(2, 1), &Ok(8 * DAY), AT);
        assert_eq!(c.outcome, Outcome::Finding);
        assert!(c.limitations.iter().any(|l| l.contains("stale")));
    }

    #[test]
    fn missing_or_unparsable_updates_file_is_never_a_pass() {
        let c = security_pending(&Err(ProbeFailure::Missing), &Ok(DAY), AT);
        assert_eq!(c.outcome, Outcome::Unavailable);
        assert!(!c.limitations.is_empty());
        let c = security_pending(&Err(ProbeFailure::Error("bad".into())), &Ok(DAY), AT);
        assert_eq!(c.outcome, Outcome::Error);
    }

    #[test]
    fn reboot_marker_states() {
        let (c, s) = reboot_required(&Ok(1), AT);
        assert_eq!((c.severity, s), (Some(Severity::Info), Some(true)));
        let (c, s) = reboot_required(&Err(ProbeFailure::Missing), AT);
        assert_eq!((c.outcome, s), (Outcome::Passed, Some(false)));
        let (c, s) = reboot_required(&Err(ProbeFailure::PermissionDenied), AT);
        assert_eq!((c.outcome, s), (Outcome::Unavailable, None));
    }
}
