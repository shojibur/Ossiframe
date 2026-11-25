fn main() {
    // Only these commands can be invoked, and only when a capability grants them.
    // Keep in sync with `ossiframe_desktop::tauri_support::COMMAND_NAMES` and
    // `ossiframe_desktop::provisioning::PROVISION_COMMAND_NAMES` and
    // `apps::APP_COMMAND_NAMES` (a unit test in src/lib.rs checks this list
    // and the capability file).
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
        // Provisioning.
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
        // Application deployment (S05). Deploy only.
        "app_types",
        "app_list",
        "app_get",
        "app_create_plan",
        "app_redeploy_plan",
        "app_rollback_plan",
        "app_remove_plan",
        "app_approve_plan",
        "app_get_env",
        "app_set_env",
        "app_list_releases",
        "app_deploy_key_status",
        "app_check_access",
    ];
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(commands)),
    )
    .expect("failed to run tauri-build");
}
