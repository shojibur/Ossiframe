//! Desktop foundation for Ossiframe applications: local storage, keychain
//! access, session management, host key approvals and IPC data types.
//!
//! This crate has no Tauri dependency so that its logic can be reused and
//! tested on its own. Apps register Tauri commands that call into it.

pub mod ipc;
pub mod service;
pub mod session;
pub mod store;
pub mod tokens;
pub mod vault;

pub use service::{ConnectOutcome, Connector, ServerService, ServiceError, SshConnector};
pub use session::{ConnState, Connection, SessionEvents};
pub use store::Store;
pub use vault::{KeychainVault, SecretVault};
