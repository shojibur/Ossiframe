//! Shared probe helper: runs one predefined command and maps failures to the
//! outcome a check should report. Cancel and connection loss abort the step;
//! everything else becomes a result.

use crate::model::{Category, CheckResult, Outcome};
use ossiframe_core::{CancelToken, CommandSpec, ExecError, Executor};
use std::time::Duration;

/// Timeout for each audit command.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Why a probe produced no usable output. Carries no raw output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeFailure {
    /// The SSH user may not read the data.
    PermissionDenied,
    /// The file or program does not exist.
    Missing,
    /// The output was cut at the executor size limit. Reported as `error`
    /// by default; readers that can work with partial coverage map it to
    /// `incomplete` themselves.
    Truncated,
    /// Unexpected exit status, timeout or channel error.
    Error(String),
}

impl ProbeFailure {
    /// `Missing` is `notApplicable` when `missing_is_not_applicable` is set
    /// (the inspected subject legitimately does not exist), else `unavailable`.
    pub fn outcome(&self, missing_is_not_applicable: bool) -> Outcome {
        match self {
            ProbeFailure::PermissionDenied => Outcome::Unavailable,
            ProbeFailure::Missing if missing_is_not_applicable => Outcome::NotApplicable,
            ProbeFailure::Missing => Outcome::Unavailable,
            ProbeFailure::Truncated | ProbeFailure::Error(_) => Outcome::Error,
        }
    }

    /// Short reason for the explanation text.
    pub fn reason(&self) -> &str {
        match self {
            ProbeFailure::PermissionDenied => "permission denied",
            ProbeFailure::Missing => "file or command not found",
            ProbeFailure::Truncated => "output was truncated",
            ProbeFailure::Error(m) => m,
        }
    }

    /// Builds the check that reports this failure. Never `passed`.
    pub fn into_check(
        &self,
        rule_id: &'static str,
        category: Category,
        title: &str,
        missing_is_not_applicable: bool,
        observed_at: &str,
    ) -> CheckResult {
        CheckResult::new(
            rule_id,
            category,
            self.outcome(missing_is_not_applicable),
            title,
            format!("Not assessed: {}.", self.reason()),
            observed_at,
        )
    }
}

/// Runs `program args` with [`PROBE_TIMEOUT`]. `Err` only for cancel and
/// connection loss. `Ok(Err(_))` is a failed probe with a short reason.
/// `sudo` is refused.
pub async fn run_probe(
    exec: &dyn Executor,
    cancel: &CancelToken,
    program: &str,
    args: &[&str],
) -> Result<Result<String, ProbeFailure>, ExecError> {
    if program == "sudo" {
        return Ok(Err(ProbeFailure::Error(
            "privileged commands are not allowed".to_owned(),
        )));
    }
    let Ok(spec) = CommandSpec::new(program, args, PROBE_TIMEOUT) else {
        return Ok(Err(ProbeFailure::Error("invalid command".to_owned())));
    };
    match exec.run(&spec, cancel).await {
        Ok(out) => {
            if !out.exit.success() {
                let err = out.stderr.to_ascii_lowercase();
                return Ok(Err(if err.contains("permission denied") {
                    ProbeFailure::PermissionDenied
                } else if err.contains("no such file") || err.contains("not found") {
                    ProbeFailure::Missing
                } else {
                    ProbeFailure::Error("command failed".to_owned())
                }));
            }
            if out.truncated {
                return Ok(Err(ProbeFailure::Truncated));
            }
            Ok(Ok(out.stdout))
        }
        Err(ExecError::Timeout) => Ok(Err(ProbeFailure::Error("command timed out".to_owned()))),
        Err(ExecError::Channel(_)) => Ok(Err(ProbeFailure::Error("channel error".to_owned()))),
        Err(e @ (ExecError::Cancelled | ExecError::ConnectionLost)) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use ossiframe_core::{ExecResult, ExitStatus};

    struct Fake(Result<ExecResult, ExecError>);

    #[async_trait]
    impl Executor for Fake {
        async fn run(&self, _: &CommandSpec, _: &CancelToken) -> Result<ExecResult, ExecError> {
            self.0.clone()
        }
    }

    fn result(code: i32, stdout: &str, stderr: &str, truncated: bool) -> Fake {
        Fake(Ok(ExecResult {
            exit: ExitStatus::Code(code),
            stdout: stdout.to_owned(),
            stderr: stderr.to_owned(),
            truncated,
        }))
    }

    async fn probe(f: Fake) -> Result<Result<String, ProbeFailure>, ExecError> {
        run_probe(&f, &CancelToken::new(), "cat", &["/etc/x"]).await
    }

    #[tokio::test]
    async fn success_returns_stdout() {
        assert_eq!(
            probe(result(0, "ok", "", false)).await.unwrap().unwrap(),
            "ok"
        );
    }

    #[tokio::test]
    async fn permission_denied_is_unavailable() {
        let f = probe(result(1, "", "cat: /x: Permission denied", false))
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(f, ProbeFailure::PermissionDenied);
        assert_eq!(f.outcome(false), Outcome::Unavailable);
        assert_eq!(f.outcome(true), Outcome::Unavailable);
    }

    #[tokio::test]
    async fn missing_file_is_not_applicable_or_unavailable() {
        let f = probe(result(1, "", "cat: /x: No such file or directory", false))
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(f, ProbeFailure::Missing);
        assert_eq!(f.outcome(true), Outcome::NotApplicable);
        assert_eq!(f.outcome(false), Outcome::Unavailable);
        let c = f.into_check("r.x", Category::Security, "t", false, "now");
        assert_eq!(c.outcome, Outcome::Unavailable);
        assert!(c.severity.is_none());
    }

    #[tokio::test]
    async fn bad_exit_truncated_timeout_and_channel_are_errors_without_raw_output() {
        let secret = "SECRET-RAW-OUTPUT";
        for f in [
            result(2, secret, secret, false),
            result(0, secret, "", true),
            Fake(Err(ExecError::Timeout)),
            Fake(Err(ExecError::Channel(secret.to_owned()))),
        ] {
            let failure = probe(f).await.unwrap().unwrap_err();
            assert_eq!(failure.outcome(false), Outcome::Error);
            assert!(!failure.reason().contains(secret));
        }
    }

    #[tokio::test]
    async fn cancel_and_connection_loss_abort() {
        for e in [ExecError::Cancelled, ExecError::ConnectionLost] {
            let got = probe(Fake(Err(e.clone()))).await.unwrap_err();
            assert_eq!(got, e);
        }
    }

    #[tokio::test]
    async fn sudo_is_refused_without_running() {
        let f = Fake(Err(ExecError::ConnectionLost));
        let got = run_probe(&f, &CancelToken::new(), "sudo", &["-n", "true"])
            .await
            .unwrap();
        assert!(matches!(got, Err(ProbeFailure::Error(_))));
    }
}
