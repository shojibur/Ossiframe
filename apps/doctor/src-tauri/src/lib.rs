//! Ossiframe Doctor: read-only server discovery.
//!
//! The frontend can only call the typed commands below. None of them accepts
//! a program, shell command, file path to read, URL or privilege level.

mod commands;
mod inventory;

use ossiframe_desktop::session::{ConnState, SessionEvents};
use ossiframe_desktop::{KeychainVault, ServerService, SshConnector, Store};
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};

pub const KEYCHAIN_SERVICE: &str = "dev.ossiframe.doctor";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StatePayload<'a> {
    server_id: &'a str,
    state: ConnState,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LostPayload<'a> {
    server_id: &'a str,
}

struct TauriEvents(AppHandle);

impl SessionEvents for TauriEvents {
    fn state_changed(&self, server_id: &str, state: ConnState) {
        let _ = self
            .0
            .emit("connection-state", StatePayload { server_id, state });
    }
    fn connection_lost(&self, server_id: &str) {
        let _ = self.0.emit("connection-lost", LostPayload { server_id });
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let store = Store::open(&dir.join("doctor.sqlite3"))
                .map_err(|e| format!("cannot open local database: {e}"))?;
            let service = ServerService::new(
                store,
                Arc::new(KeychainVault::new(KEYCHAIN_SERVICE)),
                Arc::new(SshConnector::default()),
                Arc::new(TauriEvents(app.handle().clone())),
            );
            app.manage(service);
            Ok(())
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
            commands::get_server_info,
            commands::cancel_operation,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Ossiframe Doctor");
}
