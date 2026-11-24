//! The remote execution contract implemented by transports such as
//! `ossiframe-ssh`.

use crate::command::CommandSpec;
use async_trait::async_trait;

pub use tokio_util::sync::CancellationToken as CancelToken;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitStatus {
    Code(i32),
    Signal(String),
    /// The channel closed without reporting a status.
    Unknown,
}

impl ExitStatus {
    pub fn success(&self) -> bool {
        matches!(self, ExitStatus::Code(0))
    }
}

/// Result of a command that ran to completion. A non-zero exit code is still
/// an `ExecResult`; the caller decides whether it is a failure.
#[derive(Debug, Clone)]
pub struct ExecResult {
    pub exit: ExitStatus,
    /// Lossy UTF-8, redacted, bounded.
    pub stdout: String,
    /// Lossy UTF-8, redacted, bounded.
    pub stderr: String,
    /// True when either stream was cut to the output limit.
    pub truncated: bool,
}

/// Failures where the command's outcome is unknown. The remote process may
/// still be running after any of these; callers must not retry automatically.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExecError {
    #[error("command timed out; it may still be running on the server")]
    Timeout,
    #[error("command was cancelled; it may still be running on the server")]
    Cancelled,
    #[error("connection lost; the command may still be running on the server")]
    ConnectionLost,
    #[error("channel error: {0}")]
    Channel(String),
}

#[async_trait]
pub trait Executor: Send + Sync {
    async fn run(&self, spec: &CommandSpec, cancel: &CancelToken) -> Result<ExecResult, ExecError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct Fake(Result<ExecResult, ExecError>);

    #[async_trait]
    impl Executor for Fake {
        async fn run(
            &self,
            _: &CommandSpec,
            cancel: &CancelToken,
        ) -> Result<ExecResult, ExecError> {
            if cancel.is_cancelled() {
                return Err(ExecError::Cancelled);
            }
            self.0.clone()
        }
    }

    #[tokio::test]
    async fn failures_are_typed_and_non_zero_exit_is_ok() {
        let spec = CommandSpec::new("true", &[], Duration::from_secs(1)).unwrap();
        let cancel = CancelToken::new();
        for err in [ExecError::Timeout, ExecError::ConnectionLost] {
            assert_eq!(
                Fake(Err(err.clone()))
                    .run(&spec, &cancel)
                    .await
                    .unwrap_err(),
                err
            );
        }
        let r = ExecResult {
            exit: ExitStatus::Code(2),
            stdout: String::new(),
            stderr: "x".into(),
            truncated: false,
        };
        let got = Fake(Ok(r)).run(&spec, &cancel).await.unwrap();
        assert!(!got.exit.success());
        cancel.cancel();
        let e = Fake(Err(ExecError::Timeout))
            .run(&spec, &cancel)
            .await
            .unwrap_err();
        assert_eq!(e, ExecError::Cancelled);
    }
}
