fn main() {
    // Only these commands can be invoked, and only when a capability grants them.
    let commands = &[
        "list_servers",
        "add_server",
        "update_server",
        "remove_server",
        "connect_server",
        "disconnect_server",
        "trust_host_key",
        "replace_host_key",
        "get_server_info",
        "cancel_operation",
    ];
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(commands)),
    )
    .expect("failed to run tauri-build");
}
