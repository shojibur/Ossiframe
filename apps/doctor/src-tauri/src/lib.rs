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

    // Provisioning boundary (S04).
    //
    // `PROVISION_COMMAND_NAMES` only exists when `ossiframe-desktop` is built
    // with its `provisioning` feature, which Doctor must not enable. The names
    // are therefore read from the desktop crate's source text and cross-checked
    // against a hardcoded copy, so a renamed or added command fails here.
    //
    // Honest limits of this boundary:
    // - It rests on command registration (invoke handler, `build.rs` manifest,
    //   capability file), not on Cargo features. `cargo build --workspace`
    //   unifies features, so when Deploy is in the same build the
    //   `provisioning` code is compiled once and shared, including into the
    //   workspace artifacts for Doctor's library. Compiled-in code is not
    //   reachable from the frontend unless a command is registered and
    //   granted, and Doctor does neither. These tests check exactly that.
    // - They are text checks. They do not execute the Tauri runtime ACL.
    const HARDCODED_PROVISION_COMMANDS: &[&str] = &[
        "provision_components",
        "provision_preflight",
        "provision_create_plan",
        "provision_approve_plan",
        "provision_cancel",
        "provision_reconcile",
        "provision_mark_resolved",
        "provision_release_stale_lock",
        "provision_get_operation",
        "provision_list_operations",
    ];

    fn provision_names_from_desktop_source() -> Vec<String> {
        let src = include_str!("../../../../crates/ossiframe-desktop/src/provisioning/commands.rs");
        let start = src
            .find("pub const PROVISION_COMMAND_NAMES")
            .expect("PROVISION_COMMAND_NAMES present");
        let body = &src[start..];
        let end = body.find("];").expect("end of list");
        body[..end]
            .lines()
            .filter_map(|l| l.trim().strip_prefix('"'))
            .filter_map(|l| l.strip_suffix("\","))
            .map(str::to_owned)
            .collect()
    }

    /// Production part of this file: everything before the test module.
    fn production_source() -> &'static str {
        let src = include_str!("lib.rs");
        &src[..src.find("#[cfg(test)]").expect("test module marker")]
    }

    #[test]
    fn hardcoded_provision_names_match_desktop_source() {
        assert_eq!(
            provision_names_from_desktop_source(),
            HARDCODED_PROVISION_COMMANDS
        );
    }

    #[test]
    fn no_provision_command_in_capability_manifest_or_handler() {
        let capability = include_str!("../capabilities/main.json");
        let build = include_str!("../build.rs");
        let handler = production_source();
        for name in HARDCODED_PROVISION_COMMANDS {
            for (what, text) in [
                ("capability", capability),
                ("build.rs", build),
                ("invoke handler source", handler),
            ] {
                assert!(!text.contains(name), "{what} contains {name}");
                assert!(
                    !text.contains(&kebab(name)),
                    "{what} contains {}",
                    kebab(name)
                );
            }
        }
        // Also catch any other provisioning-flavoured name or setup call.
        for (what, text) in [
            ("capability", capability),
            ("build.rs", build),
            ("invoke handler source", handler),
        ] {
            assert!(!text.contains("provision"), "{what} mentions provision");
        }
    }

    #[test]
    fn manifest_does_not_enable_provisioning_or_depend_on_it() {
        let toml = include_str!("../Cargo.toml");
        assert!(
            !toml.contains("provisioning"),
            "Cargo.toml mentions provisioning"
        );
        assert!(!toml.contains("ossiframe-mod-ubuntu-provisioning"));
        assert!(!toml.contains("ossiframe-deploy"));
        // Doctor enables only the `tauri` feature of the desktop crate.
        let line = toml
            .lines()
            .find(|l| l.starts_with("ossiframe-desktop"))
            .expect("ossiframe-desktop dependency");
        assert!(line.contains("features = [\"tauri\"]"), "{line}");
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
