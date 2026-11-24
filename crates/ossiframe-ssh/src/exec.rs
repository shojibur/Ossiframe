use crate::connect::HostKeyHandler;
use async_trait::async_trait;
use ossiframe_core::redact::{BoundedBuffer, OUTPUT_CAP};
use ossiframe_core::{
    CancelToken, CommandSpec, ExecError, ExecResult, Executor, ExitStatus, SecretString,
};
use russh::client::{Handle, Msg};
use russh::{Channel, ChannelMsg};

/// Runs commands over an authenticated SSH connection.
pub struct SshExecutor {
    handle: Handle<HostKeyHandler>,
    redact: Vec<SecretString>,
}

impl SshExecutor {
    pub(crate) fn new(handle: Handle<HostKeyHandler>) -> Self {
        Self {
            handle,
            redact: Vec::new(),
        }
    }

    /// Registers values that must be replaced in command output.
    pub fn with_redactions(mut self, secrets: Vec<SecretString>) -> Self {
        self.redact = secrets;
        self
    }

    /// True when the underlying connection has closed (for example after
    /// unanswered keepalives).
    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }

    pub async fn disconnect(&self) {
        let _ = self
            .handle
            .disconnect(russh::Disconnect::ByApplication, "", "en")
            .await;
    }
}

#[derive(Default)]
struct Collected {
    exit: Option<ExitStatus>,
}

async fn collect(
    ch: &mut Channel<Msg>,
    out: &mut BoundedBuffer,
    err: &mut BoundedBuffer,
) -> Collected {
    let mut c = Collected::default();
    while let Some(msg) = ch.wait().await {
        match msg {
            ChannelMsg::Data { data } => out.push(&data),
            ChannelMsg::ExtendedData { data, ext: 1 } => err.push(&data),
            ChannelMsg::ExitStatus { exit_status } => {
                c.exit = Some(ExitStatus::Code(
                    i32::try_from(exit_status).unwrap_or(i32::MAX),
                ));
            }
            ChannelMsg::ExitSignal { signal_name, .. } => {
                c.exit = Some(ExitStatus::Signal(format!("{signal_name:?}")))
            }
            ChannelMsg::Close => break,
            _ => {}
        }
    }
    c
}

/// How a channel that stopped producing messages should be reported.
pub(crate) fn end_state(
    exit: Option<ExitStatus>,
    connection_closed: bool,
) -> Result<ExitStatus, ExecError> {
    match exit {
        Some(e) => Ok(e),
        None if connection_closed => Err(ExecError::ConnectionLost),
        None => Ok(ExitStatus::Unknown),
    }
}

enum Outcome {
    Done(Collected),
    Timeout,
    Cancelled,
}

#[async_trait]
impl Executor for SshExecutor {
    async fn run(&self, spec: &CommandSpec, cancel: &CancelToken) -> Result<ExecResult, ExecError> {
        if self.handle.is_closed() {
            return Err(ExecError::ConnectionLost);
        }
        let lost = |e: russh::Error| {
            if self.handle.is_closed() {
                ExecError::ConnectionLost
            } else {
                ExecError::Channel(e.to_string())
            }
        };
        let mut ch = self.handle.channel_open_session().await.map_err(lost)?;
        ch.exec(true, spec.to_command_line()).await.map_err(lost)?;
        let _ = ch.eof().await;

        let mut out = BoundedBuffer::new(OUTPUT_CAP);
        let mut err = BoundedBuffer::new(OUTPUT_CAP);
        let outcome = tokio::select! {
            c = collect(&mut ch, &mut out, &mut err) => Outcome::Done(c),
            _ = tokio::time::sleep(spec.timeout()) => Outcome::Timeout,
            _ = cancel.cancelled() => Outcome::Cancelled,
        };
        let collected = match outcome {
            Outcome::Done(c) => c,
            // Best effort. Closing the channel does not prove the remote
            // process stopped.
            Outcome::Timeout => {
                let _ = ch.close().await;
                return Err(ExecError::Timeout);
            }
            Outcome::Cancelled => {
                let _ = ch.close().await;
                return Err(ExecError::Cancelled);
            }
        };

        let exit = end_state(collected.exit, self.handle.is_closed())?;
        let secrets: Vec<&str> = self.redact.iter().map(SecretString::expose).collect();
        let (stdout, t1) = out.finalize(&secrets);
        let (stderr, t2) = err.finalize(&secrets);
        Ok(ExecResult {
            exit,
            stdout,
            stderr,
            truncated: t1 || t2,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_states() {
        assert_eq!(
            end_state(Some(ExitStatus::Code(3)), true),
            Ok(ExitStatus::Code(3))
        );
        assert_eq!(end_state(None, true), Err(ExecError::ConnectionLost));
        assert_eq!(end_state(None, false), Ok(ExitStatus::Unknown));
    }
}
