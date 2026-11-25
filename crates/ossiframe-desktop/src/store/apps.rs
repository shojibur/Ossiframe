//! Deployed applications and their releases. Values are stored as plain
//! strings; the application module validates them before they get here.
//! No secret is stored: no passwords, keys or environment values.

use super::{Store, StoreError, now};
use rusqlite::{Connection, OptionalExtension, params};

/// App state name that no longer blocks removing the server profile.
pub const APP_STATE_REMOVED: &str = "removed";
/// A create operation is running or its outcome is not known.
pub const APP_STATE_PROVISIONING: &str = "provisioning";
/// Created and serving a release.
pub const APP_STATE_ACTIVE: &str = "active";
/// A deploy or rollback is running or its outcome is not known.
pub const APP_STATE_DEPLOYING: &str = "deploying";
/// Created, but no release is deployed yet (for example an SSH repository
/// that waits for its deploy key).
pub const APP_STATE_AWAITING_DEPLOY: &str = "awaiting-deploy";
/// The last operation failed and the app needs attention.
pub const APP_STATE_FAILED: &str = "failed";

/// RFC 3339 UTC timestamp in the format the store writes.
pub fn timestamp() -> String {
    now()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppRecord {
    pub id: String,
    pub server_id: String,
    pub slug: String,
    pub app_type: String,
    pub domain: String,
    pub repo_url: String,
    pub repo_branch: String,
    pub entry_file: String,
    /// The app's Linux user, rechecked before any `RunAs` step.
    pub linux_user: String,
    pub app_root: String,
    pub db_enabled: bool,
    pub db_name: Option<String>,
    pub db_user: Option<String>,
    pub disable_default_site: bool,
    pub state: String,
    pub current_release_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseRecord {
    pub id: String,
    pub app_id: String,
    pub commit_sha: Option<String>,
    pub branch: String,
    pub status: String,
    pub operation_id: Option<String>,
    pub created_at: String,
}

const APP_COLS: &str = "id, server_id, slug, app_type, domain, repo_url, repo_branch, entry_file, linux_user, app_root, db_enabled, db_name, db_user, disable_default_site, state, current_release_id, created_at, updated_at";

fn row_to_app(r: &rusqlite::Row<'_>) -> rusqlite::Result<AppRecord> {
    Ok(AppRecord {
        id: r.get(0)?,
        server_id: r.get(1)?,
        slug: r.get(2)?,
        app_type: r.get(3)?,
        domain: r.get(4)?,
        repo_url: r.get(5)?,
        repo_branch: r.get(6)?,
        entry_file: r.get(7)?,
        linux_user: r.get(8)?,
        app_root: r.get(9)?,
        db_enabled: r.get(10)?,
        db_name: r.get(11)?,
        db_user: r.get(12)?,
        disable_default_site: r.get(13)?,
        state: r.get(14)?,
        current_release_id: r.get(15)?,
        created_at: r.get(16)?,
        updated_at: r.get(17)?,
    })
}

/// True while the server has an app whose state is not Removed.
pub(super) fn has_live_apps(conn: &Connection, server_id: &str) -> Result<bool, StoreError> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM apps WHERE server_id=?1 AND state<>?2)",
        params![server_id, APP_STATE_REMOVED],
        |r| r.get(0),
    )?)
}

impl Store {
    /// Inserts an app. A duplicate `(server_id, slug)` or an unknown server
    /// is a storage error.
    pub fn insert_app(&self, a: &AppRecord) -> Result<(), StoreError> {
        self.db().execute(
            &format!(
                "INSERT INTO apps ({APP_COLS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)"
            ),
            params![
                a.id,
                a.server_id,
                a.slug,
                a.app_type,
                a.domain,
                a.repo_url,
                a.repo_branch,
                a.entry_file,
                a.linux_user,
                a.app_root,
                a.db_enabled,
                a.db_name,
                a.db_user,
                a.disable_default_site,
                a.state,
                a.current_release_id,
                a.created_at,
                a.updated_at
            ],
        )?;
        Ok(())
    }

    pub fn get_app(&self, id: &str) -> Result<AppRecord, StoreError> {
        self.db()
            .query_row(
                &format!("SELECT {APP_COLS} FROM apps WHERE id=?1"),
                [id],
                row_to_app,
            )
            .optional()?
            .ok_or(StoreError::NotFound)
    }

    pub fn list_apps(&self, server_id: &str) -> Result<Vec<AppRecord>, StoreError> {
        let db = self.db();
        let mut stmt = db.prepare(&format!(
            "SELECT {APP_COLS} FROM apps WHERE server_id=?1 ORDER BY slug"
        ))?;
        let rows = stmt
            .query_map([server_id], row_to_app)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn set_app_state(&self, id: &str, state: &str) -> Result<(), StoreError> {
        let n = self.db().execute(
            "UPDATE apps SET state=?2, updated_at=?3 WHERE id=?1",
            params![id, state, now()],
        )?;
        if n == 0 {
            Err(StoreError::NotFound)
        } else {
            Ok(())
        }
    }

    pub fn set_current_release(
        &self,
        id: &str,
        release_id: Option<&str>,
    ) -> Result<(), StoreError> {
        let n = self.db().execute(
            "UPDATE apps SET current_release_id=?2, updated_at=?3 WHERE id=?1",
            params![id, release_id, now()],
        )?;
        if n == 0 {
            Err(StoreError::NotFound)
        } else {
            Ok(())
        }
    }

    pub fn insert_release(&self, r: &ReleaseRecord) -> Result<(), StoreError> {
        self.db().execute(
            "INSERT INTO releases (id, app_id, commit_sha, branch, status, operation_id, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                r.id,
                r.app_id,
                r.commit_sha,
                r.branch,
                r.status,
                r.operation_id,
                r.created_at
            ],
        )?;
        Ok(())
    }

    /// Updates a release row. `commit_sha` and `operation_id` are only
    /// replaced when given. Returns whether a row matched.
    pub fn update_release(
        &self,
        app_id: &str,
        release_id: &str,
        status: &str,
        commit_sha: Option<&str>,
        operation_id: Option<&str>,
    ) -> Result<bool, StoreError> {
        let n = self.db().execute(
            "UPDATE releases SET status=?3, commit_sha=COALESCE(?4, commit_sha),
             operation_id=COALESCE(?5, operation_id) WHERE app_id=?1 AND id=?2",
            params![app_id, release_id, status, commit_sha, operation_id],
        )?;
        Ok(n > 0)
    }

    /// Deletes an app whose create never got going: state provisioning or
    /// failed and no current release. Returns whether a row was deleted.
    pub fn delete_draft_app(&self, id: &str) -> Result<bool, StoreError> {
        let n = self.db().execute(
            "DELETE FROM apps WHERE id=?1 AND state IN (?2, ?3) AND current_release_id IS NULL",
            params![id, APP_STATE_PROVISIONING, APP_STATE_FAILED],
        )?;
        Ok(n > 0)
    }

    /// Deletes a release row that never went anywhere: status pending or
    /// failed. Returns whether a row was deleted.
    pub fn delete_release(&self, app_id: &str, release_id: &str) -> Result<bool, StoreError> {
        let n = self.db().execute(
            "DELETE FROM releases WHERE app_id=?1 AND id=?2 AND status IN ('pending','failed')",
            params![app_id, release_id],
        )?;
        Ok(n > 0)
    }

    /// Releases of an app, newest first.
    pub fn list_releases(&self, app_id: &str) -> Result<Vec<ReleaseRecord>, StoreError> {
        let db = self.db();
        let mut stmt = db.prepare(
            "SELECT id, app_id, commit_sha, branch, status, operation_id, created_at
             FROM releases WHERE app_id=?1 ORDER BY created_at DESC, rowid DESC",
        )?;
        let rows = stmt
            .query_map([app_id], |r| {
                Ok(ReleaseRecord {
                    id: r.get(0)?,
                    app_id: r.get(1)?,
                    commit_sha: r.get(2)?,
                    branch: r.get(3)?,
                    status: r.get(4)?,
                    operation_id: r.get(5)?,
                    created_at: r.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{raw_operation, record};
    use super::*;

    fn app(id: &str, server: &str, slug: &str) -> AppRecord {
        AppRecord {
            id: id.into(),
            server_id: server.into(),
            slug: slug.into(),
            app_type: "laravel".into(),
            domain: "example.com".into(),
            repo_url: "https://github.com/o/r.git".into(),
            repo_branch: "main".into(),
            entry_file: "public/index.php".into(),
            linux_user: format!("ossi-{slug}"),
            app_root: format!("/srv/ossiframe/apps/{slug}"),
            db_enabled: true,
            db_name: Some(format!("ossi_{slug}")),
            db_user: Some(format!("ossi_{slug}")),
            disable_default_site: false,
            state: "active".into(),
            current_release_id: None,
            created_at: now(),
            updated_at: now(),
        }
    }

    #[test]
    fn apps_and_releases_round_trip() {
        let s = Store::open_in_memory().unwrap();
        s.insert(&record("srv")).unwrap();
        raw_operation(&s, "op", "srv", "succeeded");
        let a = app("a1", "srv", "shop");
        s.insert_app(&a).unwrap();
        assert_eq!(s.get_app("a1").unwrap(), a);
        assert_eq!(s.list_apps("srv").unwrap(), vec![a]);
        let r = ReleaseRecord {
            id: "r1".into(),
            app_id: "a1".into(),
            commit_sha: None,
            branch: "main".into(),
            status: "deployed".into(),
            operation_id: Some("op".into()),
            created_at: now(),
        };
        s.insert_release(&r).unwrap();
        s.set_current_release("a1", Some("r1")).unwrap();
        s.set_app_state("a1", "deploying").unwrap();
        let got = s.get_app("a1").unwrap();
        assert_eq!(got.current_release_id.as_deref(), Some("r1"));
        assert_eq!(got.state, "deploying");
        assert_eq!(s.list_releases("a1").unwrap(), vec![r]);
        assert_eq!(s.get_app("nope"), Err(StoreError::NotFound));
        assert_eq!(s.set_app_state("nope", "x"), Err(StoreError::NotFound));
    }

    #[test]
    fn release_update_and_draft_delete() {
        let s = Store::open_in_memory().unwrap();
        s.insert(&record("srv")).unwrap();
        raw_operation(&s, "op", "srv", "succeeded");
        let mut a = app("a1", "srv", "shop");
        a.state = APP_STATE_PROVISIONING.into();
        s.insert_app(&a).unwrap();
        let r = ReleaseRecord {
            id: "r1".into(),
            app_id: "a1".into(),
            commit_sha: None,
            branch: "main".into(),
            status: "pending".into(),
            operation_id: None,
            created_at: now(),
        };
        s.insert_release(&r).unwrap();
        assert!(
            s.update_release("a1", "r1", "deployed", Some("abc"), Some("op"))
                .unwrap()
        );
        // A later update without a commit keeps the commit.
        assert!(s.update_release("a1", "r1", "failed", None, None).unwrap());
        let got = &s.list_releases("a1").unwrap()[0];
        assert_eq!(
            (got.status.as_str(), got.commit_sha.as_deref()),
            ("failed", Some("abc"))
        );
        assert!(!s.update_release("a1", "nope", "x", None, None).unwrap());
        assert!(s.delete_draft_app("a1").unwrap());
        assert_eq!(s.get_app("a1"), Err(StoreError::NotFound));
        // An app with a release in service is never a draft.
        s.insert_app(&a).unwrap();
        s.set_current_release("a1", None).unwrap();
        s.set_app_state("a1", APP_STATE_ACTIVE).unwrap();
        assert!(!s.delete_draft_app("a1").unwrap());
    }

    #[test]
    fn slug_is_unique_per_server() {
        let s = Store::open_in_memory().unwrap();
        s.insert(&record("one")).unwrap();
        s.insert(&record("two")).unwrap();
        s.insert_app(&app("a1", "one", "shop")).unwrap();
        assert!(s.insert_app(&app("a2", "one", "shop")).is_err());
        s.insert_app(&app("a3", "two", "shop")).unwrap();
    }

    #[test]
    fn remove_server_is_refused_while_an_app_is_not_removed() {
        let s = Store::open_in_memory().unwrap();
        s.insert(&record("srv")).unwrap();
        s.insert_app(&app("a1", "srv", "shop")).unwrap();
        s.insert_app(&app("a2", "srv", "blog")).unwrap();
        assert_eq!(s.delete("srv"), Err(StoreError::InUse));
        s.set_app_state("a1", APP_STATE_REMOVED).unwrap();
        assert_eq!(s.delete("srv"), Err(StoreError::InUse));
        s.set_app_state("a2", APP_STATE_REMOVED).unwrap();
        s.delete("srv").unwrap();
        assert_eq!(s.get_app("a1"), Err(StoreError::NotFound));
    }
}
