//! SSH transport for Ossiframe, built on russh.
//!
//! No russh type appears in this crate's public API. The host key is checked
//! during the handshake, before any credential is sent.

mod connect;
mod exec;

pub use connect::{AuthMethod, ConnectError, ConnectOptions, ConnectTarget, connect};
pub use exec::SshExecutor;
