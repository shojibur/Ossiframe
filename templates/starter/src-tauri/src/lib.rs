//! {{product_name}}: an Ossiframe app.
//!
//! The frontend can only call the shared typed commands from
//! `ossiframe-desktop`. None of them accepts a program, shell command, file
//! path to read, URL or privilege level. Modules are trusted compile-time code
//! registered below.
use ossiframe_core::Registry;
{{provision_uses}}
use ossiframe_desktop::tauri_support::{AppSetup, commands, setup};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let registry = Registry::builder()
        // Generator insertion point: one `.register(...)` line per selected module.
        {{module_registrations}}
        .build()
        .expect("module registry");

    tauri::Builder::default()
        .setup(move |app| {
            setup(
                app,
                AppSetup {
                    db_file: "app.sqlite3",
                    registry,
                },
            ){{setup_try}}
            {{provision_setup}}
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
            {{provision_handlers}}
        ])
        .run(tauri::generate_context!())
        .expect("error while running {{product_name}}");
}

#[cfg(test)]
mod tests {
    {{provision_test_uses}}
    use ossiframe_desktop::tauri_support::COMMAND_NAMES;
    use std::collections::BTreeSet;

    fn kebab(name: &str) -> String {
        name.replace('_', "-")
    }

    #[test]
    fn capability_allows_exactly_the_shared_commands() {
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
        let expected: BTreeSet<String> = {{command_names_iter}}.map(|n| kebab(n)).collect();
        assert_eq!(allowed, expected);
    }

    #[test]
    fn build_manifest_lists_exactly_the_shared_commands() {
        let text = include_str!("../build.rs");
        for name in {{command_names_loop}} {
            assert!(
                text.contains(&format!("\"{name}\"")),
                "build.rs lacks {name}"
            );
        }
        assert_eq!(text.matches("        \"").count(), {{command_names_len}});
    }
}
