//! Application deployment (S05) for Ossiframe Deploy: the typed `app_*`
//! commands, the provisioner for application plans and the sink that keeps
//! the local app and release records in step with operations.
//!
//! This is app-level code. `ossiframe-desktop` is Core-level and does not
//! depend on the app-deploy module; the generic engine, plan store and store
//! tables are reused from it.
pub mod commands;
pub mod dto;
pub mod events;
pub mod plans;
pub mod provisioner;

pub use commands::{APP_COMMAND_NAMES, Apps};

use ossiframe_core::Provisioner;
use ossiframe_desktop::provisioning::engine::OperationEvents;
use ossiframe_desktop::provisioning::{self, Provisioning};
use std::sync::Arc;
use tauri::{Manager, Runtime};

/// Registers provisioning with the application-aware provisioner and event
/// sink, runs startup recovery, and registers the `app_*` command state.
/// Call after `tauri_support::setup`.
pub fn setup<R: Runtime>(app: &mut tauri::App<R>) -> Result<(), Box<dyn std::error::Error>> {
    provisioning::setup_with(app, |store, ui| {
        let events: Arc<dyn OperationEvents> = Arc::new(events::AppEvents::new(ui, store.clone()));
        let provisioner: Arc<dyn Provisioner> =
            Arc::new(provisioner::DeployProvisioner::new(store));
        (provisioner, events)
    })?;
    let apps = Apps::from_provisioning(&app.state::<Provisioning>());
    app.manage(apps);
    Ok(())
}
