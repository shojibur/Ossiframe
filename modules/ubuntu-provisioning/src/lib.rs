//! Provisioning of a base stack on Ubuntu servers. Depends on
//! `ossiframe-core` (contracts) and `ossiframe-mod-linux-inventory`
//! (read-only facts). The module registers no `OperationInfo`: all
//! provisioning goes through the engine's typed commands and the
//! [`Provisioner`] contract, never through `Registry::run`.

pub mod components;
pub mod gate;
pub mod plan;
pub mod preflight;
pub mod recheck;
pub mod sources;
pub mod verify;

use async_trait::async_trait;
use ossiframe_core::{
    CancelToken, CheckResult, ComponentInfo, Drift, Executor, Module, ModuleId, ModuleManifest,
    OperationContext, OperationError, OperationInfo, Plan, PlanError, PreflightReport, Provisioner,
};

const NO_OPERATIONS: &[OperationInfo] = &[];

/// The `ubuntu-provisioning` module.
pub struct UbuntuProvisioning;

#[async_trait]
impl Module for UbuntuProvisioning {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            id: ModuleId::parse("ubuntu-provisioning").expect("valid module id"),
            name: "Ubuntu provisioning".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "Preflight, plan and install a base stack on Ubuntu servers".to_owned(),
            dependencies: vec![ModuleId::parse("linux-inventory").expect("valid module id")],
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

#[async_trait]
impl Provisioner for UbuntuProvisioning {
    fn components(&self) -> Vec<ComponentInfo> {
        components::infos()
    }

    /// Runs the preflight with the default selection and no prior-operation
    /// records, so any server with service-bearing packages is classified
    /// ExistingUnmanaged (fails closed). The engine calls
    /// [`preflight::preflight_with`] to pass the Store's records and the
    /// user's selection.
    async fn preflight(
        &self,
        exec: &dyn Executor,
        cancel: &CancelToken,
    ) -> Result<PreflightReport, OperationError> {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let input = preflight::PreflightInput::without_records(preflight::rfc3339_utc(secs));
        preflight::preflight_with(exec, cancel, &input)
            .await
            .map(|(_, report)| report)
    }

    async fn preflight_for(
        &self,
        exec: &dyn Executor,
        cancel: &CancelToken,
        selection: &[String],
        prior: &[ossiframe_core::PriorOperation],
    ) -> Result<PreflightReport, OperationError> {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let input = preflight::PreflightInput {
            selection: selection.to_vec(),
            observed_at: preflight::rfc3339_utc(secs),
            prior_operations: prior.to_vec(),
        };
        preflight::preflight_with(exec, cancel, &input)
            .await
            .map(|(_, report)| report)
    }

    fn plan(&self, report: &PreflightReport, selection: &[String]) -> Result<Plan, PlanError> {
        plan::plan(report, selection)
    }

    async fn recheck(
        &self,
        exec: &dyn Executor,
        cancel: &CancelToken,
        plan: &Plan,
    ) -> Result<(), Vec<Drift>> {
        recheck::run(exec, cancel, plan).await
    }

    async fn gate(
        &self,
        exec: &dyn Executor,
        cancel: &CancelToken,
        plan: &Plan,
        step_id: &str,
    ) -> Result<String, String> {
        gate::run(exec, cancel, plan, step_id).await
    }

    async fn verify(
        &self,
        exec: &dyn Executor,
        cancel: &CancelToken,
        plan: &Plan,
    ) -> Vec<CheckResult> {
        verify::run(exec, cancel, plan).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_declares_inventory_and_registers_zero_operations() {
        let m = UbuntuProvisioning;
        assert!(m.operations().is_empty());
        let manifest = m.manifest();
        assert_eq!(manifest.id.as_str(), "ubuntu-provisioning");
        let deps: Vec<_> = manifest.dependencies.iter().map(|d| d.as_str()).collect();
        assert_eq!(deps, ["linux-inventory"]);
    }

    #[test]
    fn components_lists_all_with_redis_optional() {
        let infos = UbuntuProvisioning.components();
        let ids: Vec<_> = infos.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "git", "nginx", "php", "php-fpm", "composer", "mysql", "redis"
            ]
        );
        let optional: Vec<_> = infos.iter().filter(|c| c.optional).collect();
        assert_eq!(optional.len(), 1);
        assert_eq!(optional[0].id, "redis");
    }
}
