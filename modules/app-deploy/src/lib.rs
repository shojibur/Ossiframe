//! Application deployment on Ubuntu servers. Depends only on
//! `ossiframe-core`. The module registers no `OperationInfo`: all work goes
//! through the engine's typed commands, never through `Registry::run`.
//! Application types are data in [`app_type`]; a new type is added here only.

pub mod app_type;
pub mod create;
pub mod env;
pub mod git_access;
pub mod identity;
pub mod paths;
pub mod preflight;
pub mod release;
pub mod remove;
pub mod verify;
pub mod write;

use async_trait::async_trait;
use ossiframe_core::{
    Module, ModuleId, ModuleManifest, OperationContext, OperationError, OperationInfo,
};

const NO_OPERATIONS: &[OperationInfo] = &[];

/// The `app-deploy` module.
pub struct AppDeploy;

#[async_trait]
impl Module for AppDeploy {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            id: ModuleId::parse("app-deploy").expect("valid module id"),
            name: "Application deployment".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "Create, deploy, roll back and remove applications".to_owned(),
            dependencies: Vec::new(),
        }
    }

    fn operations(&self) -> &'static [OperationInfo] {
        NO_OPERATIONS
    }

    async fn run(
        &self,
        operation: &str,
        _args: serde_json::Value,
        _ctx: &OperationContext<'_>,
    ) -> Result<serde_json::Value, OperationError> {
        Err(OperationError::InvalidArguments(format!(
            "unknown operation: {operation}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_registers_zero_operations() {
        assert!(AppDeploy.operations().is_empty());
        assert_eq!(AppDeploy.manifest().id.as_str(), "app-deploy");
    }
}
