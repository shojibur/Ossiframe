//! Ossiframe Deploy: server provisioning and application deployment.
//!
//! The frontend can only call the shared typed commands, the ten typed
//! `provision_*` commands from `ossiframe-desktop` and the thirteen typed
//! `app_*` commands of this app. None of them accepts a program, shell
//! command, path, URL or privilege level. Modules are `linux-inventory`,
//! `ubuntu-provisioning` and `app-deploy` (the last one is reached only
//! through the `app_*` commands).
pub mod apps;

use ossiframe_core::Registry;
use ossiframe_desktop::provisioning::commands as provision;
use ossiframe_desktop::tauri_support::{AppSetup, commands, setup};
use ossiframe_mod_linux_inventory::LinuxInventory;
use ossiframe_mod_ubuntu_provisioning::UbuntuProvisioning;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let registry = Registry::builder()
        .register(LinuxInventory)
        .register(UbuntuProvisioning)
        .build()
        .expect("module registry");
    tauri::Builder::default()
        .setup(move |app| {
            setup(
                app,
                AppSetup {
                    db_file: "deploy.sqlite3",
                    registry,
                },
            )?;
            apps::setup(app)
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_servers,
            commands::add_server,
            commands::update_server,
            commands::remove_server,
            commands::connect_server,
            commands::disconnect_server,
            commands::trust_host_key,
            commands::replace_host_key,
            commands::cancel_operation,
            commands::run_operation,
            commands::list_modules,
            provision::provision_components,
            provision::provision_preflight,
            provision::provision_create_plan,
            provision::provision_approve_plan,
            provision::provision_cancel,
            provision::provision_reconcile,
            provision::provision_mark_resolved,
            provision::provision_release_stale_lock,
            provision::provision_get_operation,
            provision::provision_list_operations,
            apps::commands::app_types,
            apps::commands::app_list,
            apps::commands::app_get,
            apps::commands::app_create_plan,
            apps::commands::app_redeploy_plan,
            apps::commands::app_rollback_plan,
            apps::commands::app_remove_plan,
            apps::commands::app_approve_plan,
            apps::commands::app_get_env,
            apps::commands::app_set_env,
            apps::commands::app_list_releases,
            apps::commands::app_deploy_key_status,
            apps::commands::app_check_access,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Ossiframe Deploy");
}

#[cfg(test)]
mod tests {
    use crate::apps::APP_COMMAND_NAMES;
    use ossiframe_desktop::provisioning::PROVISION_COMMAND_NAMES;
    use ossiframe_desktop::tauri_support::COMMAND_NAMES;
    use std::collections::BTreeSet;

    fn kebab(name: &str) -> String {
        name.replace('_', "-")
    }

    fn expected_commands() -> BTreeSet<String> {
        COMMAND_NAMES
            .iter()
            .chain(PROVISION_COMMAND_NAMES)
            .chain(APP_COMMAND_NAMES)
            .map(|n| kebab(n))
            .collect()
    }

    fn permissions() -> Vec<String> {
        let text = include_str!("../capabilities/main.json");
        let json: serde_json::Value = serde_json::from_str(text).expect("capability json");
        json["permissions"]
            .as_array()
            .expect("permissions array")
            .iter()
            .map(|p| p.as_str().expect("permission string").to_owned())
            .collect()
    }

    #[test]
    fn capability_allows_exactly_the_shared_provision_and_app_commands() {
        let allowed: BTreeSet<String> = permissions()
            .iter()
            .filter_map(|p| p.strip_prefix("allow-"))
            .map(str::to_owned)
            .collect();
        assert_eq!(allowed, expected_commands());
        assert_eq!(
            permissions().len(),
            expected_commands().len() + 1,
            "only core:event:default besides the allow- entries"
        );
    }

    #[test]
    fn build_manifest_lists_exactly_the_shared_provision_and_app_commands() {
        let text = include_str!("../build.rs");
        for name in COMMAND_NAMES
            .iter()
            .chain(PROVISION_COMMAND_NAMES)
            .chain(APP_COMMAND_NAMES)
        {
            assert!(
                text.contains(&format!("\"{name}\"")),
                "build.rs lacks {name}"
            );
        }
        assert_eq!(
            text.matches("        \"").count(),
            COMMAND_NAMES.len() + PROVISION_COMMAND_NAMES.len() + APP_COMMAND_NAMES.len()
        );
    }

    #[test]
    fn capability_has_no_dialog_fs_or_shell_permissions() {
        for p in permissions() {
            assert!(
                !p.starts_with("dialog:") && !p.starts_with("fs:") && !p.starts_with("shell:"),
                "forbidden permission {p}"
            );
        }
    }

    #[test]
    fn manifest_has_no_doctor_or_audit_dependency_and_no_dialog_plugin() {
        let toml = include_str!("../Cargo.toml");
        assert!(!toml.contains("ossiframe-doctor"));
        assert!(!toml.contains("linux-audit"));
        assert!(!toml.contains("tauri-plugin"));
    }

    #[test]
    fn app_identity_is_deploy_specific() {
        let conf = include_str!("../tauri.conf.json");
        assert!(conf.contains("\"identifier\": \"dev.ossiframe.deploy\""));
        assert!(conf.contains("\"freezePrototype\": true"));
        assert!(conf.contains("default-src 'none'"));
    }
}
