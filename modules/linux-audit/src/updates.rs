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
    /// Standard security updates. The parser sets `Some` for every recognized
    /// file; `None` is only constructed by callers that do not know the count.
    pub security: Option<u32>,
    /// True when `security` is `Some(0)` because the security line was absent
    /// while updates are pending, not because the file said zero.
    pub security_inferred: bool,
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
    /// Age of the `updates-available` file itself (its own mtime), in seconds.
    pub cache_age: Result<u64, ProbeFailure>,
    /// Modification time of the reboot marker. `Missing` means no marker.
    pub reboot: Result<u64, ProbeFailure>,
}

fn count(token: &str) -> Option<u32> {
    token.parse::<u32>().ok().filter(|n| *n <= MAX_COUNT)
}

/// Parses the localized English motd text written by update-notifier, for
/// example `3 updates can be applied immediately.` and
/// `1 of these updates is a standard security update.` Unrecognized text is
/// an error, never a pass.
///
/// Source: update-notifier `data/apt_check.py`, function
/// `write_human_readable_summary`
/// (https://git.launchpad.net/update-notifier/tree/data/apt_check.py). It
/// always writes the "N updates can be applied immediately." line, and writes
/// the "standard security update(s)" line only inside `if security_updates > 0`.
/// So with a recognized total line, an absent security line means zero
/// standard security updates, and this parser infers `Some(0)` and flags it
/// with `security_inferred`. A security line that exists but cannot be read
/// is still an error. ESM security updates are counted separately by the
/// source and are not part of this count.
pub fn parse_pending(text: &str) -> Result<Pending, &'static str> {
    let mut total = None;
    let mut security = None;
    let mut esm = None;
    let mut lines = Vec::new();
    for raw in text.lines().take(100) {
        let lower = raw.trim().trim_end_matches('.').to_ascii_lowercase();
        let words: Vec<&str> = lower.split_whitespace().collect();
        let n = words.first().and_then(|w| count(w));
        // A security count line whose number cannot be read must not leave the
        // count defaulting to zero.
        if lower.contains("of these update") && lower.contains("security") && n.is_none() {
            return Err("the security update count line was not recognized");
        }
        let Some(n) = n else {
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
    // See the source note above: no security line means zero.
    let security_inferred = security.is_none() && total > 0;
    let security = security.or(Some(0));
    if security.is_some_and(|s| s > total) {
        return Err("the security update count is larger than the total");
    }
    Ok(Pending {
        total,
        security,
        security_inferred,
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
    let cache_mtime = run_probe(exec, cancel, "stat", &["-c", "%Y", UPDATES_AVAILABLE])
        .await?
        .and_then(|t| parse_epoch(&t));
    let stamp = run_probe(exec, cancel, "stat", &["-c", "%Y", UPDATE_STAMP])
        .await?
        .and_then(|t| parse_epoch(&t));
    // The server clock is read once, and only when an age can be computed.
    let now = if cache_mtime.is_ok() || stamp.is_ok() {
        run_probe(exec, cancel, "date", &["-u", "+%s"])
            .await?
            .and_then(|t| parse_epoch(&t))
    } else {
        Err(ProbeFailure::Error("server clock not read".to_owned()))
    };
    let age_of = |mtime: Result<u64, ProbeFailure>| {
        let m = mtime?;
        age_secs(now.clone()?, m)
    };
    let cache_age = age_of(cache_mtime);
    let age = age_of(stamp);
    let reboot = run_probe(exec, cancel, "stat", &["-c", "%Y", REBOOT_REQUIRED])
        .await?
        .and_then(|t| parse_epoch(&t));
    Ok(Collected {
        pending,
        age,
        cache_age,
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

fn age_note(
    cache_age: &Result<u64, ProbeFailure>,
    stamp_age: &Result<u64, ProbeFailure>,
) -> String {
    let cache = match cache_age {
        Ok(a) => format!(
            "Cache age: the update-notifier file was last written {} ago.",
            format_age(*a)
        ),
        Err(_) => "Cache age is unknown.".to_owned(),
    };
    let stamp = match stamp_age {
        Ok(a) => format!(
            " Apt metadata was last refreshed {} ago (reported separately).",
            format_age(*a)
        ),
        Err(_) => String::new(),
    };
    cache + &stamp
}

/// `upd.security-pending`.
pub fn security_pending(
    pending: &Result<Pending, ProbeFailure>,
    cache_age: &Result<u64, ProbeFailure>,
    stamp_age: &Result<u64, ProbeFailure>,
    observed_at: &str,
) -> CheckResult {
    let title = "Pending security updates";
    let rule = "upd.security-pending";
    // Freshness is judged on the age of the update-notifier file itself.
    let stale = match cache_age {
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
                .limitation(age_note(cache_age, stamp_age));
        }
    };
    let mut c = if let Some(n) = p.security.filter(|n| *n > 0) {
        CheckResult::finding(
            rule,
            Category::Updates,
            Severity::Medium,
            title,
            format!("{n} security update(s) are reported as pending."),
            observed_at,
        )
        .recommendation("Install pending security updates with apt, then reboot if required.")
    } else if p.security.is_none() {
        CheckResult::new(
            rule,
            Category::Updates,
            Outcome::Incomplete,
            title,
            format!(
                "Not fully assessed: {} update(s) are reported as pending, but the cached data \
                 has no security update count, so security updates may be among them.",
                p.total
            ),
            observed_at,
        )
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
             cache is stale or its age is unknown, so pending updates may be missed.",
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
        .limitation(age_note(cache_age, stamp_age));
    if stale && p.total > 0 {
        c = c.limitation(
            "The cached metadata is stale or its age is unknown; counts may be outdated.",
        );
    }
    if p.security_inferred {
        c = c.limitation(
            "update-notifier omits the security line when there are no security updates; \
             inferred zero.",
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
    let note = "update-success-stamp records the last successful apt update. The age of the \
                pending update file is read separately from its own modification time.";
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
            security: Some(security),
            security_inferred: false,
            esm_security: None,
            lines: vec!["x".to_owned()],
        })
    }

    /// Updates pending, security count line absent.
    fn p_unknown(total: u32) -> Result<Pending, ProbeFailure> {
        Ok(Pending {
            total,
            security: None,
            security_inferred: false,
            esm_security: None,
            lines: vec!["x".to_owned()],
        })
    }

    fn sp(
        pending: &Result<Pending, ProbeFailure>,
        cache_age: &Result<u64, ProbeFailure>,
    ) -> CheckResult {
        security_pending(pending, cache_age, &Ok(DAY), AT)
    }

    #[test]
    fn parses_motd_text() {
        let t = " \n3 updates can be applied immediately.\n1 of these updates is a standard \
                 security update.\nTo see these additional updates run: apt list --upgradable\n";
        let r = parse_pending(t).unwrap();
        assert_eq!((r.total, r.security, r.esm_security), (3, Some(1), None));
        assert_eq!(r.lines.len(), 2);
        assert!(!r.security_inferred);
        // update-notifier omits the security line when the count is zero.
        let r = parse_pending("1 update can be applied immediately.").unwrap();
        assert_eq!(
            (r.total, r.security, r.security_inferred),
            (1, Some(0), true)
        );
        let r = parse_pending("0 updates can be applied immediately.\n").unwrap();
        assert_eq!(
            (r.total, r.security, r.security_inferred),
            (0, Some(0), false)
        );
        let r = parse_pending(
            "10 updates can be applied immediately.\n4 of these updates are standard security \
             updates.\n2 additional security updates can be applied with ESM Apps.\n",
        )
        .unwrap();
        assert_eq!(
            (r.total, r.security, r.esm_security),
            (10, Some(4), Some(2))
        );
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
            "3 updates can be applied immediately.\nmany of these updates are security updates.",
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
        let c = sp(&p(3, 1), &fresh);
        assert_eq!(
            (c.outcome, c.severity),
            (Outcome::Finding, Some(Severity::Medium))
        );
        let c = sp(&p(3, 0), &fresh);
        assert_eq!(
            (c.outcome, c.severity),
            (Outcome::Finding, Some(Severity::Info))
        );
        let c = sp(&p(0, 0), &fresh);
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
            let c = sp(&p(0, 0), &age);
            assert_eq!(c.outcome, Outcome::Incomplete);
            assert!(
                c.limitations
                    .iter()
                    .any(|l| l.contains("not live repository"))
            );
        }
        // Findings are kept, with a stale-data limitation.
        let c = sp(&p(2, 1), &Ok(8 * DAY));
        assert_eq!(c.outcome, Outcome::Finding);
        assert!(c.limitations.iter().any(|l| l.contains("stale")));
    }

    #[test]
    fn freshness_uses_the_cache_age_not_the_apt_stamp_age() {
        // Fresh apt stamp, stale cache file: still incomplete.
        let c = security_pending(&p(0, 0), &Ok(8 * DAY), &Ok(60), AT);
        assert_eq!(c.outcome, Outcome::Incomplete);
        // Stale apt stamp, fresh cache file: judged fresh, both ages reported.
        let c = security_pending(&p(0, 0), &Ok(60), &Ok(30 * DAY), AT);
        assert_eq!(c.outcome, Outcome::Passed);
        let notes = c.limitations.join(" ");
        assert!(
            notes.contains("30 days") && notes.contains("minutes"),
            "{notes}"
        );
    }

    #[test]
    fn missing_security_line_is_inferred_zero_with_limitation() {
        let pending = parse_pending("3 updates can be applied immediately.\n")
            .map_err(|m| ProbeFailure::Error(m.to_owned()));
        let c = sp(&pending, &Ok(60));
        assert_eq!(
            (c.outcome, c.severity),
            (Outcome::Finding, Some(Severity::Info))
        );
        assert!(
            c.limitations
                .iter()
                .any(|l| l.contains("inferred zero") && l.contains("omits the security line"))
        );
        // Explicit zero from the file carries no inference note.
        let c = sp(&p(3, 0), &Ok(60));
        assert!(!c.limitations.iter().any(|l| l.contains("inferred zero")));
    }

    #[test]
    fn unknown_security_count_is_never_zero_or_passed() {
        for age in [Ok(60), Ok(8 * DAY), Err(ProbeFailure::Missing)] {
            let c = sp(&p_unknown(3), &age);
            assert_eq!(c.outcome, Outcome::Incomplete);
            assert!(c.severity.is_none());
        }
    }

    #[tokio::test]
    async fn collect_reads_cache_age_from_its_own_mtime() {
        use async_trait::async_trait;
        use ossiframe_core::{CommandSpec, ExecResult, ExitStatus};
        struct Fake {
            cache: Option<&'static str>,
        }
        #[async_trait]
        impl Executor for Fake {
            async fn run(
                &self,
                spec: &CommandSpec,
                _: &CancelToken,
            ) -> Result<ExecResult, ExecError> {
                let last = spec.args().last().cloned().unwrap_or_default();
                let (code, out) = match (spec.program(), last.as_str()) {
                    ("cat", UPDATES_AVAILABLE) => (0, "0 updates can be applied immediately.\n"),
                    ("stat", UPDATES_AVAILABLE) => match self.cache {
                        Some(t) => (0, t),
                        None => (1, ""),
                    },
                    ("stat", UPDATE_STAMP) => (0, "100\n"),
                    ("date", "+%s") => (0, "1000\n"),
                    _ => (1, ""),
                };
                Ok(ExecResult {
                    exit: ExitStatus::Code(code),
                    stdout: out.to_owned(),
                    stderr: if code == 1 {
                        "No such file or directory".to_owned()
                    } else {
                        String::new()
                    },
                    truncated: false,
                })
            }
        }
        let cancel = CancelToken::new();
        let c = collect(
            &Fake {
                cache: Some("940\n"),
            },
            &cancel,
        )
        .await
        .unwrap();
        assert_eq!((c.cache_age.ok(), c.age.ok()), (Some(60), Some(900)));
        // Missing cache mtime: cache age unknown, apt stamp age still known.
        let c = collect(&Fake { cache: None }, &cancel).await.unwrap();
        assert_eq!(c.cache_age, Err(ProbeFailure::Missing));
        assert_eq!(c.age.as_ref().ok(), Some(&900));
        let check = security_pending(&c.pending, &c.cache_age, &c.age, AT);
        assert_eq!(check.outcome, Outcome::Incomplete);
    }

    #[test]
    fn missing_or_unparsable_updates_file_is_never_a_pass() {
        let c = sp(&Err(ProbeFailure::Missing), &Ok(DAY));
        assert_eq!(c.outcome, Outcome::Unavailable);
        assert!(!c.limitations.is_empty());
        let c = sp(&Err(ProbeFailure::Error("bad".into())), &Ok(DAY));
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
