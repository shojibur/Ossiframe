//! Ossiframe Doctor: read-only server discovery.
//!
//! The frontend can only call the shared typed commands from
//! `ossiframe-desktop`. None of them accepts a program, shell command, file
//! path to read, URL or privilege level. Doctor adds two report commands that
//! take a typed report request and never a path or file content. Its modules
//! are `linux-inventory` and `linux-audit`.
pub mod report_commands;

use ossiframe_core::Registry;
use ossiframe_desktop::tauri_support::{AppSetup, commands, setup};
use ossiframe_mod_linux_audit::LinuxAudit;
use ossiframe_mod_linux_inventory::LinuxInventory;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let registry = Registry::builder()
        .register(LinuxInventory)
        .register(LinuxAudit)
        .build()
        .expect("module registry");
    tauri::Builder::default()
        // Rust API only. No window is granted `dialog:` permissions, so the
        // plugin's JavaScript commands cannot be invoked.
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            setup(
                app,
                AppSetup {
                    db_file: "doctor.sqlite3",
                    registry,
                },
            )
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
            report_commands::build_report,
            report_commands::save_report,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Ossiframe Doctor");
}

#[cfg(test)]
mod tests {
    use ossiframe_desktop::tauri_support::COMMAND_NAMES;
    use std::collections::BTreeSet;

    fn kebab(name: &str) -> String {
        name.replace('_', "-")
    }

    const DOCTOR_COMMANDS: &[&str] = &["build_report", "save_report"];

    fn expected_commands() -> BTreeSet<String> {
        COMMAND_NAMES
            .iter()
            .chain(DOCTOR_COMMANDS)
            .map(|n| kebab(n))
            .collect()
    }

    #[test]
    fn capability_allows_exactly_the_shared_and_report_commands() {
        let text = include_str!("../capabilities/main.json");
        let json: serde_json::Value = serde_json::from_str(text).expect("capability json");
        let allowed: BTreeSet<String> = json["permissions"]
            .as_array()
            .expect("permissions array")
            .iter()
            .filter_map(|p| p.as_str())
            .filter_map(|p| p.strip_prefix("allow-"))
            .map(str::to_owned)
            .collect();
        assert_eq!(allowed, expected_commands());
    }

    #[test]
    fn build_manifest_lists_exactly_the_shared_and_report_commands() {
        let text = include_str!("../build.rs");
        for name in COMMAND_NAMES.iter().chain(DOCTOR_COMMANDS) {
            assert!(
                text.contains(&format!("\"{name}\"")),
                "build.rs lacks {name}"
            );
        }
        assert_eq!(
            text.matches("        \"").count(),
            COMMAND_NAMES.len() + DOCTOR_COMMANDS.len()
        );
    }

    #[test]
    fn capability_has_no_dialog_or_fs_permissions() {
        let text = include_str!("../capabilities/main.json");
        let json: serde_json::Value = serde_json::from_str(text).expect("capability json");
        for p in json["permissions"].as_array().expect("permissions array") {
            let p = p.as_str().expect("permission string");
            assert!(
                !p.starts_with("dialog:") && !p.starts_with("fs:"),
                "forbidden permission {p}"
            );
        }
    }
}
