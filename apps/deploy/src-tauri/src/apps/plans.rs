//! Plan building for the `app_*` commands: the module's typed step builders
//! put together into a core `Plan`, plus the intent that says what the local
//! store must record when the plan is approved and when it ends.
//!
//! The server snapshot in `Plan::target` is filled in by the command layer
//! right before the plan is stored, whatever this file puts there.
use super::dto::{CreateInputs, parse_app_type, release_row_id};
use ossiframe_core::provision::{
    FileChange, Plan, PlanStep, PlanTarget, Preconditions, Resources, ServiceEffect, SudoMode,
};
use ossiframe_core::{AppSlug, BranchName, GitUrl};
use ossiframe_desktop::ipc::{IpcError, PlanReason};
use ossiframe_desktop::store::{APP_STATE_PROVISIONING, AppRecord, timestamp};
use ossiframe_mod_app_deploy::create::{CreateInput, DbPassword, STEP_USER_ADD, create_steps};
use ossiframe_mod_app_deploy::git_access::{AccessResult, HostKeys, git_setup_steps};
use ossiframe_mod_app_deploy::identity::{app_slug, app_user};
use ossiframe_mod_app_deploy::paths::{AppRoot, EntryFile};
use ossiframe_mod_app_deploy::preflight::MIN_FREE_DISK_MB;
use ossiframe_mod_app_deploy::release::{
    DeployError, DeployInput, ReleaseId, STEP_CLONE, STEP_SWITCH_LINK, deploy_steps, redeploy_steps,
};
use ossiframe_mod_app_deploy::remove::STEP_SITE_REMOVE;
use std::collections::BTreeMap;

/// Plan `watched_listings` key: fingerprints of the Git host keys the user
/// confirmed. The recheck scans the host again and compares.
pub const WATCH_HOST_KEYS: &str = "git-host-key-fingerprints";
/// First step of the `.env` write plan (`app_write("env", ..)`).
pub const STEP_ENV_INSTALL: &str = "env-install";

/// What a plan does, read back from its steps. Works on a plan reloaded from
/// storage, so settling an operation after a restart needs no side table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `deploy`: the plan also clones and switches the first release.
    Create {
        deploy: bool,
    },
    Deploy,
    Rollback,
    Remove,
    EnvSet,
}

pub fn plan_kind(plan: &Plan) -> Option<Kind> {
    let has = |id: &str| plan.steps.iter().any(|s| s.id == id);
    if has(STEP_USER_ADD) {
        Some(Kind::Create {
            deploy: has(STEP_CLONE),
        })
    } else if has(STEP_SITE_REMOVE) {
        Some(Kind::Remove)
    } else if has(STEP_CLONE) {
        Some(Kind::Deploy)
    } else if has(STEP_SWITCH_LINK) {
        Some(Kind::Rollback)
    } else if has(STEP_ENV_INSTALL) {
        Some(Kind::EnvSet)
    } else {
        None
    }
}

/// A release a plan will switch to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseChoice {
    pub id: ReleaseId,
    pub branch: String,
}

/// What approval and the end of the operation record in the local store.
#[derive(Debug, Clone)]
pub enum Intent {
    Create {
        app: Box<AppRecord>,
        release: Option<ReleaseChoice>,
    },
    Deploy {
        app_id: String,
        release: ReleaseChoice,
    },
    Rollback {
        app_id: String,
    },
    Remove {
        app_id: String,
    },
    EnvSet {
        app_id: String,
    },
}

/// Steps and display lists from one of the module's builders.
#[derive(Debug, Clone, Default)]
pub struct Pieces {
    pub steps: Vec<PlanStep>,
    pub files: Vec<FileChange>,
    pub services: Vec<ServiceEffect>,
}

impl Pieces {
    fn extend(&mut self, other: Pieces) {
        self.steps.extend(other.steps);
        self.files.extend(other.files);
        self.services.extend(other.services);
    }
}

impl From<ossiframe_mod_app_deploy::create::CreateSteps> for Pieces {
    fn from(c: ossiframe_mod_app_deploy::create::CreateSteps) -> Self {
        Self {
            steps: c.steps,
            files: c.files,
            services: c.services,
        }
    }
}

impl From<ossiframe_mod_app_deploy::release::ReleaseSteps> for Pieces {
    fn from(c: ossiframe_mod_app_deploy::release::ReleaseSteps) -> Self {
        Self {
            steps: c.steps,
            files: c.files,
            services: c.services,
        }
    }
}

impl From<ossiframe_mod_app_deploy::remove::RemoveSteps> for Pieces {
    fn from(c: ossiframe_mod_app_deploy::remove::RemoveSteps) -> Self {
        Self {
            steps: c.steps,
            files: c.files,
            services: c.services,
        }
    }
}

pub fn deploy_error(e: DeployError) -> IpcError {
    match e {
        DeployError::Invalid(v) => v.into(),
        DeployError::AccessNotChecked => IpcError::Plan {
            reason: PlanReason::Blocked,
            message: AccessResult::AuthDenied.message().to_owned(),
            fingerprints: Vec::new(),
        },
    }
}

/// The core `Plan` around the steps. The component is always `app`, the one
/// the post-switch checks report under.
pub fn assemble(
    app_id: &str,
    pieces: Pieces,
    watched: BTreeMap<String, Vec<String>>,
    disk_needed: bool,
) -> Plan {
    Plan {
        target: PlanTarget {
            server_id: String::new(),
            name: String::new(),
            host: String::new(),
            port: 0,
            user: String::new(),
            host_key_sha256: String::new(),
            app_id: Some(app_id.to_owned()),
        },
        os: "ubuntu 24.04".to_owned(),
        components: vec!["app".to_owned()],
        already_present: Vec::new(),
        packages: Vec::new(),
        files: pieces.files,
        services: pieces.services,
        resources: Resources {
            disk_mb_needed: if disk_needed {
                MIN_FREE_DISK_MB as u32
            } else {
                0
            },
            mem_mb_min: 0,
        },
        privileges: vec![
            "root through sudo -n: system users, NGINX and PHP-FPM files, the MySQL database"
                .to_owned(),
            "the application's own Linux user through sudo -n -u: clone, build, .env, releases"
                .to_owned(),
        ],
        limitations: vec![
            "If the connection drops mid-operation the outcome is unknown. Ossiframe never retries it; it reads back the real state.".to_owned(),
            "Database migrations are never reversed by a rollback.".to_owned(),
        ],
        steps: pieces.steps,
        preconditions: Preconditions {
            os_version: "24.04".to_owned(),
            packages: BTreeMap::new(),
            sudo: SudoMode::Passwordless,
            package_sources: Vec::new(),
            watched_listings: watched,
            min_mem_mb: 0,
            min_disk_mb: if disk_needed { MIN_FREE_DISK_MB as u32 } else { 0 },
            server_managed: true,
        },
    }
}

/// A create plan: user, directories, pool, site, database, `.env`, then for
/// an SSH repository the pinned host keys and the deploy key. For an HTTPS
/// repository the first deploy is part of the plan. For an SSH repository it
/// is not: the user adds the deploy key and runs "Deploy" afterwards.
pub struct BuiltCreate {
    pub pieces: Pieces,
    pub watched: BTreeMap<String, Vec<String>>,
    pub app: AppRecord,
    pub release: Option<ReleaseChoice>,
}

pub fn build_create(
    i: &CreateInputs,
    server_id: &str,
    app_id: &str,
    keys: &HostKeys,
    password: &DbPassword,
    release: ReleaseId,
) -> Result<BuiltCreate, IpcError> {
    let create = CreateInput {
        app_type: i.app_type,
        slug: i.slug.clone(),
        domain: i.domain.clone(),
        entry_file: i.entry.clone(),
        with_database: i.with_database,
        disable_default_site: i.disable_default_site,
    };
    let with_db = create.database()?;
    let mut pieces = Pieces::from(create_steps(&create, password)?);
    let git = git_setup_steps(&i.slug, &i.url, keys)?;
    pieces.steps.extend(git.steps);
    pieces.files.extend(git.files);
    let deploys_now = !i.url.is_ssh();
    if deploys_now {
        pieces.extend(
            deploy_steps(&DeployInput {
                app_type: i.app_type,
                slug: i.slug.clone(),
                url: i.url.clone(),
                branch: i.branch.clone(),
                entry_file: i.entry.clone(),
                release: release.clone(),
                has_env: with_db,
                run_migrations: false,
                access: None,
            })
            .map_err(deploy_error)?
            .into(),
        );
    }
    let mut watched = BTreeMap::new();
    if let HostKeys::Confirmed { keys, .. } = keys {
        let mut fps: Vec<String> = keys.iter().map(|k| k.fingerprint.clone()).collect();
        fps.sort();
        watched.insert(WATCH_HOST_KEYS.to_owned(), fps);
    }
    let slug = app_slug(i.slug.as_str())?;
    let user = app_user(&slug)?;
    let root = AppRoot::new(&slug)?;
    let now = timestamp();
    let db = format!("ossi_{}", slug.as_str());
    let app = AppRecord {
        id: app_id.to_owned(),
        server_id: server_id.to_owned(),
        slug: slug.as_str().to_owned(),
        app_type: i.app_type.info().id.to_owned(),
        domain: i.domain.as_str().to_owned(),
        repo_url: i.url.as_str().to_owned(),
        repo_branch: i.branch.as_str().to_owned(),
        entry_file: i.entry.as_str().to_owned(),
        linux_user: user.as_str().to_owned(),
        app_root: root.as_str().to_owned(),
        db_enabled: with_db,
        db_name: with_db.then(|| db.clone()),
        db_user: with_db.then_some(db),
        disable_default_site: i.disable_default_site,
        state: APP_STATE_PROVISIONING.to_owned(),
        current_release_id: None,
        created_at: now.clone(),
        updated_at: now,
    };
    Ok(BuiltCreate {
        pieces,
        watched,
        app,
        release: deploys_now.then(|| ReleaseChoice {
            id: release,
            branch: i.branch.as_str().to_owned(),
        }),
    })
}

/// The deploy steps for an existing app. For an SSH repository `access` is
/// the result of a fresh access check and must have passed.
pub fn build_redeploy(
    app: &AppRecord,
    branch: &BranchName,
    release: &ReleaseId,
    run_migrations: bool,
    access: Option<AccessResult>,
) -> Result<Pieces, IpcError> {
    let input = DeployInput {
        app_type: parse_app_type(&app.app_type)?,
        slug: app_slug(&app.slug)?,
        url: GitUrl::parse(&app.repo_url)?,
        branch: branch.clone(),
        entry_file: EntryFile::parse(&app.entry_file)?,
        release: release.clone(),
        has_env: app.db_enabled,
        run_migrations,
        access,
    };
    Ok(redeploy_steps(&input).map_err(deploy_error)?.into())
}

/// The row id of a release of this app.
pub fn row_id(app: &AppRecord, release: &ReleaseId) -> String {
    release_row_id(&app.id, release)
}

/// Slug of a stored app.
pub fn slug_of(app: &AppRecord) -> Result<AppSlug, IpcError> {
    Ok(app_slug(&app.slug)?)
}
