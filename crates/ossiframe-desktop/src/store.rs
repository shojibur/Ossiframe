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
];

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

    /// Deletes the profile and, through the foreign key, its pin.
    pub fn delete(&self, id: &str) -> Result<(), StoreError> {
        let n = self.db().execute("DELETE FROM servers WHERE id=?1", [id])?;
        if n == 0 {
            Err(StoreError::NotFound)
        } else {
            Ok(())
        }
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
        assert_eq!(n, 1);
        for table in ["servers", "host_keys"] {
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
