//! Ossiframe Core: shared types and contracts for building infrastructure
//! applications. This crate has no network, database, keychain or UI code.

pub mod command;
pub mod executor;
pub mod host_key;
pub mod ident;
pub mod module;
pub mod redact;
pub mod registry;
pub mod secret;

pub use command::CommandSpec;
pub use executor::{CancelToken, ExecError, ExecResult, Executor, ExitStatus};
pub use host_key::{HostKeyDecision, PinnedHostKey, PresentedHostKey};
pub use ident::{Hostname, KeyPath, Port, ServerName, SshUser, ValidationError};
pub use module::{
    Module, ModuleId, ModuleManifest, OperationContext, OperationError, OperationId, OperationInfo,
};
pub use registry::{Registry, RegistryBuilder, RegistryError, RunError};
pub use secret::SecretString;
