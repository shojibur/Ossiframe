//! Operation history: operations, steps and events. Nothing here stores a
//! secret. Operations are never deleted with the server profile.

use super::{Store, StoreError, now};
use crate::ipc::cap;
use ossiframe_core::OperationState;
use ossiframe_core::redact::finalize_output;
use rusqlite::{OptionalExtension, params};

/// Maximum stored output per step.
pub const STEP_OUTPUT_CAP: usize = 64 * 1024;
/// Maximum events kept per operation. The oldest are dropped first.
pub const MAX_EVENTS_PER_OPERATION: usize = 200;
/// Maximum operations returned by [`Store::list_operations`].
pub const MAX_LIST: usize = 50;
const MAX_EVENT_DETAIL: usize = 1024;

/// Name used in the database for a state. Matches the serde names.
pub fn state_name(s: OperationState) -> &'static str {
    match s {
        OperationState::Pending => "pending",
        OperationState::Running => "running",
        OperationState::Verifying => "verifying",
        OperationState::Succeeded => "succeeded",
        OperationState::Failed => "failed",
        OperationState::Cancelled => "cancelled",
        OperationState::UnknownOutcome => "unknownOutcome",
    }
}

pub fn parse_state(s: &str) -> Option<OperationState> {
    OperationState::ALL
        .into_iter()
        .find(|st| state_name(*st) == s)
}

/// Identity of the server an operation was approved for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationIdentity {
    pub server_id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub host_key_sha256: String,
}

#[derive(Debug, Clone)]
pub struct NewOperation {
    pub id: String,
    pub identity: OperationIdentity,
    pub kind: String,
    pub plan_digest: String,
    pub plan_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationRecord {
    pub id: String,
    /// `None` after the profile was removed.
    pub server_id: Option<String>,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub host_key_sha256: String,
    pub kind: String,
    pub plan_digest: String,
    pub plan_json: String,
    pub state: OperationState,
    pub cancel_requested: bool,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub error: Option<String>,
    pub verification_json: Option<String>,
    pub resolution_json: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepRecord {
    pub idx: u32,
    pub step_id: String,
    pub title: String,
    pub unit_name: Option<String>,
    pub status: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub exit_code: Option<i32>,
    pub output: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRecord {
    pub kind: String,
    pub from_state: Option<OperationState>,
    pub to_state: Option<OperationState>,
    pub unit_name: Option<String>,
    pub detail: Option<String>,
    pub at: String,
}

const OP_COLS: &str = "id, server_id, server_name, server_host, server_port, server_user, host_key_sha256, kind, plan_digest, plan_json, status, cancel_requested, created_at, started_at, finished_at, error, verification_json, resolution_json";

fn row_to_op(r: &rusqlite::Row<'_>) -> rusqlite::Result<OperationRecord> {
    let status: String = r.get(10)?;
    Ok(OperationRecord {
        id: r.get(0)?,
        server_id: r.get(1)?,
        name: r.get(2)?,
        host: r.get(3)?,
        port: r.get(4)?,
        user: r.get(5)?,
        host_key_sha256: r.get(6)?,
        kind: r.get(7)?,
        plan_digest: r.get(8)?,
        plan_json: r.get(9)?,
        // An unknown status is treated as needing review, never as finished.
        state: parse_state(&status).unwrap_or(OperationState::UnknownOutcome),
        cancel_requested: r.get::<_, i64>(11)? != 0,
        created_at: r.get(12)?,
        started_at: r.get(13)?,
        finished_at: r.get(14)?,
        error: r.get(15)?,
        verification_json: r.get(16)?,
        resolution_json: r.get(17)?,
    })
}

impl Store {
    /// Random id of this installation (the desktop identity in lock records).
    pub fn installation_id(&self) -> Result<String, StoreError> {
        Ok(self.db().query_row(
            "SELECT value FROM meta WHERE key='installation_id'",
            [],
            |r| r.get(0),
        )?)
    }

    /// Records a new Pending operation with its identity snapshot.
    pub fn insert_operation(&self, op: &NewOperation) -> Result<(), StoreError> {
        let i = &op.identity;
        self.db().execute(
            "INSERT INTO operations (id, server_id, server_name, server_host, server_port, server_user, host_key_sha256, kind, plan_digest, plan_json, status, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'pending', ?11)",
            params![
                op.id,
                i.server_id,
                i.name,
                i.host,
                i.port,
                i.user,
                i.host_key_sha256,
                op.kind,
                op.plan_digest,
                op.plan_json,
                now()
            ],
        )?;
        Ok(())
    }

    /// Sets the state and the (capped) error text. A finished operation
    /// (Succeeded, Failed, Cancelled) is never changed again.
    pub fn update_operation_status(
        &self,
        id: &str,
        state: OperationState,
        error: Option<&str>,
    ) -> Result<(), StoreError> {
        let ts = now();
        let started = (state == OperationState::Running).then_some(ts.as_str());
        let finished = state.is_terminal().then_some(ts.as_str());
        let n = self.db().execute(
            "UPDATE operations SET status=?2, error=?3,
                 started_at=COALESCE(started_at, ?4), finished_at=COALESCE(?5, finished_at)
             WHERE id=?1 AND status NOT IN ('succeeded','failed','cancelled')",
            params![id, state_name(state), error.map(cap), started, finished],
        )?;
        if n == 0 {
            return Err(StoreError::Db(
                "operation not found or already finished".into(),
            ));
        }
        Ok(())
    }

    pub fn set_cancel_requested(&self, id: &str) -> Result<(), StoreError> {
        let n = self
            .db()
            .execute("UPDATE operations SET cancel_requested=1 WHERE id=?1", [id])?;
        if n == 0 {
            Err(StoreError::NotFound)
        } else {
            Ok(())
        }
    }

    /// Stores the verification result (checks and verdicts as JSON).
    pub fn set_verification_json(&self, id: &str, json: &str) -> Result<(), StoreError> {
        self.set_json_column("verification_json", id, json)
    }

    /// Stores the user decision and evidence of a manual resolution.
    pub fn set_resolution_json(&self, id: &str, json: &str) -> Result<(), StoreError> {
        self.set_json_column("resolution_json", id, json)
    }

    fn set_json_column(&self, column: &str, id: &str, json: &str) -> Result<(), StoreError> {
        // `column` is one of two fixed names chosen above, never user input.
        let n = self.db().execute(
            &format!("UPDATE operations SET {column}=?2 WHERE id=?1"),
            params![id, json],
        )?;
        if n == 0 {
            Err(StoreError::NotFound)
        } else {
            Ok(())
        }
    }

    /// Inserts or replaces a step row. Output is not touched; use
    /// [`Store::replace_step_output`].
    pub fn upsert_step(&self, operation_id: &str, step: &StepRecord) -> Result<(), StoreError> {
        self.db().execute(
            "INSERT INTO operation_steps (operation_id, idx, step_id, title, unit_name, status, started_at, finished_at, exit_code)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(operation_id, idx) DO UPDATE SET
               step_id=excluded.step_id, title=excluded.title, unit_name=excluded.unit_name,
               status=excluded.status, started_at=excluded.started_at,
               finished_at=excluded.finished_at, exit_code=excluded.exit_code",
            params![
                operation_id,
                step.idx,
                step.step_id,
                step.title,
                step.unit_name,
                step.status,
                step.started_at,
                step.finished_at,
                step.exit_code
            ],
        )?;
        Ok(())
    }

    /// Replaces the stored output of a step with redacted, valid UTF-8 text
    /// of at most [`STEP_OUTPUT_CAP`] bytes.
    pub fn replace_step_output(
        &self,
        operation_id: &str,
        idx: u32,
        raw: &[u8],
        secrets: &[&str],
    ) -> Result<(), StoreError> {
        let (text, _) = finalize_output(raw, false, secrets, STEP_OUTPUT_CAP);
        let n = self.db().execute(
            "UPDATE operation_steps SET output=?3 WHERE operation_id=?1 AND idx=?2",
            params![operation_id, idx, text],
        )?;
        if n == 0 {
            Err(StoreError::NotFound)
        } else {
            Ok(())
        }
    }

    pub fn get_steps(&self, operation_id: &str) -> Result<Vec<StepRecord>, StoreError> {
        let db = self.db();
        let mut stmt = db.prepare(
            "SELECT idx, step_id, title, unit_name, status, started_at, finished_at, exit_code, output
             FROM operation_steps WHERE operation_id=?1 ORDER BY idx",
        )?;
        let rows = stmt
            .query_map([operation_id], |r| {
                Ok(StepRecord {
                    idx: r.get(0)?,
                    step_id: r.get(1)?,
                    title: r.get(2)?,
                    unit_name: r.get(3)?,
                    status: r.get(4)?,
                    started_at: r.get(5)?,
                    finished_at: r.get(6)?,
                    exit_code: r.get(7)?,
                    output: r.get(8)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Appends an event and drops the oldest ones beyond the per-operation
    /// cap. `detail` is capped and must be plain text.
    pub fn append_event(
        &self,
        operation_id: &str,
        server_label: &str,
        event: &EventRecord,
    ) -> Result<(), StoreError> {
        let mut db = self.db();
        let tx = db.transaction()?;
        tx.execute(
            "INSERT INTO operation_events (operation_id, server_label, at, kind, from_state, to_state, unit_name, detail)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                operation_id,
                server_label,
                now(),
                event.kind,
                event.from_state.map(state_name),
                event.to_state.map(state_name),
                event.unit_name,
                event.detail.as_deref().map(|d| bounded(d, MAX_EVENT_DETAIL)),
            ],
        )?;
        tx.execute(
            "DELETE FROM operation_events WHERE operation_id=?1 AND id NOT IN
               (SELECT id FROM operation_events WHERE operation_id=?1 ORDER BY id DESC LIMIT ?2)",
            params![operation_id, MAX_EVENTS_PER_OPERATION as i64],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn get_events(&self, operation_id: &str) -> Result<Vec<EventRecord>, StoreError> {
        let db = self.db();
        let mut stmt = db.prepare(
            "SELECT kind, from_state, to_state, unit_name, detail, at
             FROM operation_events WHERE operation_id=?1 ORDER BY id",
        )?;
        let rows = stmt
            .query_map([operation_id], |r| {
                let from: Option<String> = r.get(1)?;
                let to: Option<String> = r.get(2)?;
                Ok(EventRecord {
                    kind: r.get(0)?,
                    from_state: from.as_deref().and_then(parse_state),
                    to_state: to.as_deref().and_then(parse_state),
                    unit_name: r.get(3)?,
                    detail: r.get(4)?,
                    at: r.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_operation(&self, id: &str) -> Result<OperationRecord, StoreError> {
        self.db()
            .query_row(
                &format!("SELECT {OP_COLS} FROM operations WHERE id=?1"),
                [id],
                row_to_op,
            )
            .optional()?
            .ok_or(StoreError::NotFound)
    }

    /// Newest first. `limit` is clamped to [`MAX_LIST`]. With `server_id`,
    /// only that server's operations are returned.
    pub fn list_operations(
        &self,
        server_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<OperationRecord>, StoreError> {
        let limit = limit.clamp(1, MAX_LIST);
        let db = self.db();
        let mut stmt = db.prepare(&format!(
            "SELECT {OP_COLS} FROM operations WHERE (?1 IS NULL OR server_id=?1)
             ORDER BY created_at DESC, rowid DESC LIMIT ?2"
        ))?;
        let rows = stmt
            .query_map(params![server_id, limit as i64], row_to_op)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Startup recovery. The app may have died while remote work was running,
    /// so Pending, Running and Verifying operations become UnknownOutcome.
    /// Nothing is retried. Returns the recovered operation ids.
    pub fn recover_in_flight_operations(&self) -> Result<Vec<String>, StoreError> {
        let mut db = self.db();
        let tx = db.transaction()?;
        let rows: Vec<(String, String, String)> = {
            let mut stmt = tx.prepare(
                "SELECT id, status, server_name FROM operations
                 WHERE status IN ('pending','running','verifying')",
            )?;
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<Result<Vec<_>, _>>()?
        };
        for (id, status, label) in &rows {
            tx.execute(
                "UPDATE operations SET status='unknownOutcome' WHERE id=?1",
                [id],
            )?;
            tx.execute(
                "INSERT INTO operation_events (operation_id, server_label, at, kind, from_state, to_state, detail)
                 VALUES (?1, ?2, ?3, 'recovery', ?4, 'unknownOutcome', 'the app stopped while this operation was in progress; remote work may still be running')",
                params![id, label, now(), status],
            )?;
        }
        tx.commit()?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }
}

fn bounded(s: &str, max: usize) -> String {
    let mut end = max.min(s.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests::record;

    fn new_op(id: &str, server: &str) -> NewOperation {
        NewOperation {
            id: id.into(),
            identity: OperationIdentity {
                server_id: server.into(),
                name: "web".into(),
                host: "example.com".into(),
                port: 22,
                user: "root".into(),
                host_key_sha256: "SHA256:abc".into(),
            },
            kind: "provision".into(),
            plan_digest: "0".repeat(64),
            plan_json: "{}".into(),
        }
    }

    fn step(idx: u32) -> StepRecord {
        StepRecord {
            idx,
            step_id: "apt-update".into(),
            title: "Update".into(),
            unit_name: None,
            status: "running".into(),
            started_at: None,
            finished_at: None,
            exit_code: None,
            output: None,
        }
    }

    #[test]
    fn startup_recovery_marks_only_in_flight_rows() {
        let s = Store::open_in_memory().unwrap();
        s.insert(&record("a")).unwrap();
        let states = [
            ("pending", OperationState::Pending),
            ("running", OperationState::Running),
            ("verifying", OperationState::Verifying),
            ("succeeded", OperationState::Succeeded),
            ("failed", OperationState::Failed),
            ("cancelled", OperationState::Cancelled),
            ("unknown", OperationState::UnknownOutcome),
        ];
        for (id, st) in states {
            s.insert_operation(&new_op(id, "a")).unwrap();
            // Pending is the insert state; walk others there directly.
            s.db()
                .execute(
                    "UPDATE operations SET status=?2 WHERE id=?1",
                    params![id, state_name(st)],
                )
                .unwrap();
        }
        let mut recovered = s.recover_in_flight_operations().unwrap();
        recovered.sort();
        assert_eq!(recovered, ["pending", "running", "verifying"]);
        for (id, st) in states {
            let expect = match st {
                OperationState::Pending | OperationState::Running | OperationState::Verifying => {
                    OperationState::UnknownOutcome
                }
                other => other,
            };
            assert_eq!(s.get_operation(id).unwrap().state, expect, "{id}");
        }
        assert_eq!(s.get_events("running").unwrap()[0].kind, "recovery");
        assert!(s.recover_in_flight_operations().unwrap().is_empty());
    }

    #[test]
    fn finished_operation_status_is_never_changed() {
        let s = Store::open_in_memory().unwrap();
        s.insert(&record("a")).unwrap();
        s.insert_operation(&new_op("o", "a")).unwrap();
        s.update_operation_status("o", OperationState::Running, None)
            .unwrap();
        let long = "x".repeat(5000);
        s.update_operation_status("o", OperationState::Failed, Some(&long))
            .unwrap();
        let op = s.get_operation("o").unwrap();
        assert_eq!(op.error.unwrap().len(), crate::ipc::MAX_MESSAGE);
        assert!(op.started_at.is_some() && op.finished_at.is_some());
        assert!(
            s.update_operation_status("o", OperationState::Running, None)
                .is_err()
        );
    }

    #[test]
    fn step_output_is_capped_on_a_char_boundary_and_valid_utf8() {
        let s = Store::open_in_memory().unwrap();
        s.insert(&record("a")).unwrap();
        s.insert_operation(&new_op("o", "a")).unwrap();
        s.upsert_step("o", &step(0)).unwrap();
        // Two-byte characters so the cap falls inside one only if mishandled.
        let mut raw = "é".repeat(STEP_OUTPUT_CAP).into_bytes();
        raw.extend_from_slice(&[0xff, 0xfe]);
        s.replace_step_output("o", 0, &raw, &["é"]).unwrap();
        let out = s.get_steps("o").unwrap()[0].output.clone().unwrap();
        assert!(out.len() <= STEP_OUTPUT_CAP);
        assert!(out.starts_with("[REDACTED]"));
        s.replace_step_output("o", 0, &[b'a'; 10], &[]).unwrap();
        assert_eq!(
            s.get_steps("o").unwrap()[0].output.as_deref(),
            Some("aaaaaaaaaa")
        );
        let odd = "é".repeat(STEP_OUTPUT_CAP / 2 + 1);
        s.replace_step_output("o", 0, odd.as_bytes(), &[]).unwrap();
        let out = s.get_steps("o").unwrap()[0].output.clone().unwrap();
        assert_eq!(out.len(), STEP_OUTPUT_CAP, "cut on a character boundary");
        assert!(s.replace_step_output("o", 9, b"x", &[]).is_err());
    }

    #[test]
    fn events_are_capped_per_operation_and_keep_the_newest() {
        let s = Store::open_in_memory().unwrap();
        s.insert(&record("a")).unwrap();
        s.insert_operation(&new_op("o", "a")).unwrap();
        s.insert_operation(&new_op("p", "a")).unwrap();
        let ev = |n: usize| EventRecord {
            kind: format!("e{n}"),
            from_state: None,
            to_state: None,
            unit_name: None,
            detail: Some("é".repeat(2000)),
            at: String::new(),
        };
        s.append_event("p", "web", &ev(0)).unwrap();
        for n in 0..MAX_EVENTS_PER_OPERATION + 5 {
            s.append_event("o", "web", &ev(n)).unwrap();
        }
        let events = s.get_events("o").unwrap();
        assert_eq!(events.len(), MAX_EVENTS_PER_OPERATION);
        assert_eq!(events[0].kind, "e5");
        assert!(events[0].detail.as_ref().unwrap().len() <= MAX_EVENT_DETAIL);
        assert_eq!(
            s.get_events("p").unwrap().len(),
            1,
            "other operation untouched"
        );
    }

    #[test]
    fn list_is_limited_to_50_newest_first() {
        let s = Store::open_in_memory().unwrap();
        s.insert(&record("a")).unwrap();
        for n in 0..60 {
            s.insert_operation(&new_op(&format!("o{n:02}"), "a"))
                .unwrap();
        }
        let all = s.list_operations(None, 1000).unwrap();
        assert_eq!(all.len(), MAX_LIST);
        assert_eq!(all[0].id, "o59");
        assert_eq!(s.list_operations(Some("a"), 3).unwrap().len(), 3);
        assert!(s.list_operations(Some("zzz"), 3).unwrap().is_empty());
    }

    #[test]
    fn installation_id_is_stable_random_hex() {
        let s = Store::open_in_memory().unwrap();
        let id = s.installation_id().unwrap();
        assert_eq!(id.len(), 32);
        assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(s.installation_id().unwrap(), id);
        assert_ne!(
            Store::open_in_memory().unwrap().installation_id().unwrap(),
            id
        );
    }
}
