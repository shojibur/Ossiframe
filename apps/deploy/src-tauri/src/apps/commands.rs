//! The thirteen `app_*` IPC commands (S05).
//!
//! These commands live in the Deploy app, not in `ossiframe-desktop`: the
//! desktop crate is Core-level and must not depend on the app-deploy module
//! (App -> Modules -> Core). Everything generic stays in desktop and is
//! reused as is: the plan store with its approval checks (digest, single
//! use, expiry, server identity, connection, per-server busy rule), the
//! engine with its remote lock, recheck, reconcile and mark-resolved, and
//! the `apps` and `releases` tables.
//!
//! The frontend sends ids, validated names and flags. It never sends a
//! program, argv, path, privilege or plan content: plans are built here from
//! the module's typed step builders and held in memory, with their stdin
//! (the database statement, `.env` content), until they are approved or
//! expire. Stdin is never stored.
//!
//! All logic lives in [`Apps`] so it can be tested without a Tauri app. The
//! `#[tauri::command]` functions are thin wrappers.
use super::dto::{
    AccessCheckDto, AppDto, AppSummaryDto, CreateParams, EnvChangeDto, ReleaseDto, parse_app_type,
    parse_env_changes, release_of_row,
};
use super::events::{
    RELEASE_DEPLOYED, RELEASE_FAILED, RELEASE_PENDING, RELEASE_ROLLED_BACK_FROM,
    RELEASE_ROLLED_BACK_TO,
};
use super::plans::{
    Intent, Pieces, ReleaseChoice, assemble, build_create, build_redeploy, row_id, slug_of,
};
use ossiframe_core::executor::CancelToken;
use ossiframe_core::module::OperationError;
use ossiframe_core::provision::{Conflict, FileChange, Plan, PlanTarget, Severity};
use ossiframe_core::{BranchName, LinuxUserName};
use ossiframe_desktop::ipc::{GitHostFingerprint, IpcError, PlanReason, cap};
use ossiframe_desktop::provisioning::Provisioning;
use ossiframe_desktop::provisioning::commands::{
    OperationStartedDto, PlanDto, approval_error, plan_error, start_error,
};
use ossiframe_desktop::provisioning::engine::{Engine, ServerLink, ServiceLink};
use ossiframe_desktop::provisioning::plans::{
    ApprovalContext, ApprovalError, PlanStore, ServerIdentity, is_digest, is_uuid,
};
use ossiframe_desktop::service::ServerService;
use ossiframe_desktop::store::{
    APP_STATE_ACTIVE, APP_STATE_AWAITING_DEPLOY, APP_STATE_DEPLOYING, APP_STATE_FAILED,
    APP_STATE_REMOVED, AppRecord, ReleaseRecord, Store, StoreError, timestamp,
};
use ossiframe_mod_app_deploy::app_type::{AppType, AppTypeInfo, app_types as module_app_types};
use ossiframe_mod_app_deploy::create::{CreateInput, DbPassword};
use ossiframe_mod_app_deploy::env::{
    EnvContext, EnvEntryDto, EnvError, env_path, env_write_steps, read_env,
};
use ossiframe_mod_app_deploy::git_access::{
    DeployKeyStatus, GitAccessError, check_access, deploy_key_status, plan_host_keys,
};
use ossiframe_mod_app_deploy::identity::app_user;
use ossiframe_mod_app_deploy::paths::AppRoot;
use ossiframe_mod_app_deploy::preflight::{
    self, AppUserExpectation, PreflightInput, PreflightReport, verify_app_user,
};
use ossiframe_mod_app_deploy::release::{
    ReleaseId, ReleaseRecord as ModuleRelease, ReleaseStatus, RollbackError, observe_app_state,
    rollback_plan,
};
use ossiframe_mod_app_deploy::remove::{RemoveInput, remove_steps};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use tauri::State;

/// Bare names of the application commands, for `AppManifest::commands` and
/// the capability `allow-` entries. Deploy only.
pub const APP_COMMAND_NAMES: &[&str] = &[
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

// ---- server access ------------------------------------------------------------------

/// What the commands need to know about a server profile.
#[derive(Debug, Clone)]
pub struct Profile {
    pub identity: ServerIdentity,
    pub name: String,
    pub connected: bool,
}

/// The server side the commands run against. The production implementation
/// is [`ServiceAccess`]; tests supply a scripted one.
pub trait ServerAccess: Send + Sync {
    fn profile(&self, server_id: &str) -> Result<Profile, IpcError>;
    fn link(&self, server_id: &str) -> Arc<dyn ServerLink>;
}

pub struct ServiceAccess(pub Arc<ServerService>);

impl ServerAccess for ServiceAccess {
    fn profile(&self, id: &str) -> Result<Profile, IpcError> {
        let v = self.0.get(id)?;
        Ok(Profile {
            connected: v.state == ossiframe_desktop::ConnState::Connected,
            identity: ServerIdentity {
                host: v.record.host.as_str().to_owned(),
                port: v.record.port.get(),
                user: v.record.username.as_str().to_owned(),
                host_key_sha256: v.pin.map(|p| p.sha256).unwrap_or_default(),
            },
            name: v.record.name.clone().into(),
        })
    }

    fn link(&self, server_id: &str) -> Arc<dyn ServerLink> {
        Arc::new(ServiceLink::new(self.0.clone(), server_id))
    }
}

// ---- error mapping ----------------------------------------------------------------

fn app_not_found() -> IpcError {
    plan_error(PlanReason::AppNotFound, "unknown application")
}

fn blocked(message: &str) -> IpcError {
    plan_error(PlanReason::Blocked, &cap(message))
}

fn storage(e: &StoreError) -> IpcError {
    IpcError::Storage {
        message: cap(&e.to_string()),
    }
}

fn op_err(e: OperationError) -> IpcError {
    match e {
        OperationError::InvalidArguments(m) => IpcError::Validation {
            field: "input".into(),
            message: cap(&m),
        },
        OperationError::Exec(x) => x.into(),
        OperationError::Failed(m) => IpcError::OperationFailed { message: cap(&m) },
    }
}

fn env_error(e: EnvError) -> IpcError {
    match e {
        EnvError::Invalid(v) => v.into(),
        EnvError::TooLarge => IpcError::Validation {
            field: "changes".into(),
            message: "the .env file would be too large".into(),
        },
        EnvError::ReadFailed => IpcError::OperationFailed {
            message: "the .env file could not be read".into(),
        },
        EnvError::Exec(x) => x.into(),
        EnvError::Refused(m) => IpcError::Validation {
            field: "changes".into(),
            message: cap(&m),
        },
    }
}

fn rollback_error(e: RollbackError) -> IpcError {
    match e {
        RollbackError::Invalid(v) => v.into(),
        other => blocked(&other.to_string()),
    }
}

/// The user-facing error for a Git host key problem. An unconfirmed host
/// carries its scanned fingerprints so the UI can show them.
fn git_error(e: GitAccessError) -> IpcError {
    match &e {
        GitAccessError::HostKeyUnconfirmed { host, port, keys } => IpcError::Plan {
            reason: PlanReason::GitHostKeyUnconfirmed,
            message: e.to_string(),
            fingerprints: keys
                .iter()
                .map(|k| GitHostFingerprint {
                    host: host.as_str().to_owned(),
                    port: *port,
                    key_type: k.key_type.clone(),
                    sha256: k.fingerprint.clone(),
                })
                .collect(),
        },
        _ => blocked(&e.to_string()),
    }
}

/// A refused preflight. A domain used by another Ossiframe site is its own
/// reason; everything else is `blocked` with the blockers' messages.
pub fn preflight_error(conflicts: &[Conflict]) -> IpcError {
    let blockers: Vec<&Conflict> = conflicts
        .iter()
        .filter(|c| c.severity == Severity::Blocker)
        .collect();
    let reason = if blockers.iter().any(|c| c.id == "domain-in-use") {
        PlanReason::DomainInUse
    } else {
        PlanReason::Blocked
    };
    let text = blockers
        .iter()
        .map(|c| c.message.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    plan_error(reason, &cap(&text))
}

fn require_state(app: &AppRecord, allowed: &[&str]) -> Result<(), IpcError> {
    if allowed.contains(&app.state.as_str()) {
        Ok(())
    } else {
        Err(blocked(&format!(
            "the application is {}; this action is not available now",
            app.state
        )))
    }
}

/// States that allow a deploy, a removal or an environment change.
const IDLE_STATES: [&str; 3] = [
    APP_STATE_ACTIVE,
    APP_STATE_AWAITING_DEPLOY,
    APP_STATE_FAILED,
];

// ---- the service ----------------------------------------------------------------------

type Link = Arc<dyn ServerLink>;

/// Local changes made when a plan is approved, and how to take them back
/// when the engine refuses to start.
enum Undo {
    None,
    DropDraft(String),
    Restore {
        app_id: String,
        state: String,
        release_row: Option<String>,
    },
}

pub struct Apps {
    access: Arc<dyn ServerAccess>,
    store: Arc<Store>,
    engine: Arc<Engine>,
    plans: PlanStore,
    /// Server id -> (plan id, intent). One plan per server, as in the plan
    /// store, so a newer plan replaces the older intent.
    intents: Mutex<HashMap<String, (String, Intent)>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Apps {
    pub fn new(access: Arc<dyn ServerAccess>, store: Arc<Store>, engine: Arc<Engine>) -> Self {
        Self {
            access,
            store,
            engine,
            plans: PlanStore::default(),
            intents: Mutex::default(),
        }
    }

    /// From the provisioning state the app already registered.
    pub fn from_provisioning(p: &Provisioning) -> Self {
        Self::new(Arc::new(ServiceAccess(p.service())), p.store(), p.engine())
    }

    // -- lookups -------------------------------------------------------------

    fn server<'a>(&self, id: &'a str) -> Result<&'a str, IpcError> {
        if is_uuid(id) {
            Ok(id)
        } else {
            Err(IpcError::NotFound)
        }
    }

    fn app(&self, id: &str) -> Result<AppRecord, IpcError> {
        if !is_uuid(id) {
            return Err(app_not_found());
        }
        self.store.get_app(id).map_err(|e| match e {
            StoreError::NotFound => app_not_found(),
            other => storage(&other),
        })
    }

    fn connected(&self, server_id: &str) -> Result<(Profile, Link), IpcError> {
        let p = self.access.profile(server_id)?;
        if !p.connected {
            return Err(IpcError::NotConnected);
        }
        Ok((p, self.access.link(server_id)))
    }

    /// The evidence belongs to the server that was connected at the start.
    fn same_server(link: &Link, p: &Profile) -> Result<(), IpcError> {
        if link.identity().as_ref() == Some(&p.identity) {
            Ok(())
        } else {
            Err(plan_error(
                PlanReason::WrongServer,
                "the connection changed while the server was being checked",
            ))
        }
    }

    /// The app user is the one recorded for this app, a plain no-login user
    /// outside privileged groups, before any command runs as that user.
    async fn verified_user(&self, link: &Link, app: &AppRecord) -> Result<(), IpcError> {
        let conflicts = verify_app_user(
            &**link,
            &CancelToken::new(),
            &slug_of(app)?,
            &AppUserExpectation::Existing {
                recorded_user: LinuxUserName::parse(&app.linux_user)?,
            },
        )
        .await
        .map_err(op_err)?;
        if conflicts.is_empty() {
            Ok(())
        } else {
            Err(preflight_error(&conflicts))
        }
    }

    async fn preflight(
        &self,
        link: &Link,
        profile: &Profile,
        input: &PreflightInput,
    ) -> Result<PreflightReport, IpcError> {
        let report = preflight::run(&**link, &CancelToken::new(), input)
            .await
            .map_err(op_err)?;
        Self::same_server(link, profile)?;
        if !report.can_proceed() {
            return Err(preflight_error(&report.conflicts));
        }
        Ok(report)
    }

    fn existing_input(
        &self,
        app: &AppRecord,
        url: ossiframe_core::GitUrl,
    ) -> Result<PreflightInput, IpcError> {
        Ok(PreflightInput {
            app_type: parse_app_type(&app.app_type)?,
            with_database: app.db_enabled,
            slug: slug_of(app)?,
            domain: ossiframe_core::DomainName::parse(&app.domain)?,
            repository: url,
            user: AppUserExpectation::Existing {
                recorded_user: LinuxUserName::parse(&app.linux_user)?,
            },
            own_site_file: Some(format!("ossiframe-{}", app.slug)),
        })
    }

    /// Stores a built plan with its intent and returns what the user sees.
    fn store_plan(
        &self,
        server_id: &str,
        profile: Profile,
        mut plan: Plan,
        intent: Intent,
    ) -> PlanDto {
        // The identity snapshot is set here, before the plan is stored and
        // digested, whatever the builder put in it.
        plan.target = PlanTarget {
            server_id: server_id.to_owned(),
            name: profile.name,
            host: profile.identity.host,
            port: profile.identity.port,
            user: profile.identity.user,
            host_key_sha256: profile.identity.host_key_sha256,
            app_id: plan.target.app_id,
        };
        let handle = self.plans.insert(plan.clone());
        lock(&self.intents).insert(server_id.to_owned(), (handle.id.clone(), intent));
        PlanDto {
            plan_id: handle.id,
            digest: handle.digest,
            expires_in_secs: handle.expires_in.as_secs(),
            plan,
            warnings: Vec::new(),
        }
    }

    // -- reads -----------------------------------------------------------------

    pub fn types(&self) -> Vec<AppTypeInfo> {
        module_app_types()
    }

    pub fn list(&self, server_id: &str) -> Result<Vec<AppSummaryDto>, IpcError> {
        let id = self.server(server_id)?;
        Ok(self
            .store
            .list_apps(id)
            .map_err(|e| storage(&e))?
            .iter()
            .map(AppSummaryDto::new)
            .collect())
    }

    pub fn get(&self, app_id: &str) -> Result<AppDto, IpcError> {
        let app = self.app(app_id)?;
        let releases = self.store.list_releases(&app.id).map_err(|e| storage(&e))?;
        let current = app
            .current_release_id
            .as_deref()
            .and_then(|c| releases.iter().find(|r| r.id == c));
        Ok(AppDto::new(&app, current))
    }

    pub fn list_releases(&self, app_id: &str) -> Result<Vec<ReleaseDto>, IpcError> {
        let app = self.app(app_id)?;
        Ok(self
            .store
            .list_releases(&app.id)
            .map_err(|e| storage(&e))?
            .iter()
            .map(ReleaseDto::from_record)
            .collect())
    }

    /// The environment of the app with secret values left out. A secret's
    /// value never leaves the backend.
    pub async fn get_env(&self, app_id: &str) -> Result<Vec<EnvEntryDto>, IpcError> {
        let app = self.app(app_id)?;
        let (_, link) = self.connected(&app.server_id)?;
        self.verified_user(&link, &app).await?;
        let slug = slug_of(&app)?;
        let user = app_user(&slug)?;
        let root = AppRoot::new(&slug)?;
        let file = read_env(&*link, &CancelToken::new(), &user, &root)
            .await
            .map_err(env_error)?;
        Ok(file.entries())
    }

    /// The deploy key setup screen: public key, per-host instructions and the
    /// result of the read-only access check.
    pub async fn deploy_key_status(&self, app_id: &str) -> Result<DeployKeyStatus, IpcError> {
        let app = self.app(app_id)?;
        let url = ossiframe_core::GitUrl::parse(&app.repo_url)?;
        let branch = BranchName::parse(&app.repo_branch)?;
        let (_, link) = self.connected(&app.server_id)?;
        self.verified_user(&link, &app).await?;
        deploy_key_status(&*link, &CancelToken::new(), &slug_of(&app)?, &url, &branch)
            .await
            .map_err(op_err)
    }

    /// "Check access": the read-only `git ls-remote` of the deploy-key gate.
    /// No plan, nothing is changed.
    pub async fn check_access(&self, app_id: &str) -> Result<AccessCheckDto, IpcError> {
        let app = self.app(app_id)?;
        let url = ossiframe_core::GitUrl::parse(&app.repo_url)?;
        let branch = BranchName::parse(&app.repo_branch)?;
        let (_, link) = self.connected(&app.server_id)?;
        self.verified_user(&link, &app).await?;
        check_access(&*link, &CancelToken::new(), &slug_of(&app)?, &url, &branch)
            .await
            .map(Into::into)
            .map_err(op_err)
    }

    // -- plans -----------------------------------------------------------------

    /// Creates the app: user, directories, pool, site, database, `.env`, and
    /// for an SSH repository the pinned host keys and the deploy key. A
    /// public HTTPS repository is also cloned and switched in this plan; an
    /// SSH repository waits for the deploy key and a separate "Deploy". A
    /// failed preflight stores nothing.
    pub async fn create_plan(&self, p: &CreateParams) -> Result<PlanDto, IpcError> {
        let i = p.parse()?;
        let server_id = self.server(&p.server_id)?;
        let (profile, link) = self.connected(server_id)?;
        for a in self.store.list_apps(server_id).map_err(|e| storage(&e))? {
            if a.slug == i.slug.as_str() {
                return Err(plan_error(
                    PlanReason::SlugInUse,
                    "an application with this name already exists on this server",
                ));
            }
            if a.state != APP_STATE_REMOVED && a.domain == i.domain.as_str() {
                return Err(plan_error(
                    PlanReason::DomainInUse,
                    "this domain belongs to another application on this server",
                ));
            }
        }
        let with_database = CreateInput {
            app_type: i.app_type,
            slug: i.slug.clone(),
            domain: i.domain.clone(),
            entry_file: i.entry.clone(),
            with_database: i.with_database,
            disable_default_site: i.disable_default_site,
        }
        .database()?;
        let report = self
            .preflight(
                &link,
                &profile,
                &PreflightInput {
                    app_type: i.app_type,
                    with_database,
                    slug: i.slug.clone(),
                    domain: i.domain.clone(),
                    repository: i.url.clone(),
                    user: AppUserExpectation::New,
                    own_site_file: None,
                },
            )
            .await?;
        let keys = plan_host_keys(&i.url, &report.host_keys, &i.confirmed).map_err(git_error)?;
        let password = DbPassword::generate()?;
        let app_id = uuid::Uuid::new_v4().to_string();
        let built = build_create(&i, server_id, &app_id, &keys, &password, ReleaseId::now())?;
        let plan = assemble(&app_id, built.pieces, built.watched, true);
        Ok(self.store_plan(
            server_id,
            profile,
            plan,
            Intent::Create {
                app: Box::new(built.app),
                release: built.release,
            },
        ))
    }

    /// A deploy of the app's branch (or an override). For an SSH repository
    /// the access check runs first and must pass.
    pub async fn redeploy_plan(
        &self,
        app_id: &str,
        repo_branch: Option<&str>,
        run_migrations: bool,
    ) -> Result<PlanDto, IpcError> {
        let branch = repo_branch
            .map(|b| BranchName::parse(b.trim()))
            .transpose()?;
        let app = self.app(app_id)?;
        require_state(&app, &IDLE_STATES)?;
        let branch = match branch {
            Some(b) => b,
            None => BranchName::parse(&app.repo_branch)?,
        };
        let url = ossiframe_core::GitUrl::parse(&app.repo_url)?;
        let (profile, link) = self.connected(&app.server_id)?;
        self.preflight(&link, &profile, &self.existing_input(&app, url.clone())?)
            .await?;
        let access = if url.is_ssh() {
            let r = check_access(&*link, &CancelToken::new(), &slug_of(&app)?, &url, &branch)
                .await
                .map_err(op_err)?;
            Self::same_server(&link, &profile)?;
            if !r.passed() {
                return Err(blocked(r.message()));
            }
            Some(r)
        } else {
            None
        };
        let release = ReleaseId::now();
        let pieces = build_redeploy(&app, &branch, &release, run_migrations, access)?;
        let plan = assemble(&app.id, pieces, BTreeMap::new(), true);
        Ok(self.store_plan(
            &app.server_id,
            profile,
            plan,
            Intent::Deploy {
                app_id: app.id.clone(),
                release: ReleaseChoice {
                    id: release,
                    branch: branch.as_str().to_owned(),
                },
            },
        ))
    }

    /// Switches `current` back to a release of this app that still exists on
    /// the server. No clone, build or migration.
    pub async fn rollback_plan(&self, app_id: &str, release_id: &str) -> Result<PlanDto, IpcError> {
        let target = ReleaseId::parse(release_id)?;
        let app = self.app(app_id)?;
        require_state(&app, &[APP_STATE_ACTIVE, APP_STATE_FAILED])?;
        let (profile, link) = self.connected(&app.server_id)?;
        self.verified_user(&link, &app).await?;
        let app_type = parse_app_type(&app.app_type)?;
        let slug = slug_of(&app)?;
        let obs = observe_app_state(&*link, &CancelToken::new(), &slug, app_type)
            .await
            .map_err(|e| IpcError::OperationFailed { message: cap(&e) })?;
        Self::same_server(&link, &profile)?;
        let records: Vec<ModuleRelease> = self
            .store
            .list_releases(&app.id)
            .map_err(|e| storage(&e))?
            .iter()
            .filter_map(|r| {
                let status = match r.status.as_str() {
                    RELEASE_DEPLOYED => ReleaseStatus::Deployed,
                    RELEASE_FAILED => ReleaseStatus::Failed,
                    RELEASE_ROLLED_BACK_FROM => ReleaseStatus::RolledBackFrom,
                    RELEASE_ROLLED_BACK_TO => ReleaseStatus::RolledBackTo,
                    _ => return None,
                };
                Some(ModuleRelease {
                    id: release_of_row(&app.id, &r.id)?,
                    status,
                })
            })
            .collect();
        let steps =
            rollback_plan(app_type, &slug, &target, &records, &obs).map_err(rollback_error)?;
        let plan = assemble(&app.id, steps.into(), BTreeMap::new(), false);
        Ok(self.store_plan(
            &app.server_id,
            profile,
            plan,
            Intent::Rollback {
                app_id: app.id.clone(),
            },
        ))
    }

    /// Removes the NGINX site and the PHP-FPM pool. The Linux user, the files
    /// and the database are only touched when the matching flag is set.
    pub async fn remove_plan(
        &self,
        app_id: &str,
        drop_database: bool,
        delete_files: bool,
    ) -> Result<PlanDto, IpcError> {
        let app = self.app(app_id)?;
        require_state(&app, &IDLE_STATES)?;
        let (profile, _) = self.connected(&app.server_id)?;
        let steps = remove_steps(&RemoveInput {
            app_type: parse_app_type(&app.app_type)?,
            slug: slug_of(&app)?,
            drop_database,
            delete_files,
        })?;
        let plan = assemble(&app.id, steps.into(), BTreeMap::new(), false);
        Ok(self.store_plan(
            &app.server_id,
            profile,
            plan,
            Intent::Remove {
                app_id: app.id.clone(),
            },
        ))
    }

    /// Changes `.env` values. Only the changed keys are sent; the backend
    /// merges them into the file that is on the server, so every unchanged
    /// value, secret or not, is kept. The merged file travels as stdin of the
    /// plan's write step and is not part of the plan the user sees.
    pub async fn set_env_plan(
        &self,
        app_id: &str,
        changes: &[EnvChangeDto],
    ) -> Result<PlanDto, IpcError> {
        let typed = parse_env_changes(changes)?;
        let app = self.app(app_id)?;
        require_state(&app, &IDLE_STATES)?;
        if !app.db_enabled {
            return Err(blocked("this application has no environment file"));
        }
        let (profile, link) = self.connected(&app.server_id)?;
        self.verified_user(&link, &app).await?;
        let slug = slug_of(&app)?;
        let user = app_user(&slug)?;
        let root = AppRoot::new(&slug)?;
        let existing = read_env(&*link, &CancelToken::new(), &user, &root)
            .await
            .map_err(env_error)?;
        Self::same_server(&link, &profile)?;
        let ctx = EnvContext {
            laravel: parse_app_type(&app.app_type)? == AppType::Laravel,
            managed_database: app.db_enabled,
        };
        let steps = env_write_steps(&user, &root, &existing, &typed, ctx).map_err(env_error)?;
        let set: Vec<&str> = changes
            .iter()
            .filter(|c| !c.delete)
            .map(|c| c.key.as_str())
            .collect();
        let del: Vec<&str> = changes
            .iter()
            .filter(|c| c.delete)
            .map(|c| c.key.as_str())
            .collect();
        let pieces = Pieces {
            steps,
            files: vec![FileChange {
                path: env_path(&root)?.as_str().to_owned(),
                description: format!(
                    "Environment file, app user, mode 0600. Set: {}. Remove: {}. Values are not shown.",
                    if set.is_empty() {
                        "none".to_owned()
                    } else {
                        set.join(", ")
                    },
                    if del.is_empty() {
                        "none".to_owned()
                    } else {
                        del.join(", ")
                    },
                ),
            }],
            services: Vec::new(),
        };
        let plan = assemble(&app.id, pieces, BTreeMap::new(), false);
        Ok(self.store_plan(
            &app.server_id,
            profile,
            plan,
            Intent::EnvSet {
                app_id: app.id.clone(),
            },
        ))
    }

    // -- approval --------------------------------------------------------------

    fn take_intent(&self, server_id: &str, plan_id: &str) -> Option<Intent> {
        let mut m = lock(&self.intents);
        match m.get(server_id) {
            Some((id, _)) if id == plan_id => m.remove(server_id).map(|(_, i)| i),
            _ => None,
        }
    }

    /// Approves the stored plan. Every check of `PlanStore::take_approved`
    /// applies (digest, single use, expiry, server identity, connection and
    /// the per-server unresolved-operation rule); the engine then takes the
    /// remote lock and rechecks before the first mutating step.
    pub async fn approve_plan(
        &self,
        server_id: &str,
        plan_id: &str,
        digest: &str,
    ) -> Result<OperationStartedDto, IpcError> {
        let server_id = self.server(server_id)?;
        if !is_uuid(plan_id) {
            return Err(plan_error(PlanReason::UnknownPlan, "unknown plan"));
        }
        if !is_digest(digest) {
            return Err(IpcError::Validation {
                field: "digest".into(),
                message: "must be 64 lowercase hex characters".into(),
            });
        }
        let profile = self.access.profile(server_id)?;
        let busy = self
            .store
            .has_unresolved_operation(server_id)
            .map_err(|e| storage(&e))?;
        let approved = match self.plans.take_approved(
            server_id,
            plan_id,
            digest,
            &ApprovalContext {
                identity: profile.identity,
                connected: profile.connected,
                busy,
            },
        ) {
            Ok(a) => a,
            Err(e) => {
                // A stale request must not drop the newer plan's intent.
                if e != ApprovalError::Superseded {
                    self.take_intent(server_id, plan_id);
                }
                return Err(approval_error(e));
            }
        };
        let intent = self
            .take_intent(server_id, plan_id)
            .ok_or_else(|| plan_error(PlanReason::UnknownPlan, "unknown plan"))?;
        let undo = self.prepare(&intent)?;
        match self
            .engine
            .start(approved, self.access.link(server_id))
            .await
        {
            Ok(started) => {
                // The task owns the operation from here. Progress arrives
                // through `operation-updated` and `provision_get_operation`.
                drop(started.task);
                Ok(OperationStartedDto {
                    operation_id: started.operation_id,
                })
            }
            Err(e) => {
                self.undo(server_id, undo);
                Err(start_error(e))
            }
        }
    }

    /// Local records for the operation about to start.
    fn prepare(&self, intent: &Intent) -> Result<Undo, IpcError> {
        let pending_release = |app: &AppRecord, r: &ReleaseChoice| {
            self.store
                .insert_release(&ReleaseRecord {
                    id: row_id(app, &r.id),
                    app_id: app.id.clone(),
                    commit_sha: None,
                    branch: r.branch.clone(),
                    status: RELEASE_PENDING.to_owned(),
                    operation_id: None,
                    created_at: timestamp(),
                })
                .map_err(|e| storage(&e))
        };
        match intent {
            Intent::Create { app, release } => {
                self.store.insert_app(app).map_err(|_| {
                    plan_error(
                        PlanReason::SlugInUse,
                        "an application with this name already exists on this server",
                    )
                })?;
                if let Some(r) = release
                    && let Err(e) = pending_release(app, r)
                {
                    let _ = self.store.delete_draft_app(&app.id);
                    return Err(e);
                }
                Ok(Undo::DropDraft(app.id.clone()))
            }
            Intent::Deploy { app_id, release } => {
                let app = self.app(app_id)?;
                require_state(&app, &IDLE_STATES)?;
                self.store
                    .set_app_state(&app.id, APP_STATE_DEPLOYING)
                    .map_err(|e| storage(&e))?;
                if let Err(e) = pending_release(&app, release) {
                    let _ = self.store.set_app_state(&app.id, &app.state);
                    return Err(e);
                }
                Ok(Undo::Restore {
                    app_id: app.id.clone(),
                    state: app.state.clone(),
                    release_row: Some(row_id(&app, &release.id)),
                })
            }
            Intent::Rollback { app_id } => {
                let app = self.app(app_id)?;
                require_state(&app, &[APP_STATE_ACTIVE, APP_STATE_FAILED])?;
                self.store
                    .set_app_state(&app.id, APP_STATE_DEPLOYING)
                    .map_err(|e| storage(&e))?;
                Ok(Undo::Restore {
                    app_id: app.id.clone(),
                    state: app.state,
                    release_row: None,
                })
            }
            Intent::Remove { app_id } | Intent::EnvSet { app_id } => {
                require_state(&self.app(app_id)?, &IDLE_STATES)?;
                Ok(Undo::None)
            }
        }
    }

    /// The engine refused to start. Nothing ran, so the local records go back
    /// to what they were, unless an operation with an unknown outcome now
    /// exists for the server: then they stay as they are.
    fn undo(&self, server_id: &str, undo: Undo) {
        if self
            .store
            .has_unresolved_operation(server_id)
            .unwrap_or(true)
        {
            return;
        }
        match undo {
            Undo::None => {}
            Undo::DropDraft(id) => {
                let _ = self.store.delete_draft_app(&id);
            }
            Undo::Restore {
                app_id,
                state,
                release_row,
            } => {
                if let Some(row) = release_row {
                    let _ = self.store.delete_release(&app_id, &row);
                }
                let _ = self.store.set_app_state(&app_id, &state);
            }
        }
    }
}

// ---- Tauri glue ----------------------------------------------------------------------

type AppState<'a> = State<'a, Apps>;

#[tauri::command]
pub async fn app_types(state: AppState<'_>) -> Result<Vec<AppTypeInfo>, IpcError> {
    Ok(state.types())
}

#[tauri::command]
pub async fn app_list(
    state: AppState<'_>,
    server_id: String,
) -> Result<Vec<AppSummaryDto>, IpcError> {
    state.list(&server_id)
}

#[tauri::command]
pub async fn app_get(state: AppState<'_>, app_id: String) -> Result<AppDto, IpcError> {
    state.get(&app_id)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn app_create_plan(
    state: AppState<'_>,
    server_id: String,
    app_type: String,
    slug: String,
    domain: String,
    repo_url: String,
    repo_branch: String,
    entry_file: Option<String>,
    with_database: bool,
    disable_default_site: bool,
    trusted_git_host_fingerprints: Option<Vec<String>>,
) -> Result<PlanDto, IpcError> {
    state
        .create_plan(&CreateParams {
            server_id,
            app_type,
            slug,
            domain,
            repo_url,
            repo_branch,
            entry_file,
            with_database,
            disable_default_site,
            trusted_git_host_fingerprints,
        })
        .await
}

#[tauri::command]
pub async fn app_redeploy_plan(
    state: AppState<'_>,
    app_id: String,
    repo_branch: Option<String>,
    run_migrations: bool,
) -> Result<PlanDto, IpcError> {
    state
        .redeploy_plan(&app_id, repo_branch.as_deref(), run_migrations)
        .await
}

#[tauri::command]
pub async fn app_rollback_plan(
    state: AppState<'_>,
    app_id: String,
    release_id: String,
) -> Result<PlanDto, IpcError> {
    state.rollback_plan(&app_id, &release_id).await
}

#[tauri::command]
pub async fn app_remove_plan(
    state: AppState<'_>,
    app_id: String,
    drop_database: bool,
    delete_files: bool,
) -> Result<PlanDto, IpcError> {
    state
        .remove_plan(&app_id, drop_database, delete_files)
        .await
}

#[tauri::command]
pub async fn app_approve_plan(
    state: AppState<'_>,
    server_id: String,
    plan_id: String,
    digest: String,
) -> Result<OperationStartedDto, IpcError> {
    state.approve_plan(&server_id, &plan_id, &digest).await
}

#[tauri::command]
pub async fn app_get_env(
    state: AppState<'_>,
    app_id: String,
) -> Result<Vec<EnvEntryDto>, IpcError> {
    state.get_env(&app_id).await
}

#[tauri::command]
pub async fn app_set_env(
    state: AppState<'_>,
    app_id: String,
    changes: Vec<EnvChangeDto>,
) -> Result<PlanDto, IpcError> {
    state.set_env_plan(&app_id, &changes).await
}

#[tauri::command]
pub async fn app_list_releases(
    state: AppState<'_>,
    app_id: String,
) -> Result<Vec<ReleaseDto>, IpcError> {
    state.list_releases(&app_id)
}

#[tauri::command]
pub async fn app_deploy_key_status(
    state: AppState<'_>,
    app_id: String,
) -> Result<DeployKeyStatus, IpcError> {
    state.deploy_key_status(&app_id).await
}

#[tauri::command]
pub async fn app_check_access(
    state: AppState<'_>,
    app_id: String,
) -> Result<AccessCheckDto, IpcError> {
    state.check_access(&app_id).await
}

#[cfg(test)]
mod tests;
