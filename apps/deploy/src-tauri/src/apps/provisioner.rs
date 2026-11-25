//! The `Provisioner` the Deploy engine uses.
//!
//! One engine serves both kinds of operation, so there is one remote lock and
//! one claim set per server. [`DeployProvisioner`] sends a plan whose target
//! has an `app_id` to [`AppProvisioner`] and every other plan to the S04
//! provisioner. The engine only calls `recheck`, `gate`, `verify` and
//! `app_recovery`; plans are built by the typed functions in `plans.rs`, so
//! the generic preflight and plan entry points are the S04 ones.
use super::dto::{parse_app_type, release_of_row};
use super::plans::{Kind, WATCH_HOST_KEYS, plan_kind};
use async_trait::async_trait;
use ossiframe_core::executor::{CancelToken, Executor};
use ossiframe_core::module::OperationError;
use ossiframe_core::provision::{
    AppRecovery, AppRecoveryVerdict, CheckResult, CheckStatus, ComponentInfo, Drift, Plan,
    PlanError, PlanTarget, PreflightReport, PriorOperation, Provisioner,
};
use ossiframe_core::{CommandSpec, DomainName, GitUrl, LinuxUserName, Severity};
use ossiframe_desktop::store::{AppRecord, Store};
use ossiframe_mod_app_deploy::app_type::AppType;
use ossiframe_mod_app_deploy::create::run_gate;
use ossiframe_mod_app_deploy::git_access::{HostKeys, recheck_host_keys};
use ossiframe_mod_app_deploy::identity::app_slug;
use ossiframe_mod_app_deploy::paths::AppRoot;
use ossiframe_mod_app_deploy::preflight::{
    self, AppUserExpectation, PreflightInput, ScannedKey, ssh_endpoint, verify_app_user,
};
use ossiframe_mod_app_deploy::release::{FileState, observe_app_state};
use ossiframe_mod_app_deploy::remove::verify_removed;
use ossiframe_mod_app_deploy::verify::{VerifyInput, recover_app, release_of_plan, verify_app};
use ossiframe_mod_ubuntu_provisioning::UbuntuProvisioning;
use std::sync::Arc;
use std::time::Duration;

const SHORT: Duration = Duration::from_secs(20);

fn drift(id: &str, message: impl Into<String>) -> Drift {
    Drift {
        id: id.to_owned(),
        message: message.into(),
    }
}

fn check(id: &str, title: &str, ok: bool, detail: &str) -> CheckResult {
    CheckResult {
        component: "app".to_owned(),
        id: id.to_owned(),
        title: title.to_owned(),
        essential: true,
        status: if ok {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        detail: detail.to_owned(),
    }
}

/// Provisioner for plans that belong to an application.
pub struct AppProvisioner {
    store: Arc<Store>,
}

struct Subject {
    app: AppRecord,
    kind: Kind,
    app_type: AppType,
    slug: ossiframe_core::AppSlug,
}

impl AppProvisioner {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }

    fn subject(&self, plan: &Plan) -> Result<Subject, String> {
        let id = plan
            .target
            .app_id
            .as_deref()
            .ok_or("the plan has no application")?;
        let app = self
            .store
            .get_app(id)
            .map_err(|_| "the application record is missing".to_owned())?;
        let kind = plan_kind(plan).ok_or("the plan is not an application plan")?;
        let app_type = parse_app_type(&app.app_type).map_err(|e| e.to_string())?;
        let slug = app_slug(&app.slug).map_err(|e| e.to_string())?;
        Ok(Subject {
            app,
            kind,
            app_type,
            slug,
        })
    }

    async fn verify_user(
        &self,
        exec: &dyn Executor,
        cancel: &CancelToken,
        s: &Subject,
        expect: AppUserExpectation,
    ) -> Vec<Drift> {
        match verify_app_user(exec, cancel, &s.slug, &expect).await {
            Ok(conflicts) => conflicts
                .into_iter()
                .filter(|c| c.severity == Severity::Blocker)
                .map(|c| drift(&c.id, c.message))
                .collect(),
            Err(e) => vec![drift(
                "app-user",
                format!("the application user could not be checked: {e}"),
            )],
        }
    }

    fn existing_user(s: &Subject) -> Result<AppUserExpectation, String> {
        Ok(AppUserExpectation::Existing {
            recorded_user: LinuxUserName::parse(&s.app.linux_user).map_err(|e| e.to_string())?,
        })
    }
}

#[async_trait]
impl Provisioner for AppProvisioner {
    fn components(&self) -> Vec<ComponentInfo> {
        Vec::new()
    }

    async fn preflight(
        &self,
        _: &dyn Executor,
        _: &CancelToken,
    ) -> Result<PreflightReport, OperationError> {
        Err(OperationError::InvalidArguments(
            "application plans are built by the app commands".to_owned(),
        ))
    }

    async fn preflight_for(
        &self,
        _: &dyn Executor,
        _: &CancelToken,
        _: &[String],
        _: &[PriorOperation],
    ) -> Result<PreflightReport, OperationError> {
        Err(OperationError::InvalidArguments(
            "application plans are built by the app commands".to_owned(),
        ))
    }

    fn plan(&self, _: &PreflightReport, _: &[String]) -> Result<Plan, PlanError> {
        Err(PlanError::Refused(
            "application plans are built by the app commands".to_owned(),
        ))
    }

    /// Re-checks what the plan depends on: for a create or deploy the whole
    /// application preflight again (base stack, disk, domain, app user, and a
    /// Git host key rescan for a confirmed host), for the other plans the app
    /// user. A deploy or rollback also needs the site and pool files to still
    /// carry this app's marker.
    async fn recheck(
        &self,
        exec: &dyn Executor,
        cancel: &CancelToken,
        plan: &Plan,
    ) -> Result<(), Vec<Drift>> {
        let s = self
            .subject(plan)
            .map_err(|m| vec![drift("application", m)])?;
        let mut out: Vec<Drift> = Vec::new();
        match s.kind {
            Kind::Create { .. } | Kind::Deploy => {
                let input = (|| -> Result<PreflightInput, String> {
                    Ok(PreflightInput {
                        app_type: s.app_type,
                        with_database: s.app.db_enabled,
                        slug: s.slug.clone(),
                        domain: DomainName::parse(&s.app.domain).map_err(|e| e.to_string())?,
                        repository: GitUrl::parse(&s.app.repo_url).map_err(|e| e.to_string())?,
                        user: if matches!(s.kind, Kind::Create { .. }) {
                            AppUserExpectation::New
                        } else {
                            Self::existing_user(&s)?
                        },
                        own_site_file: (!matches!(s.kind, Kind::Create { .. }))
                            .then(|| format!("ossiframe-{}", s.slug.as_str())),
                    })
                })()
                .map_err(|m| vec![drift("application", m)])?;
                match preflight::run(exec, cancel, &input).await {
                    Ok(report) => out.extend(
                        report
                            .conflicts
                            .into_iter()
                            .filter(|c| c.severity == Severity::Blocker)
                            .map(|c| drift(&c.id, c.message)),
                    ),
                    Err(e) => out.push(drift("preflight", format!("the recheck failed: {e}"))),
                }
            }
            Kind::Rollback | Kind::Remove | Kind::EnvSet => match Self::existing_user(&s) {
                Ok(expect) => out.extend(self.verify_user(exec, cancel, &s, expect).await),
                Err(m) => out.push(drift("application", m)),
            },
        }
        if let Some(fps) = plan.preconditions.watched_listings.get(WATCH_HOST_KEYS)
            && let Some((host, port)) = GitUrl::parse(&s.app.repo_url)
                .ok()
                .and_then(|u| ssh_endpoint(&u))
        {
            let planned = HostKeys::Confirmed {
                host,
                port,
                keys: fps
                    .iter()
                    .map(|fp| ScannedKey {
                        key_type: String::new(),
                        fingerprint: fp.clone(),
                        key: String::new(),
                    })
                    .collect(),
            };
            match recheck_host_keys(exec, cancel, &planned).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => out.push(drift("git-host-keys", e.to_string())),
                Err(e) => out.push(drift("git-host-keys", e.to_string())),
            }
        }
        if out.is_empty() && matches!(s.kind, Kind::Deploy | Kind::Rollback) {
            match observe_app_state(exec, cancel, &s.slug, s.app_type).await {
                Ok(obs) => {
                    if obs.site != FileState::Managed {
                        out.push(drift(
                            "site-marker",
                            "the NGINX site is missing or was edited by hand",
                        ));
                    }
                    if s.app_type.info().uses_php && obs.pool != Some(FileState::Managed) {
                        out.push(drift(
                            "pool-marker",
                            "the PHP-FPM pool is missing or was edited by hand",
                        ));
                    }
                }
                Err(e) => out.push(drift("observe", e)),
            }
        }
        if out.is_empty() { Ok(()) } else { Err(out) }
    }

    async fn gate(
        &self,
        exec: &dyn Executor,
        cancel: &CancelToken,
        plan: &Plan,
        step_id: &str,
    ) -> Result<String, String> {
        run_gate(exec, cancel, plan, step_id).await
    }

    async fn verify(
        &self,
        exec: &dyn Executor,
        cancel: &CancelToken,
        plan: &Plan,
    ) -> Vec<CheckResult> {
        let fail = |m: &str| vec![check("application", "Application state", false, m)];
        let s = match self.subject(plan) {
            Ok(s) => s,
            Err(m) => return fail(&m),
        };
        match s.kind {
            Kind::Create { deploy: false } => provisioned(exec, cancel, &s).await,
            Kind::Create { deploy: true } | Kind::Deploy | Kind::Rollback => {
                let release = match release_of_plan(plan) {
                    Ok(r) => r,
                    Err(m) => return fail(&m),
                };
                let domain = match DomainName::parse(&s.app.domain) {
                    Ok(d) => d,
                    Err(e) => return fail(&e.to_string()),
                };
                verify_app(
                    exec,
                    cancel,
                    &VerifyInput {
                        app_type: s.app_type,
                        slug: s.slug,
                        domain,
                        release,
                    },
                )
                .await
            }
            Kind::Remove => verify_removed(exec, cancel, &s.slug, s.app_type).await,
            Kind::EnvSet => vec![env_present(exec, cancel, &s).await],
        }
    }

    /// Real state of the app behind an interrupted deploy, create-with-deploy
    /// or rollback: observe it and compare with the expected release and the
    /// release the store says was live before. The app user is verified first,
    /// since the observation runs as that user.
    async fn app_recovery(
        &self,
        exec: &dyn Executor,
        cancel: &CancelToken,
        plan: &Plan,
    ) -> Option<AppRecovery> {
        let s = self.subject(plan).ok()?;
        if !matches!(
            s.kind,
            Kind::Create { deploy: true } | Kind::Deploy | Kind::Rollback
        ) {
            return None;
        }
        let unknown = |detail: String| AppRecovery {
            verdict: AppRecoveryVerdict::Unknown,
            detail,
        };
        let expect = match Self::existing_user(&s) {
            Ok(e) => e,
            Err(m) => return Some(unknown(m)),
        };
        let problems = self.verify_user(exec, cancel, &s, expect).await;
        if let Some(p) = problems.first() {
            return Some(unknown(format!(
                "the application user could not be verified: {}",
                p.message
            )));
        }
        let release = match release_of_plan(plan) {
            Ok(r) => r,
            Err(m) => return Some(unknown(m)),
        };
        let domain = match DomainName::parse(&s.app.domain) {
            Ok(d) => d,
            Err(e) => return Some(unknown(e.to_string())),
        };
        let previous = s
            .app
            .current_release_id
            .as_deref()
            .and_then(|r| release_of_row(&s.app.id, r));
        Some(
            recover_app(
                exec,
                cancel,
                &VerifyInput {
                    app_type: s.app_type,
                    slug: s.slug,
                    domain,
                    release,
                },
                previous,
            )
            .await,
        )
    }
}

/// A create without a first deploy: site, pool and services are in place.
async fn provisioned(exec: &dyn Executor, cancel: &CancelToken, s: &Subject) -> Vec<CheckResult> {
    let obs = match observe_app_state(exec, cancel, &s.slug, s.app_type).await {
        Ok(o) => o,
        Err(e) => return vec![check("observe", "Application state", false, &e)],
    };
    let mut out = vec![
        check(
            "site",
            "NGINX site is in place",
            obs.site == FileState::Managed,
            "the site file carries this app's marker",
        ),
        check(
            "nginx",
            "NGINX is active",
            obs.nginx_active == Some(true),
            "nginx reports active",
        ),
    ];
    if s.app_type.info().uses_php {
        out.push(check(
            "pool",
            "PHP-FPM pool is in place",
            obs.pool == Some(FileState::Managed),
            "the pool file carries this app's marker",
        ));
        out.push(check(
            "fpm",
            "PHP-FPM is active",
            obs.fpm_active == Some(true),
            "php8.3-fpm reports active",
        ));
    }
    out
}

/// After an `.env` write: the file is there, read as the app user.
async fn env_present(exec: &dyn Executor, cancel: &CancelToken, s: &Subject) -> CheckResult {
    let built = (|| {
        let user = ossiframe_mod_app_deploy::identity::app_user(&s.slug)?;
        let root = AppRoot::new(&s.slug)?;
        let path = ossiframe_mod_app_deploy::env::env_path(&root)?;
        CommandSpec::run_as(user, "/usr/bin/test", &["-f", path.as_str()], SHORT)
    })();
    let ok = match built {
        Ok(cmd) => matches!(exec.run(&cmd, cancel).await, Ok(r) if r.exit.success()),
        Err(_) => false,
    };
    check(
        "env-file",
        "shared/.env exists",
        ok,
        if ok {
            "the file is present"
        } else {
            "the file could not be found"
        },
    )
}

/// Routes by plan: application plans to [`AppProvisioner`], the rest to the
/// S04 provisioner.
pub struct DeployProvisioner {
    base: UbuntuProvisioning,
    apps: AppProvisioner,
}

impl DeployProvisioner {
    pub fn new(store: Arc<Store>) -> Self {
        Self {
            base: UbuntuProvisioning,
            apps: AppProvisioner::new(store),
        }
    }

    fn is_app(plan: &Plan) -> bool {
        has_app(&plan.target)
    }
}

fn has_app(t: &PlanTarget) -> bool {
    t.app_id.is_some()
}

#[async_trait]
impl Provisioner for DeployProvisioner {
    fn components(&self) -> Vec<ComponentInfo> {
        self.base.components()
    }

    async fn preflight(
        &self,
        exec: &dyn Executor,
        cancel: &CancelToken,
    ) -> Result<PreflightReport, OperationError> {
        self.base.preflight(exec, cancel).await
    }

    async fn preflight_for(
        &self,
        exec: &dyn Executor,
        cancel: &CancelToken,
        selection: &[String],
        prior: &[PriorOperation],
    ) -> Result<PreflightReport, OperationError> {
        self.base
            .preflight_for(exec, cancel, selection, prior)
            .await
    }

    fn plan(&self, report: &PreflightReport, selection: &[String]) -> Result<Plan, PlanError> {
        self.base.plan(report, selection)
    }

    async fn recheck(
        &self,
        exec: &dyn Executor,
        cancel: &CancelToken,
        plan: &Plan,
    ) -> Result<(), Vec<Drift>> {
        if Self::is_app(plan) {
            self.apps.recheck(exec, cancel, plan).await
        } else {
            self.base.recheck(exec, cancel, plan).await
        }
    }

    async fn gate(
        &self,
        exec: &dyn Executor,
        cancel: &CancelToken,
        plan: &Plan,
        step_id: &str,
    ) -> Result<String, String> {
        if Self::is_app(plan) {
            self.apps.gate(exec, cancel, plan, step_id).await
        } else {
            self.base.gate(exec, cancel, plan, step_id).await
        }
    }

    async fn verify(
        &self,
        exec: &dyn Executor,
        cancel: &CancelToken,
        plan: &Plan,
    ) -> Vec<CheckResult> {
        if Self::is_app(plan) {
            self.apps.verify(exec, cancel, plan).await
        } else {
            self.base.verify(exec, cancel, plan).await
        }
    }

    async fn app_recovery(
        &self,
        exec: &dyn Executor,
        cancel: &CancelToken,
        plan: &Plan,
    ) -> Option<AppRecovery> {
        if Self::is_app(plan) {
            self.apps.app_recovery(exec, cancel, plan).await
        } else {
            self.base.app_recovery(exec, cancel, plan).await
        }
    }
}
