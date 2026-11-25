//! Ossiframe Core: shared types and contracts for building infrastructure
//! applications. This crate has no network, database, keychain or UI code.

pub mod command;
pub mod executor;
pub mod host_key;
pub mod ident;
pub mod module;
pub mod provision;
pub mod redact;
pub mod registry;
pub mod secret;

pub use command::{CommandSpec, Privilege};
pub use executor::{CancelToken, ExecError, ExecResult, Executor, ExitStatus, ReadOnly};
pub use host_key::{HostKeyDecision, PinnedHostKey, PresentedHostKey};
pub use ident::{
    AppSlug, BranchName, DomainName, GitUrl, Hostname, KeyPath, LinuxUserName, Port, ServerName,
    SshUser, ValidationError,
};
pub use module::{
    Module, ModuleId, ModuleManifest, OperationContext, OperationError, OperationId, OperationInfo,
};
pub use provision::{
    AppRecovery, AppRecoveryVerdict, CheckResult, CheckStatus, ComponentInfo, ComponentVerdict,
    Conflict, Drift, Event, FileChange, IllegalTransition, OperationState, PackageChange,
    PackageState, Plan, PlanError, PlanStep, PlanTarget, Preconditions, PreflightReport,
    PriorOperation, Provisioner, Resources, ServerClass, ServiceEffect, Severity, StepMode,
    SudoMode, next,
};
pub use registry::{Registry, RegistryBuilder, RegistryError, RunError};
pub use secret::SecretString;
