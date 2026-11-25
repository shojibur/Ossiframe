//! Local SQLite storage for server profiles and pinned host keys.
//! No secret is ever stored here; `secret_ref` is an opaque keychain id.

use ossiframe_core::host_key::{PinnedHostKey, PresentedHostKey};
use ossiframe_core::{Hostname, KeyPath, Port, ServerName, SshUser};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("server not found")]
    NotFound,
    #[error("a host key is already pinned for this server")]
    AlreadyPinned,
    /// The server has an operation that is pending, running, verifying or
    /// has an unknown outcome.
    #[error("the server has an unresolved operation")]
    InUse,
    #[error("storage error: {0}")]
    Db(String),
}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError::Db(e.to_string().chars().take(200).collect())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthConfig {
    KeyFile(KeyPath),
    Password,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerRecord {
    pub id: String,
    pub name: ServerName,
    pub host: Hostname,
    pub port: Port,
    pub username: SshUser,
    pub auth: AuthConfig,
    pub secret_ref: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

const MIGRATIONS: &[&str] = &[
    // 1: servers and host keys. No secret column by design.
    "CREATE TABLE servers (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        host TEXT NOT NULL,
        port INTEGER NOT NULL,
        username TEXT NOT NULL,
        auth_kind TEXT NOT NULL CHECK (auth_kind IN ('key_file','password')),
        key_path TEXT,
        secret_ref TEXT,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    );
    CREATE TABLE host_keys (
        server_id TEXT PRIMARY KEY REFERENCES servers(id) ON DELETE CASCADE,
        key_type TEXT NOT NULL,
        public_key BLOB NOT NULL,
        sha256 TEXT NOT NULL,
        pinned_at TEXT NOT NULL
    );",
    // 2: operation history, shared by every app (the tables stay empty in
    // apps that never provision). History survives profile removal: the
    // server reference is nulled and the identity snapshot stays. No secret
    // column by design.
    "CREATE TABLE operations (
        id TEXT PRIMARY KEY,
        server_id TEXT REFERENCES servers(id) ON DELETE SET NULL,
        server_name TEXT NOT NULL,
        server_host TEXT NOT NULL,
        server_port INTEGER NOT NULL,
        server_user TEXT NOT NULL,
        host_key_sha256 TEXT NOT NULL,
        kind TEXT NOT NULL,
        plan_digest TEXT NOT NULL,
        plan_json TEXT NOT NULL,
        status TEXT NOT NULL,
        cancel_requested INTEGER NOT NULL DEFAULT 0,
        created_at TEXT NOT NULL,
        started_at TEXT,
        finished_at TEXT,
        error TEXT,
        verification_json TEXT,
        resolution_json TEXT
    );
    CREATE TABLE operation_events (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        operation_id TEXT REFERENCES operations(id) ON DELETE CASCADE,
        server_label TEXT NOT NULL,
        at TEXT NOT NULL,
        kind TEXT NOT NULL,
        from_state TEXT,
        to_state TEXT,
        unit_name TEXT,
        detail TEXT
    );
    CREATE TABLE operation_steps (
        operation_id TEXT NOT NULL REFERENCES operations(id) ON DELETE CASCADE,
        idx INTEGER NOT NULL,
        step_id TEXT NOT NULL,
        title TEXT NOT NULL,
        unit_name TEXT,
        status TEXT NOT NULL,
        started_at TEXT,
        finished_at TEXT,
        exit_code INTEGER,
        output TEXT,
        PRIMARY KEY (operation_id, idx)
    );
    CREATE INDEX operations_server ON operations(server_id, created_at);
    CREATE TABLE meta (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    INSERT INTO meta (key, value) VALUES ('installation_id', lower(hex(randomblob(16))));",
];

/// Statuses in which a server is in use. Must match the camelCase names of
/// `ossiframe_core::OperationState`.
const UNRESOLVED_SQL: &str = "('pending','running','verifying','unknownOutcome')";

#[cfg(feature = "provisioning")]
mod operations;
#[cfg(feature = "provisioning")]
pub use operations::*;

pub struct Store {
    conn: Mutex<Connection>,
}

pub(crate) fn now() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self, StoreError> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        migrate(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn db(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn list(&self) -> Result<Vec<ServerRecord>, StoreError> {
        let db = self.db();
        let mut stmt = db.prepare(&format!(
            "SELECT {COLS} FROM servers ORDER BY name COLLATE NOCASE"
        ))?;
        let rows = stmt
            .query_map([], row_to_raw)?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter().map(RawRow::into_record).collect()
    }

    pub fn get(&self, id: &str) -> Result<ServerRecord, StoreError> {
        let db = self.db();
        let raw = db
            .query_row(
                &format!("SELECT {COLS} FROM servers WHERE id = ?1"),
                [id],
                row_to_raw,
            )
            .optional()?
            .ok_or(StoreError::NotFound)?;
        raw.into_record()
    }

    pub fn insert(&self, r: &ServerRecord) -> Result<(), StoreError> {
        let (kind, path) = auth_cols(&r.auth);
        self.db().execute(
            "INSERT INTO servers (id, name, host, port, username, auth_kind, key_path, secret_ref, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![r.id, r.name.as_str(), r.host.as_str(), r.port.get(), r.username.as_str(), kind, path, r.secret_ref, r.created_at, r.updated_at],
        )?;
        Ok(())
    }

    /// Updates a profile. With `clear_pin`, the pinned host key is removed in
    /// the same transaction (used when the endpoint changes).
    pub fn update(&self, r: &ServerRecord, clear_pin: bool) -> Result<(), StoreError> {
        let (kind, path) = auth_cols(&r.auth);
        let mut db = self.db();
        let tx = db.transaction()?;
        let old: Option<(String, u32, String, String, Option<String>)> = tx
            .query_row(
                "SELECT host, port, username, auth_kind, key_path FROM servers WHERE id=?1",
                [&r.id],
                |x| Ok((x.get(0)?, x.get(1)?, x.get(2)?, x.get(3)?, x.get(4)?)),
            )
            .optional()?;
        let old = old.ok_or(StoreError::NotFound)?;
        // A display-name change never redirects an operation; anything else can.
        let material = old.0 != r.host.as_str()
            || old.1 != u32::from(r.port.get())
            || old.2 != r.username.as_str()
            || old.3 != kind
            || old.4.as_deref() != path;
        if material && unresolved(&tx, &r.id)? {
            return Err(StoreError::InUse);
        }
        let n = tx.execute(
            "UPDATE servers SET name=?2, host=?3, port=?4, username=?5, auth_kind=?6, key_path=?7, secret_ref=?8, updated_at=?9 WHERE id=?1",
            params![r.id, r.name.as_str(), r.host.as_str(), r.port.get(), r.username.as_str(), kind, path, r.secret_ref, r.updated_at],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound);
        }
        if clear_pin {
            tx.execute("DELETE FROM host_keys WHERE server_id=?1", [&r.id])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn set_secret_ref(&self, id: &str, secret_ref: Option<&str>) -> Result<(), StoreError> {
        let n = self.db().execute(
            "UPDATE servers SET secret_ref=?2, updated_at=?3 WHERE id=?1",
            params![id, secret_ref, now()],
        )?;
        if n == 0 {
            Err(StoreError::NotFound)
        } else {
            Ok(())
        }
    }

    /// Deletes the profile and, through the foreign key, its pin. Operation
    /// history is kept with a null server reference. Refused while the
    /// server has an unresolved operation.
    pub fn delete(&self, id: &str) -> Result<(), StoreError> {
        let mut db = self.db();
        let tx = db.transaction()?;
        if unresolved(&tx, id)? {
            return Err(StoreError::InUse);
        }
        let n = tx.execute("DELETE FROM servers WHERE id=?1", [id])?;
        if n == 0 {
            return Err(StoreError::NotFound);
        }
        tx.commit()?;
        Ok(())
    }

    /// True while the server has an operation that is pending, running,
    /// verifying or has an unknown outcome.
    pub fn has_unresolved_operation(&self, server_id: &str) -> Result<bool, StoreError> {
        unresolved(&self.db(), server_id)
    }

    pub fn get_pin(&self, server_id: &str) -> Result<Option<PinnedHostKey>, StoreError> {
        Ok(self
            .db()
            .query_row(
                "SELECT key_type, public_key, sha256 FROM host_keys WHERE server_id=?1",
                [server_id],
                |r| {
                    Ok(PinnedHostKey {
                        key_type: r.get(0)?,
                        public_key: r.get(1)?,
                        sha256: r.get(2)?,
                    })
                },
            )
            .optional()?)
    }

    /// Pins a key for the first time. Fails if a pin already exists; only
    /// [`Store::replace_host_key`] may replace one.
    pub fn pin_host_key(&self, server_id: &str, key: &PresentedHostKey) -> Result<(), StoreError> {
        let db = self.db();
        let exists: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM host_keys WHERE server_id=?1)",
            [server_id],
            |r| r.get(0),
        )?;
        if exists {
            return Err(StoreError::AlreadyPinned);
        }
        db.execute(
            "INSERT INTO host_keys (server_id, key_type, public_key, sha256, pinned_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![server_id, key.key_type, key.public_key, key.sha256, now()],
        )
        .map_err(|e| match e {
            rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation => {
                StoreError::NotFound
            }
            e => e.into(),
        })?;
        Ok(())
    }

    pub fn replace_host_key(
        &self,
        server_id: &str,
        key: &PresentedHostKey,
    ) -> Result<(), StoreError> {
        let mut db = self.db();
        let tx = db.transaction()?;
        if unresolved(&tx, server_id)? {
            return Err(StoreError::InUse);
        }
        tx.execute("DELETE FROM host_keys WHERE server_id=?1", [server_id])?;
        tx.execute(
            "INSERT INTO host_keys (server_id, key_type, public_key, sha256, pinned_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![server_id, key.key_type, key.public_key, key.sha256, now()],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn clear_pin(&self, server_id: &str) -> Result<(), StoreError> {
        self.db()
            .execute("DELETE FROM host_keys WHERE server_id=?1", [server_id])?;
        Ok(())
    }
}

fn unresolved(conn: &Connection, server_id: &str) -> Result<bool, StoreError> {
    Ok(conn.query_row(
        &format!(
            "SELECT EXISTS(SELECT 1 FROM operations WHERE server_id=?1 AND status IN {UNRESOLVED_SQL})"
        ),
        [server_id],
        |r| r.get(0),
    )?)
}

fn migrate(conn: &mut Connection) -> Result<(), StoreError> {
    let tx = conn.transaction()?;
    tx.execute(
        "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL)",
        [],
    )?;
    let current: i64 = tx.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |r| r.get(0),
    )?;
    for (i, sql) in MIGRATIONS.iter().enumerate() {
        let version = i as i64 + 1;
        if version > current {
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT INTO schema_version (version) VALUES (?1)",
                [version],
            )?;
        }
    }
    tx.commit()?;
    Ok(())
}

const COLS: &str =
    "id, name, host, port, username, auth_kind, key_path, secret_ref, created_at, updated_at";

struct RawRow {
    id: String,
    name: String,
    host: String,
    port: u32,
    username: String,
    auth_kind: String,
    key_path: Option<String>,
    secret_ref: Option<String>,
    created_at: String,
    updated_at: String,
}

fn row_to_raw(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawRow> {
    Ok(RawRow {
        id: r.get(0)?,
        name: r.get(1)?,
        host: r.get(2)?,
        port: r.get(3)?,
        username: r.get(4)?,
        auth_kind: r.get(5)?,
        key_path: r.get(6)?,
        secret_ref: r.get(7)?,
        created_at: r.get(8)?,
        updated_at: r.get(9)?,
    })
}

impl RawRow {
    /// Re-validates stored values so a tampered database cannot inject
    /// unvalidated hosts or users.
    fn into_record(self) -> Result<ServerRecord, StoreError> {
        let bad = |e: ossiframe_core::ValidationError| {
            StoreError::Db(format!("invalid stored value: {e}"))
        };
        let auth = match (self.auth_kind.as_str(), self.key_path) {
            ("key_file", Some(p)) => AuthConfig::KeyFile(KeyPath::parse(&p).map_err(bad)?),
            ("password", _) => AuthConfig::Password,
            _ => return Err(StoreError::Db("invalid stored auth configuration".into())),
        };
        Ok(ServerRecord {
            name: ServerName::parse(&self.name).map_err(bad)?,
            host: Hostname::parse(&self.host).map_err(bad)?,
            port: Port::new(self.port).map_err(bad)?,
            username: SshUser::parse(&self.username).map_err(bad)?,
            auth,
            id: self.id,
            secret_ref: self.secret_ref,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

fn auth_cols(a: &AuthConfig) -> (&'static str, Option<&str>) {
    match a {
        AuthConfig::KeyFile(p) => ("key_file", Some(p.as_str())),
        AuthConfig::Password => ("password", None),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn record(id: &str) -> ServerRecord {
        ServerRecord {
            id: id.into(),
            name: ServerName::parse("web").unwrap(),
            host: Hostname::parse("example.com").unwrap(),
            port: Port::new(22).unwrap(),
            username: SshUser::parse("root").unwrap(),
            auth: AuthConfig::Password,
            secret_ref: None,
            created_at: now(),
            updated_at: now(),
        }
    }

    fn key(b: u8) -> PresentedHostKey {
        PresentedHostKey {
            key_type: "ssh-ed25519".into(),
            public_key: vec![b; 32],
            sha256: format!("SHA256:{b}"),
        }
    }

    #[test]
    fn migrations_are_idempotent_and_schema_has_no_secret_column() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        migrate(&mut conn).unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
        for table in [
            "servers",
            "host_keys",
            "operations",
            "operation_steps",
            "operation_events",
            "meta",
        ] {
            let mut stmt = conn
                .prepare(&format!("PRAGMA table_info({table})"))
                .unwrap();
            let cols: Vec<String> = stmt
                .query_map([], |r| r.get(1))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            for c in &cols {
                assert!(
                    !["password", "passphrase", "secret", "private_key"].contains(&c.as_str()),
                    "{table}.{c}"
                );
            }
        }
    }

    /// Inserts an operation row directly, so these tests run without the
    /// `provisioning` feature.
    pub(crate) fn raw_operation(s: &Store, id: &str, server: &str, status: &str) {
        s.db()
            .execute(
                "INSERT INTO operations (id, server_id, server_name, server_host, server_port, server_user, host_key_sha256, kind, plan_digest, plan_json, status, created_at)
                 VALUES (?1, ?2, 'snap-name', 'snap.example.com', 2222, 'deploy', 'SHA256:snap', 'provision', 'd', '{}', ?3, 'now')",
                params![id, server, status],
            )
            .unwrap();
    }

    #[test]
    fn migration_2_upgrades_an_existing_v1_database_in_place() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        // A database as migration 1 alone created it, with data.
        conn.execute_batch(MIGRATIONS[0]).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER NOT NULL);
             INSERT INTO schema_version VALUES (1);
             INSERT INTO servers VALUES ('a','web','example.com',22,'root','password',NULL,'ref','t','t');
             INSERT INTO host_keys VALUES ('a','ssh-ed25519',x'0102','SHA256:k','t');",
        )
        .unwrap();
        let s = Store::init(conn).unwrap();
        let rec = s.get("a").unwrap();
        assert_eq!(rec.secret_ref.as_deref(), Some("ref"));
        assert_eq!(s.get_pin("a").unwrap().unwrap().sha256, "SHA256:k");
        assert!(!s.has_unresolved_operation("a").unwrap());
        let version: i64 = s
            .db()
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 2);
        let id: String = s
            .db()
            .query_row(
                "SELECT value FROM meta WHERE key='installation_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(id.len(), 32);
    }

    #[test]
    fn history_survives_profile_removal_without_cascade() {
        let s = Store::open_in_memory().unwrap();
        s.insert(&record("a")).unwrap();
        s.pin_host_key("a", &key(1)).unwrap();
        raw_operation(&s, "op", "a", "succeeded");
        s.db()
            .execute(
                "INSERT INTO operation_steps (operation_id, idx, step_id, title, status) VALUES ('op',0,'s','t','succeeded')",
                [],
            )
            .unwrap();
        s.db()
            .execute(
                "INSERT INTO operation_events (operation_id, server_label, at, kind) VALUES ('op','snap-name','t','x')",
                [],
            )
            .unwrap();
        s.delete("a").unwrap();
        let (server_id, name, host, port, user, hk): (Option<String>, String, String, u32, String, String) = s
            .db()
            .query_row(
                "SELECT server_id, server_name, server_host, server_port, server_user, host_key_sha256 FROM operations WHERE id='op'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .unwrap();
        assert_eq!(server_id, None);
        assert_eq!(
            (
                name.as_str(),
                host.as_str(),
                port,
                user.as_str(),
                hk.as_str()
            ),
            (
                "snap-name",
                "snap.example.com",
                2222,
                "deploy",
                "SHA256:snap"
            )
        );
        for table in ["operation_steps", "operation_events"] {
            let n: i64 = s
                .db()
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 1, "{table} kept");
        }
    }

    #[test]
    fn profile_changes_are_blocked_for_each_unresolved_state() {
        let s = Store::open_in_memory().unwrap();
        for status in ["pending", "running", "verifying", "unknownOutcome"] {
            let id = format!("srv-{status}");
            s.insert(&record(&id)).unwrap();
            s.pin_host_key(&id, &key(1)).unwrap();
            raw_operation(&s, &format!("op-{status}"), &id, status);
            assert!(s.has_unresolved_operation(&id).unwrap());

            assert_eq!(s.delete(&id), Err(StoreError::InUse), "{status}");
            assert_eq!(s.replace_host_key(&id, &key(2)), Err(StoreError::InUse));
            assert_eq!(s.get_pin(&id).unwrap().unwrap().public_key, vec![1; 32]);

            let r = s.get(&id).unwrap();
            let mut edits = Vec::new();
            let mut e = r.clone();
            e.host = Hostname::parse("other.example.com").unwrap();
            edits.push(e);
            let mut e = r.clone();
            e.port = Port::new(2222).unwrap();
            edits.push(e);
            let mut e = r.clone();
            e.username = SshUser::parse("deploy").unwrap();
            edits.push(e);
            let mut e = r.clone();
            e.auth = AuthConfig::KeyFile(KeyPath::parse("/home/u/.ssh/id").unwrap());
            edits.push(e);
            for e in &edits {
                assert_eq!(s.update(e, true), Err(StoreError::InUse), "{status}");
            }
            assert!(s.get_pin(&id).unwrap().is_some(), "pin kept");
            assert_eq!(s.get(&id).unwrap().host, r.host);

            let mut renamed = r.clone();
            renamed.name = ServerName::parse("renamed").unwrap();
            s.update(&renamed, false).unwrap();
            assert_eq!(s.get(&id).unwrap().name.as_str(), "renamed");
        }
    }

    #[test]
    fn terminal_operations_do_not_block_changes() {
        let s = Store::open_in_memory().unwrap();
        for (n, status) in ["succeeded", "failed", "cancelled"].into_iter().enumerate() {
            let id = format!("s{n}");
            s.insert(&record(&id)).unwrap();
            raw_operation(&s, &format!("op{n}"), &id, status);
            assert!(!s.has_unresolved_operation(&id).unwrap());
            let mut e = s.get(&id).unwrap();
            e.host = Hostname::parse("other.example.com").unwrap();
            s.update(&e, true).unwrap();
            s.delete(&id).unwrap();
        }
    }

    #[test]
    fn crud_and_pins() {
        let s = Store::open_in_memory().unwrap();
        s.insert(&record("a")).unwrap();
        assert_eq!(s.list().unwrap().len(), 1);
        assert_eq!(s.get("missing").unwrap_err(), StoreError::NotFound);

        s.pin_host_key("a", &key(1)).unwrap();
        assert_eq!(
            s.pin_host_key("a", &key(2)).unwrap_err(),
            StoreError::AlreadyPinned
        );
        assert_eq!(
            s.get_pin("a").unwrap().unwrap().public_key,
            vec![1; 32],
            "old pin kept"
        );

        s.replace_host_key("a", &key(3)).unwrap();
        assert_eq!(s.get_pin("a").unwrap().unwrap().public_key, vec![3; 32]);

        s.clear_pin("a").unwrap();
        assert!(s.get_pin("a").unwrap().is_none());

        s.pin_host_key("a", &key(4)).unwrap();
        s.delete("a").unwrap();
        assert!(
            s.get_pin("a").unwrap().is_none(),
            "pin removed with profile"
        );
        assert_eq!(
            s.pin_host_key("a", &key(5)).unwrap_err(),
            StoreError::NotFound
        );
    }

    #[test]
    fn tampered_rows_are_rejected() {
        let s = Store::open_in_memory().unwrap();
        s.insert(&record("a")).unwrap();
        s.db()
            .execute(
                "UPDATE servers SET host='-oProxyCommand=x' WHERE id='a'",
                [],
            )
            .unwrap();
        assert!(matches!(s.get("a"), Err(StoreError::Db(_))));
    }
}
