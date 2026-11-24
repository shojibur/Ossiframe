fn main() {
    // Only these commands can be invoked, and only when a capability grants them.
    // Keep in sync with `ossiframe_desktop::tauri_support::COMMAND_NAMES`
    // (a unit test in src/lib.rs checks this list and the capability file).
    let commands = &[
        "list_servers",
        "add_server",
        "update_server",
        "remove_server",
        "connect_server",
        "disconnect_server",
        "trust_host_key",
        "replace_host_key",
        "cancel_operation",
        "run_operation",
        "list_modules",
    ];
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(commands)),
    )
    .expect("failed to run tauri-build");
}
