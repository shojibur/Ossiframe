//! The `firewall` step: `ENABLED` in `/etc/ufw/ufw.conf` as configuration
//! evidence only. Effective rules need root, and nftables, iptables and cloud
//! firewalls are not visible, so the effective firewall state is never claimed.
use crate::model::{Category, CheckResult, Outcome, Severity};
use crate::probe::{ProbeFailure, run_probe};
use ossiframe_core::{CancelToken, ExecError, Executor};

pub const UFW_CONF: &str = "/etc/ufw/ufw.conf";

const NOTE: &str = "Configuration file only. Effective firewall rules need root, and nftables, \
    iptables and cloud firewalls are not visible, so the effective firewall state is not known.";

/// Last `ENABLED=` assignment wins, as when the file is sourced. `None` when
/// there is no `ENABLED` line or its value is not `yes` or `no`.
pub fn parse_enabled(text: &str) -> Option<bool> {
    let mut value = None;
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("ENABLED") else {
            continue;
        };
        let Some(v) = rest.trim_start().strip_prefix('=') else {
            continue;
        };
        let v = v.trim().trim_matches(['"', '\'']).to_ascii_lowercase();
        value = match v.as_str() {
            "yes" => Some(true),
            "no" => Some(false),
            _ => None,
        };
    }
    value
}

/// Reads `ufw.conf`. `Err` only for cancel and connection loss.
pub async fn collect(
    exec: &dyn Executor,
    cancel: &CancelToken,
) -> Result<Result<String, ProbeFailure>, ExecError> {
    run_probe(exec, cancel, "cat", &[UFW_CONF]).await
}

/// `fw.ufw-config`. Also returns the parsed value for step evidence.
pub fn ufw_config(
    read: &Result<String, ProbeFailure>,
    observed_at: &str,
) -> (CheckResult, Option<bool>) {
    let title = "UFW configuration";
    let text = match read {
        Ok(t) => t,
        Err(f) => {
            let c = f
                .into_check(
                    "fw.ufw-config",
                    Category::Security,
                    title,
                    false,
                    observed_at,
                )
                .resource(UFW_CONF)
                .limitation(NOTE);
            return (c, None);
        }
    };
    match parse_enabled(text) {
        Some(true) => (
            CheckResult::new(
                "fw.ufw-config",
                Category::Security,
                Outcome::Passed,
                title,
                "UFW is enabled in its configuration file.",
                observed_at,
            )
            .resource(UFW_CONF)
            .evidence(["ENABLED=yes"])
            .limitation(NOTE),
            Some(true),
        ),
        Some(false) => (
            CheckResult::finding(
                "fw.ufw-config",
                Category::Security,
                Severity::Low,
                title,
                "UFW is disabled in its configuration file.",
                observed_at,
            )
            .resource(UFW_CONF)
            .evidence(["ENABLED=no"])
            .recommendation(
                "If no other firewall is in use, consider enabling UFW after allowing SSH.",
            )
            .limitation(NOTE),
            Some(false),
        ),
        None => (
            CheckResult::new(
                "fw.ufw-config",
                Category::Security,
                Outcome::Error,
                title,
                "Not assessed: no recognizable ENABLED=yes or ENABLED=no line was found.",
                observed_at,
            )
            .resource(UFW_CONF)
            .limitation(NOTE),
            None,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: &str = "2025-01-01T00:00:00Z";

    #[test]
    fn enabled_values() {
        assert_eq!(
            parse_enabled("# c\nENABLED=yes\nLOGLEVEL=low\n"),
            Some(true)
        );
        assert_eq!(parse_enabled("ENABLED=no"), Some(false));
        assert_eq!(parse_enabled("ENABLED = \"YES\""), Some(true));
        assert_eq!(parse_enabled("#ENABLED=yes\n"), None);
        assert_eq!(parse_enabled("ENABLED=maybe"), None);
        assert_eq!(parse_enabled("ENABLED_X=yes"), None);
        assert_eq!(parse_enabled("ENABLED=no\nENABLED=yes"), Some(true));
        assert_eq!(parse_enabled(""), None);
    }

    #[test]
    fn rule_outcomes_never_claim_effective_state() {
        let (c, v) = ufw_config(&Ok("ENABLED=yes\n".to_owned()), AT);
        assert_eq!((c.outcome, v), (Outcome::Passed, Some(true)));
        assert!(c.limitations[0].contains("need root"));
        assert!(!c.explanation.contains("active"));
        let (c, _) = ufw_config(&Ok("ENABLED=no\n".to_owned()), AT);
        assert_eq!(
            (c.outcome, c.severity),
            (Outcome::Finding, Some(Severity::Low))
        );
        for f in [ProbeFailure::Missing, ProbeFailure::PermissionDenied] {
            let (c, v) = ufw_config(&Err(f), AT);
            assert_eq!((c.outcome, v), (Outcome::Unavailable, None));
        }
        let (c, _) = ufw_config(&Ok("junk".to_owned()), AT);
        assert_eq!(c.outcome, Outcome::Error);
        let (c, _) = ufw_config(&Err(ProbeFailure::Truncated), AT);
        assert_eq!(c.outcome, Outcome::Error);
    }
}
