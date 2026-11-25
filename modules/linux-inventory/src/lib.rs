//! Read-only Linux server inventory built from predefined commands.
//!
//! This crate depends only on `ossiframe-core`.
mod inventory;
pub mod parse;
pub mod services;
pub mod sockets;
pub mod software;
pub use inventory::*;

use async_trait::async_trait;
use ossiframe_core::{
    Module, ModuleId, ModuleManifest, OperationContext, OperationError, OperationInfo,
};

const OPERATIONS: &[OperationInfo] = &[
    OperationInfo {
        id: "server-info",
        description: "Hostname, OS, kernel, CPU, memory and root disk usage",
    },
    OperationInfo {
        id: "services",
        description: "systemd services: installed, loaded, active and enablement state",
    },
    OperationInfo {
        id: "listening-sockets",
        description: "TCP and UDP listening sockets with binding class and visible process",
    },
    OperationInfo {
        id: "software",
        description: "Server software known to the dpkg package database",
    },
];

/// The `linux-inventory` module. Its only operation, `server-info`, takes no
/// arguments and runs the predefined read-only probes.
pub struct LinuxInventory;

#[async_trait]
impl Module for LinuxInventory {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            id: ModuleId::parse("linux-inventory").expect("valid module id"),
            name: "Linux inventory".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "Read-only Linux server inventory".to_owned(),
            dependencies: Vec::new(),
        }
    }

    fn operations(&self) -> &'static [OperationInfo] {
        OPERATIONS
    }

    async fn run(
        &self,
        operation: &str,
        args: serde_json::Value,
        ctx: &OperationContext<'_>,
    ) -> Result<serde_json::Value, OperationError> {
        match operation {
            "server-info" => {
                if !args.is_null() {
                    return Err(OperationError::InvalidArguments(
                        "server-info takes no arguments".to_owned(),
                    ));
                }
                let info = collect(ctx.exec, ctx.cancel, ctx.observed_at.clone()).await?;
                serde_json::to_value(info)
                    .map_err(|_| OperationError::Failed("could not encode result".to_owned()))
            }
            "services" => {
                if !args.is_null() {
                    return Err(OperationError::InvalidArguments(
                        "services takes no arguments".to_owned(),
                    ));
                }
                let info = services::collect(ctx.exec, ctx.cancel, ctx.observed_at.clone()).await?;
                serde_json::to_value(info)
                    .map_err(|_| OperationError::Failed("could not encode result".to_owned()))
            }
            "listening-sockets" => {
                if !args.is_null() {
                    return Err(OperationError::InvalidArguments(
                        "listening-sockets takes no arguments".to_owned(),
                    ));
                }
                let info = sockets::collect(ctx.exec, ctx.cancel, ctx.observed_at.clone()).await?;
                serde_json::to_value(info)
                    .map_err(|_| OperationError::Failed("could not encode result".to_owned()))
            }
            "software" => {
                if !args.is_null() {
                    return Err(OperationError::InvalidArguments(
                        "software takes no arguments".to_owned(),
                    ));
                }
                let info = software::collect(ctx.exec, ctx.cancel, ctx.observed_at.clone()).await?;
                serde_json::to_value(info)
                    .map_err(|_| OperationError::Failed("could not encode result".to_owned()))
            }
            _ => Err(OperationError::Failed("unknown operation".to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ossiframe_core::{CancelToken, CommandSpec, ExecError, ExecResult, Executor};

    struct Unreachable;

    #[async_trait]
    impl Executor for Unreachable {
        async fn run(&self, _: &CommandSpec, _: &CancelToken) -> Result<ExecResult, ExecError> {
            panic!("executor must not be reached");
        }
    }

    #[tokio::test]
    async fn non_null_args_are_rejected_before_any_command_runs() {
        let cancel = CancelToken::new();
        let ctx = OperationContext {
            exec: &Unreachable,
            cancel: &cancel,
            observed_at: String::new(),
        };
        for op in ["server-info", "services", "listening-sockets", "software"] {
            for args in [serde_json::json!({}), serde_json::json!("x")] {
                let err = LinuxInventory.run(op, args, &ctx).await.unwrap_err();
                assert!(matches!(err, OperationError::InvalidArguments(_)));
            }
        }
    }

    #[test]
    fn manifest_has_one_operation_and_no_dependencies() {
        let m = LinuxInventory.manifest();
        assert_eq!(m.id.as_str(), "linux-inventory");
        assert!(m.dependencies.is_empty());
        assert_eq!(LinuxInventory.operations()[0].id, "server-info");
    }
}
