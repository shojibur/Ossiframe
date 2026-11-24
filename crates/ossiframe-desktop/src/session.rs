//! In-memory registry of live SSH sessions, with a per-profile guard so
//! concurrent connect, disconnect, edit, remove and inventory calls for the
//! same profile run one at a time.

use async_trait::async_trait;
use ossiframe_core::{CancelToken, Executor};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// A live connection: an executor that can report and end its state.
#[async_trait]
pub trait Connection: Executor {
    fn is_closed(&self) -> bool;
    async fn close(&self);
}

#[async_trait]
impl Connection for ossiframe_ssh::SshExecutor {
    fn is_closed(&self) -> bool {
        ossiframe_ssh::SshExecutor::is_closed(self)
    }
    async fn close(&self) {
        self.disconnect().await;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConnState {
    Disconnected,
    Connecting,
    Connected,
    Error,
}

/// Notifications for the UI layer. Kept free of Tauri types.
pub trait SessionEvents: Send + Sync {
    fn state_changed(&self, server_id: &str, state: ConnState);
    fn connection_lost(&self, server_id: &str);
}

pub struct NoEvents;
impl SessionEvents for NoEvents {
    fn state_changed(&self, _: &str, _: ConnState) {}
    fn connection_lost(&self, _: &str) {}
}

#[derive(Default)]
pub struct Sessions {
    live: Mutex<HashMap<String, Arc<dyn Connection>>>,
    guards: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    ops: Mutex<HashMap<String, CancelToken>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Sessions {
    /// The per-profile guard. Hold it for the whole operation.
    pub fn guard(&self, server_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        lock(&self.guards)
            .entry(server_id.to_owned())
            .or_default()
            .clone()
    }

    /// Returns the live connection, or `None` if there is none. A closed
    /// connection is removed and reported through `lost`.
    pub fn get(&self, server_id: &str, events: &dyn SessionEvents) -> Option<Arc<dyn Connection>> {
        let mut live = lock(&self.live);
        let conn = live.get(server_id)?.clone();
        if conn.is_closed() {
            live.remove(server_id);
            drop(live);
            events.connection_lost(server_id);
            events.state_changed(server_id, ConnState::Disconnected);
            return None;
        }
        Some(conn)
    }

    pub fn is_connected(&self, server_id: &str, events: &dyn SessionEvents) -> bool {
        self.get(server_id, events).is_some()
    }

    pub fn insert(&self, server_id: &str, conn: Arc<dyn Connection>) {
        lock(&self.live).insert(server_id.to_owned(), conn);
    }

    /// Removes and closes the session, if any.
    pub async fn close(&self, server_id: &str) {
        let conn = lock(&self.live).remove(server_id);
        if let Some(c) = conn {
            c.close().await;
        }
    }

    /// Starts a cancellable operation for a profile.
    pub fn begin_op(&self, server_id: &str) -> CancelToken {
        let t = CancelToken::new();
        lock(&self.ops).insert(server_id.to_owned(), t.clone());
        t
    }

    pub fn end_op(&self, server_id: &str) {
        lock(&self.ops).remove(server_id);
    }

    /// Cancels the running operation without waiting for the profile guard.
    pub fn cancel_op(&self, server_id: &str) {
        if let Some(t) = lock(&self.ops).remove(server_id) {
            t.cancel();
        }
    }

    pub fn forget(&self, server_id: &str) {
        lock(&self.guards).remove(server_id);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ossiframe_core::{CommandSpec, ExecError, ExecResult, ExitStatus};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// Fake connection used by service tests.
    #[derive(Default)]
    pub struct FakeConn {
        pub closed: AtomicBool,
        pub runs: AtomicUsize,
    }

    #[async_trait]
    impl Executor for FakeConn {
        async fn run(&self, _: &CommandSpec, _: &CancelToken) -> Result<ExecResult, ExecError> {
            self.runs.fetch_add(1, Ordering::SeqCst);
            Ok(ExecResult {
                exit: ExitStatus::Code(0),
                stdout: String::new(),
                stderr: String::new(),
                truncated: false,
            })
        }
    }

    #[async_trait]
    impl Connection for FakeConn {
        fn is_closed(&self) -> bool {
            self.closed.load(Ordering::SeqCst)
        }
        async fn close(&self) {
            self.closed.store(true, Ordering::SeqCst);
        }
    }

    #[derive(Default)]
    pub struct RecordingEvents {
        pub lost: Mutex<Vec<String>>,
    }
    impl SessionEvents for RecordingEvents {
        fn state_changed(&self, _: &str, _: ConnState) {}
        fn connection_lost(&self, id: &str) {
            lock(&self.lost).push(id.into());
        }
    }

    #[test]
    fn closed_handles_are_removed_and_reported() {
        let s = Sessions::default();
        let ev = RecordingEvents::default();
        let c = Arc::new(FakeConn::default());
        s.insert("a", c.clone());
        assert!(s.is_connected("a", &ev));
        c.closed.store(true, Ordering::SeqCst);
        assert!(!s.is_connected("a", &ev));
        assert_eq!(*lock(&ev.lost), vec!["a".to_string()]);
        assert!(s.get("a", &ev).is_none());
    }

    #[tokio::test]
    async fn guard_serializes_calls() {
        let s = Arc::new(Sessions::default());
        let inside = Arc::new(AtomicUsize::new(0));
        let max = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..4 {
            let (s, inside, max) = (s.clone(), inside.clone(), max.clone());
            handles.push(tokio::spawn(async move {
                let g = s.guard("a");
                let _l = g.lock().await;
                let n = inside.fetch_add(1, Ordering::SeqCst) + 1;
                max.fetch_max(n, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                inside.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        assert_eq!(max.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn cancel_op_cancels_the_token() {
        let s = Sessions::default();
        let t = s.begin_op("a");
        s.cancel_op("a");
        assert!(t.is_cancelled());
    }
}
