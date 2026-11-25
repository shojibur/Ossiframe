//! The remote execution contract implemented by transports such as
//! `ossiframe-ssh`.

use crate::command::{CommandSpec, Privilege};
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

/// Failures of a command run. Except for `PrivilegeDenied` (the command was
/// not run), the outcome is unknown: the remote process may still be running,
/// and callers must not retry automatically.
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
    /// The command needs root and the executor refused it. It was not run.
    #[error("privileged command was not allowed")]
    PrivilegeDenied,
}

#[async_trait]
pub trait Executor: Send + Sync {
    async fn run(&self, spec: &CommandSpec, cancel: &CancelToken) -> Result<ExecResult, ExecError>;
}

/// An [`Executor`] wrapper that refuses any command whose
/// [`Privilege`] is not `User`.
///
/// What it does: it inspects the `Privilege` field of each `CommandSpec`.
/// `Privilege::Root` and `Privilege::RunAs` return
/// `ExecError::PrivilegeDenied` and the wrapped executor is not called. `Privilege::User` is passed through unchanged.
///
/// What it does not do: it is not a command classifier and not a sandbox.
/// It does not stop a `User`-level command from changing files or state the
/// login user can change. It does not stop trusted Rust code from using a
/// different executor (for example the raw connection) to run `Root`
/// commands. It only guards code paths that are handed this wrapper.
pub struct ReadOnly<'a>(pub &'a dyn Executor);

#[async_trait]
impl Executor for ReadOnly<'_> {
    async fn run(&self, spec: &CommandSpec, cancel: &CancelToken) -> Result<ExecResult, ExecError> {
        if spec.privilege() != Privilege::User {
            return Err(ExecError::PrivilegeDenied);
        }
        self.0.run(spec, cancel).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    struct Recorder(AtomicUsize);

    #[async_trait]
    impl Executor for Recorder {
        async fn run(&self, _: &CommandSpec, _: &CancelToken) -> Result<ExecResult, ExecError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(ExecResult {
                exit: ExitStatus::Code(0),
                stdout: String::new(),
                stderr: String::new(),
                truncated: false,
            })
        }
    }

    #[tokio::test]
    async fn read_only_denies_root_and_run_as_and_passes_user() {
        let inner = Recorder(AtomicUsize::new(0));
        let guard = ReadOnly(&inner);
        let cancel = CancelToken::new();
        let t = Duration::from_secs(1);

        let root = CommandSpec::privileged("/usr/bin/true", &[], t).unwrap();
        assert_eq!(
            guard.run(&root, &cancel).await.unwrap_err(),
            ExecError::PrivilegeDenied
        );
        assert_eq!(inner.0.load(Ordering::SeqCst), 0);

        let who = crate::ident::LinuxUserName::parse("ossi-app").unwrap();
        let run_as = CommandSpec::run_as(who, "/usr/bin/true", &[], t).unwrap();
        assert_eq!(
            guard.run(&run_as, &cancel).await.unwrap_err(),
            ExecError::PrivilegeDenied
        );
        assert_eq!(inner.0.load(Ordering::SeqCst), 0);

        let user = CommandSpec::new("true", &[], t).unwrap();
        assert!(guard.run(&user, &cancel).await.unwrap().exit.success());
        assert_eq!(inner.0.load(Ordering::SeqCst), 1);
    }

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
