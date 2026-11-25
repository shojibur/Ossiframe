//! Read-only Linux server audit: eight steps that turn evidence into typed
//! check results. Depends on `linux-inventory` for discovery parsers and on
//! `ossiframe-core` for the module and executor contracts.
//!
//! The steps run only predefined `CommandSpec` commands, never `sudo`.
mod certs;
mod firewall;
pub mod model;
pub mod probe;
pub mod report;
mod rules;
mod sshd;
mod steps;
mod updates;

use async_trait::async_trait;
use ossiframe_core::{
    Module, ModuleId, ModuleManifest, OperationContext, OperationError, OperationInfo,
};

/// The steps in scan order. The order of this slice is the step order that
/// Doctor reads from `list_modules`.
pub const OPERATIONS: &[OperationInfo] = &[
    OperationInfo {
        id: "resources",
        description: "Memory and disk space checks",
    },
    OperationInfo {
        id: "services",
        description: "Failed systemd services",
    },
    OperationInfo {
        id: "network",
        description: "Database ports listening on non-loopback addresses",
    },
    OperationInfo {
        id: "software",
        description: "Server software known to the dpkg package database",
    },
    OperationInfo {
        id: "ssh",
        description: "Global SSH daemon settings from configuration files",
    },
    OperationInfo {
        id: "firewall",
        description: "UFW configuration file",
    },
    OperationInfo {
        id: "updates",
        description: "Cached update, metadata age and reboot-required state",
    },
    OperationInfo {
        id: "certificates",
        description: "Certificate expiry for certificates in readable nginx configuration",
    },
];

/// The `linux-audit` module. Every step takes `null` arguments.
pub struct LinuxAudit;

#[async_trait]
impl Module for LinuxAudit {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            id: ModuleId::parse("linux-audit").expect("valid module id"),
            name: "Linux audit".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "Read-only Linux server audit checks".to_owned(),
            dependencies: vec![ModuleId::parse("linux-inventory").expect("valid module id")],
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
        let Some(step) = OPERATIONS.iter().find(|o| o.id == operation) else {
            return Err(OperationError::Failed("unknown operation".to_owned()));
        };
        if !args.is_null() {
            return Err(OperationError::InvalidArguments(format!(
                "{} takes no arguments",
                step.id
            )));
        }
        let result = steps::run(step.id, ctx).await?;
        serde_json::to_value(result)
            .map_err(|_| OperationError::Failed("could not encode result".to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use ossiframe_core::{CancelToken, CommandSpec, ExecError, ExecResult, Executor};

    struct Unreachable;

    #[async_trait]
    impl Executor for Unreachable {
        async fn run(&self, _: &CommandSpec, _: &CancelToken) -> Result<ExecResult, ExecError> {
            panic!("executor must not be reached");
        }
    }

    #[test]
    fn manifest_depends_on_inventory_and_steps_are_in_order() {
        let m = LinuxAudit.manifest();
        assert_eq!(m.id.as_str(), "linux-audit");
        assert_eq!(m.dependencies.len(), 1);
        assert_eq!(m.dependencies[0].as_str(), "linux-inventory");
        let ids: Vec<&str> = LinuxAudit.operations().iter().map(|o| o.id).collect();
        assert_eq!(
            ids,
            [
                "resources",
                "services",
                "network",
                "software",
                "ssh",
                "firewall",
                "updates",
                "certificates"
            ]
        );
    }

    #[tokio::test]
    async fn non_null_args_are_rejected_before_any_command_runs() {
        let cancel = CancelToken::new();
        let ctx = OperationContext {
            exec: &Unreachable,
            cancel: &cancel,
            observed_at: String::new(),
        };
        for op in OPERATIONS {
            for args in [serde_json::json!({}), serde_json::json!("x")] {
                let err = LinuxAudit.run(op.id, args, &ctx).await.unwrap_err();
                assert!(
                    matches!(err, OperationError::InvalidArguments(_)),
                    "{}",
                    op.id
                );
            }
        }
        let err = LinuxAudit
            .run("nope", serde_json::Value::Null, &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, OperationError::Failed(_)));
    }

    struct Denied;

    #[async_trait]
    impl Executor for Denied {
        async fn run(&self, _: &CommandSpec, _: &CancelToken) -> Result<ExecResult, ExecError> {
            Ok(ExecResult {
                exit: ossiframe_core::ExitStatus::Code(1),
                stdout: String::new(),
                stderr: "Permission denied".to_owned(),
                truncated: false,
            })
        }
    }

    /// No step is a stub: each collects evidence, and with no readable data
    /// none of them reports `passed`.
    #[tokio::test]
    async fn all_eight_steps_are_implemented_and_never_pass_without_data() {
        let cancel = CancelToken::new();
        let ctx = OperationContext {
            exec: &Denied,
            cancel: &cancel,
            observed_at: "2025-01-01T00:00:00Z".to_owned(),
        };
        assert_eq!(OPERATIONS.len(), 8);
        for op in OPERATIONS {
            let v = LinuxAudit
                .run(op.id, serde_json::Value::Null, &ctx)
                .await
                .unwrap();
            assert_eq!(v["step"], op.id);
            assert!(!v["evidence"].is_null(), "{}", op.id);
            for c in v["checks"].as_array().unwrap() {
                assert_ne!(c["ruleId"], "step.not-implemented", "{}", op.id);
                assert_ne!(c["outcome"], "passed", "{}", op.id);
            }
        }
    }
}
