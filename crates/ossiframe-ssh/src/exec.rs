use crate::connect::{CLEANUP_TIMEOUT, HostKeyHandler, close_handle};
use async_trait::async_trait;
use ossiframe_core::redact::{BoundedBuffer, OUTPUT_CAP};
use ossiframe_core::{CancelToken, CommandSpec, ExecError, ExecResult, Executor, ExitStatus};
use russh::client::{Handle, Msg};
use russh::{Channel, ChannelMsg};

/// Runs commands over an authenticated SSH connection.
///
/// Authentication secrets are not kept here: they are dropped after login
/// and are never part of a command, so they are not registered for output
/// redaction. Output still goes through bounded capture and whole-output
/// redaction; command-scoped secrets (sent over stdin) arrive with
/// privileged execution in a later spec.
pub struct SshExecutor {
    handle: Handle<HostKeyHandler>,
}

impl SshExecutor {
    pub(crate) fn new(handle: Handle<HostKeyHandler>) -> Self {
        Self { handle }
    }

    /// True when the underlying connection has closed (for example after
    /// unanswered keepalives).
    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }

    /// Sends a disconnect, waiting at most a short, fixed time.
    pub async fn disconnect(&self) {
        close_handle(&self.handle).await;
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

/// Opens the channel, submits the command and collects output. The channel
/// is stored in `slot` as soon as it exists, so the caller can close it if
/// this future is abandoned on timeout or cancel.
async fn exchange(
    handle: &Handle<HostKeyHandler>,
    command: String,
    slot: &mut Option<Channel<Msg>>,
    out: &mut BoundedBuffer,
    err: &mut BoundedBuffer,
) -> Result<Collected, russh::Error> {
    let ch = slot.insert(handle.channel_open_session().await?);
    ch.exec(true, command).await?;
    let _ = ch.eof().await;
    Ok(collect(ch, out, err).await)
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
    Done(Result<Collected, russh::Error>),
    Timeout,
    Cancelled,
}

#[async_trait]
impl Executor for SshExecutor {
    /// The timeout and the cancel token cover the whole operation: channel
    /// creation, command submission, output and exit status. A timeout or
    /// cancel does not mean the remote process stopped.
    async fn run(&self, spec: &CommandSpec, cancel: &CancelToken) -> Result<ExecResult, ExecError> {
        if self.handle.is_closed() {
            return Err(ExecError::ConnectionLost);
        }
        let deadline = tokio::time::Instant::now() + spec.timeout();
        let mut slot: Option<Channel<Msg>> = None;
        let mut out = BoundedBuffer::new(OUTPUT_CAP);
        let mut err = BoundedBuffer::new(OUTPUT_CAP);

        let outcome = tokio::select! {
            biased;
            _ = cancel.cancelled() => Outcome::Cancelled,
            _ = tokio::time::sleep_until(deadline) => Outcome::Timeout,
            r = exchange(&self.handle, spec.to_command_line(), &mut slot, &mut out, &mut err) => Outcome::Done(r),
        };

        let collected = match outcome {
            Outcome::Done(Ok(c)) => c,
            other => {
                // Best effort and bounded. Closing the channel does not prove
                // the remote process stopped.
                if let Some(ch) = slot.take() {
                    let _ = tokio::time::timeout(CLEANUP_TIMEOUT, ch.close()).await;
                }
                return Err(match other {
                    Outcome::Timeout => ExecError::Timeout,
                    Outcome::Cancelled => ExecError::Cancelled,
                    Outcome::Done(Err(_)) if self.handle.is_closed() => ExecError::ConnectionLost,
                    Outcome::Done(Err(e)) => {
                        ExecError::Channel(e.to_string().chars().take(200).collect())
                    }
                    Outcome::Done(Ok(_)) => unreachable!("handled above"),
                });
            }
        };

        let exit = end_state(collected.exit, self.handle.is_closed())?;
        let (stdout, t1) = out.finalize(&[]);
        let (stderr, t2) = err.finalize(&[]);
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
