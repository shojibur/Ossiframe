//! Release deploy, boot check, atomic switch, retention, redeploy and
//! rollback steps, plus a read-only observation of an app's real state.
//!
//! Everything inside an app directory runs as `RunAs(app user)`; root runs
//! only `systemctl reload php8.3-fpm` (correction 1). Application code
//! (composer scripts, artisan, the boot check) never runs as root.
//!
//! Long steps (clone, composer, migrate) are `Durable`. The engine launches a
//! durable `RunAs(user)` step with `systemd-run --uid=<user> --gid=<user>`,
//! started by root: systemd runs the process as the app user, and the unit
//! survives an SSH loss so a disconnect ends in `UnknownOutcome`, not in a
//! guess. Short steps are `Foreground`.
//!
//! Steps that depend on the cloned repository (composer for Php, the
//! `APP_KEY` check, retention) are `Gate` steps run by [`run_gate`]; the
//! create module's `run_gate` delegates to it. A gate step runs over the
//! channel, so a lost connection during the Php composer run ends the step
//! as failed with an "unknown state" message, not as `UnknownOutcome`.
//!
//! A failing step or boot check stops the operation before the switch and
//! keeps the release directory. The `.git` directory is kept in the release;
//! the NGINX site denies dotfiles.
//!
//! Redeploy never writes `.env`, never touches the database password and has
//! no user, pool, site or database step. `APP_KEY` is generated only while it
//! is empty (first deploy); a set key is never changed.

use crate::app_type::AppType;
use crate::create::{STEP_USER_VERIFY, find, nginx_marker, plan_slug, run, run_ok, v};
use crate::git_access::{AccessResult, deploy_allowed, git_check_step, git_ssh_command};
use crate::identity::{app_slug, app_user};
use crate::paths::{AppRoot, EntryFile, FPM_POOL_DIR, NGINX_SITES_AVAILABLE};
use ossiframe_core::{
    AppSlug, BranchName, CancelToken, CommandSpec, Executor, FileChange, GitUrl, LinuxUserName,
    Plan, PlanStep, Privilege, ServiceEffect, StepMode, ValidationError,
};
use std::time::Duration;

pub const STEP_CLONE: &str = "release-clone";
pub const STEP_COMMIT: &str = "release-commit";
pub const STEP_STORAGE_DIRS: &str = "release-storage-dirs";
pub const STEP_STORAGE_REMOVE: &str = "release-storage-remove";
pub const STEP_STORAGE_LINK: &str = "release-storage-link";
pub const STEP_LINK_ENV: &str = "release-link-env";
/// Laravel: durable. Php: gate that runs composer only if `composer.json`
/// exists.
pub const STEP_COMPOSER: &str = "build-composer";
/// Gate: `key:generate` only while `APP_KEY` is empty.
pub const STEP_APP_KEY: &str = "build-app-key";
pub const STEP_ARTISAN_STORAGE_LINK: &str = "build-storage-link";
pub const STEP_MIGRATE: &str = "build-migrate";
pub const STEP_CONFIG_CACHE: &str = "build-config-cache";
pub const STEP_ROUTE_CACHE: &str = "build-route-cache";
pub const STEP_VIEW_CACHE: &str = "build-view-cache";
pub const STEP_BOOT_CHECK: &str = "boot-check";
/// Rollback: the target release directory still exists.
pub const STEP_RELEASE_EXISTS: &str = "release-exists";
pub const STEP_SWITCH_LINK: &str = "release-switch-link";
pub const STEP_SWITCH: &str = "release-switch";
pub const STEP_FPM_RELOAD: &str = "fpm-reload";
/// Gate: remove releases beyond the retention limit.
pub const STEP_CLEANUP: &str = "release-cleanup";

/// Newest releases kept besides `current`.
pub const KEEP_RELEASES: usize = 5;

const SHORT: Duration = Duration::from_secs(60);
const MEDIUM: Duration = Duration::from_secs(180);
const CLONE_DEADLINE: Duration = Duration::from_secs(900);
const BUILD_DEADLINE: Duration = Duration::from_secs(1800);

const ENV: &str = "/usr/bin/env";
const GIT: &str = "/usr/bin/git";
const COMPOSER: &str = "/usr/bin/composer";
const PHP: &str = "/usr/bin/php";
const LN: &str = "/usr/bin/ln";
const MV: &str = "/usr/bin/mv";
const RM: &str = "/usr/bin/rm";
const MKDIR: &str = "/usr/bin/mkdir";
const TEST: &str = "/usr/bin/test";
const LS: &str = "/usr/bin/ls";
const READLINK: &str = "/usr/bin/readlink";
const GREP: &str = "/usr/bin/grep";
const HEAD: &str = "/usr/bin/head";
const SYSTEMCTL: &str = "/usr/bin/systemctl";
const SWITCH_TMP: &str = "current.ossiframe-tmp";

// ---------------------------------------------------------------------------
// Release id
// ---------------------------------------------------------------------------

/// `YYYYMMDDTHHMMSSZ` (UTC). Sorts chronologically as a string.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReleaseId(String);

impl ReleaseId {
    pub fn parse(input: &str) -> Result<Self, ValidationError> {
        const F: &str = "releaseId";
        let b = input.as_bytes();
        let shape = b.len() == 16
            && b[8] == b'T'
            && b[15] == b'Z'
            && b[..8].iter().chain(&b[9..15]).all(u8::is_ascii_digit);
        if !shape {
            return Err(ValidationError::new(F, "must look like 20250131T235959Z"));
        }
        let n = |r: std::ops::Range<usize>| input[r].parse::<u32>().unwrap_or(u32::MAX);
        let ok = (1..=12).contains(&n(4..6))
            && (1..=31).contains(&n(6..8))
            && n(9..11) < 24
            && n(11..13) < 60
            && n(13..15) < 60;
        if !ok {
            return Err(ValidationError::new(F, "is not a valid UTC timestamp"));
        }
        Ok(Self(input.to_owned()))
    }

    /// From seconds since the Unix epoch (civil-from-days, proleptic
    /// Gregorian).
    pub fn from_unix_secs(secs: u64) -> Self {
        let days = (secs / 86_400) as i64;
        let rem = secs % 86_400;
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = yoe + era * 400 + i64::from(m <= 2);
        Self(format!(
            "{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z",
            rem / 3600,
            rem % 3600 / 60,
            rem % 60
        ))
    }

    pub fn now() -> Self {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        Self::from_unix_secs(secs)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `<app_root>/releases/<id>`.
    pub fn dir(&self, root: &AppRoot) -> String {
        format!("{}/releases/{}", root.as_str(), self.0)
    }
}

/// The release id of `<root>/releases/<id>`, nothing else.
fn release_of_dir(root: &AppRoot, dir: &str) -> Result<ReleaseId, String> {
    dir.strip_prefix(&format!("{}/releases/", root.as_str()))
        .and_then(|id| ReleaseId::parse(id).ok())
        .ok_or_else(|| "the step does not name a release of this app; re-plan required".to_owned())
}

// ---------------------------------------------------------------------------
// Inputs and outputs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DeployInput {
    pub app_type: AppType,
    pub slug: AppSlug,
    pub url: GitUrl,
    pub branch: BranchName,
    pub entry_file: EntryFile,
    pub release: ReleaseId,
    /// The app has `shared/.env` (Laravel always, Php when it has a
    /// database). Static: never.
    pub has_env: bool,
    /// Laravel only. Off unless the user opted in for this deploy.
    pub run_migrations: bool,
    /// Result of the last "Check access"; required for an SSH URL.
    pub access: Option<AccessResult>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeployError {
    Invalid(ValidationError),
    /// An SSH repository whose deploy key has not passed "Check access".
    AccessNotChecked,
}

impl std::fmt::Display for DeployError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(e) => write!(f, "{e}"),
            Self::AccessNotChecked => f.write_str(
                "the deploy key has not passed the access check; run \"Check access\" first",
            ),
        }
    }
}

impl std::error::Error for DeployError {}

impl From<ValidationError> for DeployError {
    fn from(e: ValidationError) -> Self {
        Self::Invalid(e)
    }
}

/// What a release plan adds to a `Plan`.
#[derive(Debug, Clone)]
pub struct ReleaseSteps {
    pub steps: Vec<PlanStep>,
    pub files: Vec<FileChange>,
    pub services: Vec<ServiceEffect>,
}

fn st(id: &str, title: &str, command: CommandSpec, mode: StepMode, deadline: Duration) -> PlanStep {
    PlanStep {
        id: id.to_owned(),
        title: title.to_owned(),
        command,
        mode,
        deadline,
        requires_stdin: false,
    }
}

fn fg(id: &str, title: &str, command: CommandSpec) -> PlanStep {
    st(id, title, command, StepMode::Foreground, SHORT)
}

fn ra(
    user: &LinuxUserName,
    program: &str,
    args: &[&str],
    t: Duration,
) -> Result<CommandSpec, ValidationError> {
    CommandSpec::run_as(user.clone(), program, args, t)
}

fn artisan(
    user: &LinuxUserName,
    release_dir: &str,
    args: &[&str],
    t: Duration,
) -> Result<CommandSpec, ValidationError> {
    let script = format!("{release_dir}/artisan");
    let mut a: Vec<&str> = vec![&script];
    a.extend_from_slice(args);
    ra(user, PHP, &a, t)
}

fn composer_command(
    user: &LinuxUserName,
    root: &AppRoot,
    release_dir: &str,
) -> Result<CommandSpec, ValidationError> {
    let r = root.as_str();
    let chdir = format!("--chdir={release_dir}");
    let home = format!("HOME={r}");
    let chome = format!("COMPOSER_HOME={r}/.composer");
    let wd = format!("--working-dir={release_dir}");
    ra(
        user,
        ENV,
        &[
            &chdir,
            &home,
            &chome,
            "COMPOSER_NO_INTERACTION=1",
            COMPOSER,
            "install",
            "--no-dev",
            "--no-interaction",
            "--prefer-dist",
            "--optimize-autoloader",
            &wd,
        ],
        BUILD_DEADLINE,
    )
}

fn clone_command(
    user: &LinuxUserName,
    root: &AppRoot,
    url: &GitUrl,
    branch: &BranchName,
    release_dir: &str,
) -> Result<CommandSpec, ValidationError> {
    let ssh_env = format!("GIT_SSH_COMMAND={}", git_ssh_command(root));
    let mut args: Vec<&str> = Vec::new();
    if url.is_ssh() {
        args.push(&ssh_env);
    }
    args.extend([
        "GIT_TERMINAL_PROMPT=0",
        GIT,
        "clone",
        "--depth",
        "1",
        "--branch",
        branch.as_str(),
        "--single-branch",
        "--",
        url.as_str(),
        release_dir,
    ]);
    ra(user, ENV, &args, CLONE_DEADLINE)
}

/// The Boot_Check, run as the app user.
pub fn boot_check_command(
    app_type: AppType,
    user: &LinuxUserName,
    release_dir: &str,
    entry: &EntryFile,
) -> Result<CommandSpec, ValidationError> {
    let entry_path = format!("{release_dir}/{}", entry.as_str());
    match app_type {
        AppType::Laravel => artisan(user, release_dir, &["--version"], SHORT),
        AppType::Php => ra(user, PHP, &["-l", &entry_path], SHORT),
        AppType::Static => ra(user, TEST, &["-f", &entry_path], SHORT),
    }
}

fn switch_steps(
    user: &LinuxUserName,
    root: &AppRoot,
    release_dir: &str,
    uses_php: bool,
) -> Result<Vec<PlanStep>, ValidationError> {
    let tmp = format!("{}/{SWITCH_TMP}", root.as_str());
    let cur = format!("{}/current", root.as_str());
    let mut steps = vec![
        fg(
            STEP_SWITCH_LINK,
            "Prepare the new current link",
            ra(user, LN, &["-sfnT", "--", release_dir, &tmp], SHORT)?,
        ),
        fg(
            STEP_SWITCH,
            "Switch current to the release (atomic rename)",
            ra(user, MV, &["-Tf", "--", &tmp, &cur], SHORT)?,
        ),
    ];
    if uses_php {
        steps.push(fg(
            STEP_FPM_RELOAD,
            "Reload PHP-FPM so opcache sees the new release",
            CommandSpec::privileged(SYSTEMCTL, &["reload", "php8.3-fpm"], SHORT)?,
        ));
    }
    Ok(steps)
}

fn verify_user_step(user: &LinuxUserName) -> Result<PlanStep, ValidationError> {
    Ok(st(
        STEP_USER_VERIFY,
        &format!("Verify {} is a safe application user", user.as_str()),
        CommandSpec::new("/usr/bin/getent", &["passwd", "--", user.as_str()], SHORT)?,
        StepMode::Gate,
        SHORT,
    ))
}

fn effects(
    uses_php: bool,
    release_dir: &str,
    root: &AppRoot,
) -> (Vec<FileChange>, Vec<ServiceEffect>) {
    let files = vec![
        FileChange {
            path: release_dir.to_owned(),
            description: "New release (cloned, built), app user".to_owned(),
        },
        FileChange {
            path: format!("{}/current", root.as_str()),
            description: "Symlink switched to the new release, app user".to_owned(),
        },
    ];
    let services = if uses_php {
        vec![ServiceEffect {
            unit: "php8.3-fpm".to_owned(),
            effect: "reload".to_owned(),
        }]
    } else {
        Vec::new()
    };
    (files, services)
}

// ---------------------------------------------------------------------------
// Deploy and redeploy
// ---------------------------------------------------------------------------

/// Clone, build, boot check, switch and cleanup. For a create plan, put these
/// after the user verification and git access steps. A redeploy uses
/// [`redeploy_steps`].
pub fn deploy_steps(input: &DeployInput) -> Result<ReleaseSteps, DeployError> {
    let info = input.app_type.info();
    if info.entry_file_fixed && input.entry_file.as_str() != info.entry_file {
        return Err(ValidationError::new(
            "entryFile",
            format!("must be {} for this application type", info.entry_file),
        )
        .into());
    }
    if input.run_migrations && input.app_type != AppType::Laravel {
        return Err(
            ValidationError::new("migrations", "only a Laravel app runs migrations").into(),
        );
    }
    let needs_env = match input.app_type {
        AppType::Laravel => true,
        AppType::Php => input.has_env,
        AppType::Static => false,
    };
    if input.app_type == AppType::Laravel && !input.has_env {
        return Err(ValidationError::new("env", "a Laravel app needs shared/.env").into());
    }
    if input.app_type == AppType::Static && input.has_env {
        return Err(ValidationError::new("env", "a static site has no .env").into());
    }
    if !deploy_allowed(&input.url, input.access) {
        return Err(DeployError::AccessNotChecked);
    }
    let slug = app_slug(input.slug.as_str())?;
    let user = app_user(&slug)?;
    let root = AppRoot::new(&slug)?;
    let r = root.as_str();
    let dir = input.release.dir(&root);
    let laravel = input.app_type == AppType::Laravel;

    let mut steps: Vec<PlanStep> = Vec::new();
    if input.url.is_ssh() {
        steps.push(git_check_step(&slug, &input.url, &input.branch)?);
    }
    steps.push(st(
        STEP_CLONE,
        "Clone the repository into a new release (depth 1)",
        clone_command(&user, &root, &input.url, &input.branch, &dir)?,
        StepMode::Durable,
        CLONE_DEADLINE,
    ));
    steps.push(fg(
        STEP_COMMIT,
        "Read the commit of the release",
        ra(&user, GIT, &["-C", &dir, "rev-parse", "HEAD"], SHORT)?,
    ));
    if laravel {
        let storage = format!("{r}/shared/storage");
        let sub: Vec<String> = [
            "app/public",
            "framework/cache",
            "framework/sessions",
            "framework/views",
            "logs",
        ]
        .iter()
        .map(|p| format!("{storage}/{p}"))
        .collect();
        let mut mk: Vec<&str> = vec!["-p", "--"];
        mk.extend(sub.iter().map(String::as_str));
        steps.push(fg(
            STEP_STORAGE_DIRS,
            "Create the shared storage directories (kept across releases)",
            ra(&user, MKDIR, &mk, SHORT)?,
        ));
        steps.push(fg(
            STEP_STORAGE_REMOVE,
            "Remove the release's own storage directory",
            ra(&user, RM, &["-rf", "--", &format!("{dir}/storage")], MEDIUM)?,
        ));
        steps.push(fg(
            STEP_STORAGE_LINK,
            "Link storage to the shared storage",
            ra(
                &user,
                LN,
                &["-sfnT", "--", &storage, &format!("{dir}/storage")],
                SHORT,
            )?,
        ));
    }
    if needs_env {
        steps.push(fg(
            STEP_LINK_ENV,
            "Link .env to shared/.env",
            ra(
                &user,
                LN,
                &["-sfnT", "--", "../../shared/.env", &format!("{dir}/.env")],
                SHORT,
            )?,
        ));
    }
    match input.app_type {
        AppType::Laravel => {
            steps.push(st(
                STEP_COMPOSER,
                "Install Composer dependencies (no dev)",
                composer_command(&user, &root, &dir)?,
                StepMode::Durable,
                BUILD_DEADLINE,
            ));
            steps.push(st(
                STEP_APP_KEY,
                "Generate APP_KEY only if it is empty",
                artisan(&user, &dir, &["key:generate", "--force"], SHORT)?,
                StepMode::Gate,
                SHORT.saturating_mul(2),
            ));
            steps.push(fg(
                STEP_ARTISAN_STORAGE_LINK,
                "Link public/storage",
                artisan(&user, &dir, &["storage:link"], SHORT)?,
            ));
            if input.run_migrations {
                steps.push(st(
                    STEP_MIGRATE,
                    "Run database migrations (opted in; never reversed by a rollback)",
                    artisan(&user, &dir, &["migrate", "--force"], BUILD_DEADLINE)?,
                    StepMode::Durable,
                    BUILD_DEADLINE,
                ));
            }
            for (id, title, cmd) in [
                (STEP_CONFIG_CACHE, "Cache the configuration", "config:cache"),
                (STEP_ROUTE_CACHE, "Cache the routes", "route:cache"),
                (STEP_VIEW_CACHE, "Cache the views", "view:cache"),
            ] {
                steps.push(st(
                    id,
                    title,
                    artisan(&user, &dir, &[cmd], MEDIUM)?,
                    StepMode::Foreground,
                    MEDIUM,
                ));
            }
        }
        AppType::Php => steps.push(st(
            STEP_COMPOSER,
            "Install Composer dependencies if composer.json exists",
            composer_command(&user, &root, &dir)?,
            StepMode::Gate,
            BUILD_DEADLINE,
        )),
        AppType::Static => {}
    }
    steps.push(fg(
        STEP_BOOT_CHECK,
        "Boot check the new release",
        boot_check_command(input.app_type, &user, &dir, &input.entry_file)?,
    ));
    steps.extend(switch_steps(&user, &root, &dir, info.uses_php)?);
    steps.push(st(
        STEP_CLEANUP,
        "Remove releases beyond the newest five (never current)",
        ra(&user, LS, &["-1", "--", &format!("{r}/releases")], SHORT)?,
        StepMode::Gate,
        MEDIUM,
    ));
    let (files, services) = effects(info.uses_php, &dir, &root);
    Ok(ReleaseSteps {
        steps,
        files,
        services,
    })
}

/// A deploy for an existing app: verify the app user, then the same release
/// steps. No user, pool, site, database or `.env` step.
pub fn redeploy_steps(input: &DeployInput) -> Result<ReleaseSteps, DeployError> {
    let mut out = deploy_steps(input)?;
    let user = app_user(&app_slug(input.slug.as_str())?)?;
    out.steps.insert(0, verify_user_step(&user)?);
    Ok(out)
}

/// The commit from `git rev-parse HEAD` output: 40 (or 64) lowercase hex.
pub fn parse_commit_sha(stdout: &str) -> Option<String> {
    let s = stdout.trim();
    let ok =
        matches!(s.len(), 40 | 64) && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    ok.then(|| s.to_owned())
}

// ---------------------------------------------------------------------------
// Retention
// ---------------------------------------------------------------------------

/// Releases to delete: everything beyond the newest [`KEEP_RELEASES`], except
/// the `current` target and the release just deployed.
pub fn retention_victims(
    existing: &[ReleaseId],
    current: Option<&ReleaseId>,
    deployed: Option<&ReleaseId>,
) -> Vec<ReleaseId> {
    let mut all = existing.to_vec();
    all.sort();
    all.dedup();
    all.reverse();
    all.into_iter()
        .skip(KEEP_RELEASES)
        .filter(|r| Some(r) != current && Some(r) != deployed)
        .collect()
}

/// Valid release ids from `ls -1` of the releases directory. Other names are
/// ignored and never deleted.
pub fn parse_release_listing(stdout: &str) -> Vec<ReleaseId> {
    let mut v: Vec<ReleaseId> = stdout
        .lines()
        .filter_map(|l| ReleaseId::parse(l.trim()).ok())
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Where `current` points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CurrentLink {
    /// No symlink.
    Absent,
    Release(ReleaseId),
    /// A link to something that is not one of this app's releases.
    Other,
}

pub fn parse_current_link(root: &AppRoot, stdout: &str) -> CurrentLink {
    match release_of_dir(root, stdout.trim()) {
        Ok(id) => CurrentLink::Release(id),
        Err(_) => CurrentLink::Other,
    }
}

// ---------------------------------------------------------------------------
// Rollback
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseStatus {
    Deployed,
    Failed,
    RolledBackFrom,
    RolledBackTo,
}

/// A release recorded for the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseRecord {
    pub id: ReleaseId,
    pub status: ReleaseStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RollbackError {
    /// Not one of this app's recorded releases.
    UnknownRelease,
    /// The release failed to deploy.
    FailedRelease,
    /// Recorded, but its directory is gone.
    MissingOnDisk,
    AlreadyCurrent,
    Invalid(ValidationError),
}

impl std::fmt::Display for RollbackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::UnknownRelease => "that release is not one of this application's releases",
            Self::FailedRelease => "that release failed to deploy and cannot be rolled back to",
            Self::MissingOnDisk => "that release no longer exists on the server",
            Self::AlreadyCurrent => "that release is already the current release",
            Self::Invalid(_) => "the application identity is not valid",
        })
    }
}

impl std::error::Error for RollbackError {}

impl From<ValidationError> for RollbackError {
    fn from(e: ValidationError) -> Self {
        Self::Invalid(e)
    }
}

/// Pure validation against the records and the observed disk state.
pub fn validate_rollback(
    target: &ReleaseId,
    records: &[ReleaseRecord],
    obs: &AppObservation,
) -> Result<(), RollbackError> {
    let rec = records
        .iter()
        .find(|r| &r.id == target)
        .ok_or(RollbackError::UnknownRelease)?;
    if rec.status == ReleaseStatus::Failed {
        return Err(RollbackError::FailedRelease);
    }
    if !obs.releases.contains(target) {
        return Err(RollbackError::MissingOnDisk);
    }
    if obs.current == CurrentLink::Release(target.clone()) {
        return Err(RollbackError::AlreadyCurrent);
    }
    Ok(())
}

/// Rollback plan: verify the user, verify the directory, switch. No clone,
/// build or migration. `obs` comes from [`observe_app_state`].
pub fn rollback_plan(
    app_type: AppType,
    slug: &AppSlug,
    target: &ReleaseId,
    records: &[ReleaseRecord],
    obs: &AppObservation,
) -> Result<ReleaseSteps, RollbackError> {
    validate_rollback(target, records, obs)?;
    let slug = app_slug(slug.as_str())?;
    let user = app_user(&slug)?;
    let root = AppRoot::new(&slug)?;
    let dir = target.dir(&root);
    let uses_php = app_type.info().uses_php;
    let mut steps = vec![
        verify_user_step(&user)?,
        fg(
            STEP_RELEASE_EXISTS,
            "Check the release directory exists",
            ra(&user, TEST, &["-d", &dir], SHORT)?,
        ),
    ];
    steps.extend(switch_steps(&user, &root, &dir, uses_php)?);
    let (files, services) = effects(uses_php, &dir, &root);
    Ok(ReleaseSteps {
        steps,
        files: files.into_iter().skip(1).collect(),
        services,
    })
}

// ---------------------------------------------------------------------------
// Gates
// ---------------------------------------------------------------------------

/// Gate ids this module runs.
pub fn is_gate(step_id: &str) -> bool {
    matches!(step_id, STEP_COMPOSER | STEP_APP_KEY | STEP_CLEANUP)
}

/// The step's command must be the shape this module builds.
fn expect_shape(cmd: &CommandSpec, user: &LinuxUserName, program: &str) -> Result<(), String> {
    if cmd.program() == program && cmd.privilege() == Privilege::RunAs(user.clone()) {
        Ok(())
    } else {
        Err("the step is not valid; re-plan required".to_owned())
    }
}

/// Delegation target of `create::run_gate` for the release gates.
pub async fn run_gate(
    exec: &dyn Executor,
    cancel: &CancelToken,
    plan: &Plan,
    step_id: &str,
) -> Result<String, String> {
    let slug = plan_slug(plan)?;
    let user = v(app_user(&slug))?;
    let root = v(AppRoot::new(&slug))?;
    let step = find(plan, step_id)?;
    match step_id {
        STEP_COMPOSER => {
            expect_shape(&step.command, &user, ENV)?;
            let dir = step
                .command
                .args()
                .iter()
                .find_map(|a| a.strip_prefix("--working-dir="))
                .ok_or("the step is not valid; re-plan required")?;
            release_of_dir(&root, dir)?;
            let probe = v(ra(
                &user,
                TEST,
                &["-f", &format!("{dir}/composer.json")],
                SHORT,
            ))?;
            match run(exec, cancel, &probe).await?.0 {
                0 => {}
                1 => return Ok("no composer.json in the release; Composer skipped".to_owned()),
                c => return Err(format!("composer.json could not be checked (exit {c})")),
            }
            run_ok(exec, cancel, &step.command, "composer install").await?;
            Ok("composer install completed".to_owned())
        }
        STEP_APP_KEY => {
            expect_shape(&step.command, &user, PHP)?;
            let script = step.command.args().first().ok_or("the step is not valid")?;
            let dir = script
                .strip_suffix("/artisan")
                .ok_or("the step is not valid; re-plan required")?;
            release_of_dir(&root, dir)?;
            // Reports only whether a value is set, never the value.
            let probe = v(ra(
                &user,
                GREP,
                &[
                    "-q",
                    "-e",
                    "^APP_KEY=[^[:space:]\"']",
                    "--",
                    &format!("{}/shared/.env", root.as_str()),
                ],
                SHORT,
            ))?;
            match run(exec, cancel, &probe).await?.0 {
                0 => Ok("APP_KEY is already set; kept".to_owned()),
                1 => {
                    run_ok(exec, cancel, &step.command, "key:generate").await?;
                    Ok("APP_KEY generated".to_owned())
                }
                _ => Err("shared/.env could not be read".to_owned()),
            }
        }
        STEP_CLEANUP => {
            expect_shape(&step.command, &user, LS)?;
            let deployed = release_of_dir(&root, {
                find(plan, STEP_SWITCH_LINK)?
                    .command
                    .args()
                    .get(2)
                    .ok_or("the plan has no switch step")?
            })?;
            let (code, ls) = run(exec, cancel, &step.command).await?;
            if code != 0 {
                return Ok("retention skipped: the releases directory could not be listed".into());
            }
            let readlink = v(ra(
                &user,
                READLINK,
                &["--", &format!("{}/current", root.as_str())],
                SHORT,
            ))?;
            let (code, link) = run(exec, cancel, &readlink).await?;
            // Never delete anything unless `current` is understood.
            let current = match (code, parse_current_link(&root, &link.stdout)) {
                (0, CurrentLink::Release(id)) => id,
                _ => {
                    return Ok(
                        "retention skipped: current does not point to a release of this app".into(),
                    );
                }
            };
            let existing = parse_release_listing(&ls.stdout);
            let victims = retention_victims(&existing, Some(&current), Some(&deployed));
            let mut removed = 0usize;
            let mut failed = 0usize;
            for id in &victims {
                let rm = v(ra(&user, RM, &["-rf", "--", &id.dir(&root)], MEDIUM))?;
                match run(exec, cancel, &rm).await? {
                    (0, _) => removed += 1,
                    _ => failed += 1,
                }
            }
            Ok(format!(
                "kept {} releases; removed {removed}, could not remove {failed}",
                existing.len() - removed
            ))
        }
        other => Err(format!("unknown check {other}")),
    }
}

// ---------------------------------------------------------------------------
// Observation (reconcile and verification, correction 8)
// ---------------------------------------------------------------------------

/// A managed file, judged by its first line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileState {
    Missing,
    /// Present with this app's marker line.
    Managed,
    Altered,
}

/// What is really on the server. Read-only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppObservation {
    pub current: CurrentLink,
    pub releases: Vec<ReleaseId>,
    pub site: FileState,
    /// `None` for a type without a pool.
    pub pool: Option<FileState>,
    pub nginx_active: Option<bool>,
    pub fpm_active: Option<bool>,
}

/// `is-active` prints one state per unit, in argument order.
pub fn parse_is_active(stdout: &str) -> Vec<bool> {
    stdout.lines().map(|l| l.trim() == "active").collect()
}

async fn file_state(
    exec: &dyn Executor,
    cancel: &CancelToken,
    path: &str,
    marker: &str,
) -> Result<FileState, String> {
    // The site and pool files are world-readable, so the login user reads them.
    let head = v(CommandSpec::new(HEAD, &["-n", "1", "--", path], SHORT))?;
    let (code, r) = run(exec, cancel, &head).await?;
    Ok(match code {
        0 if r.stdout.lines().next() == Some(marker) => FileState::Managed,
        0 => FileState::Altered,
        _ => FileState::Missing,
    })
}

/// Reads the real state of an app: the `current` link, the release
/// directories, the site and pool files with their markers, and the NGINX
/// and PHP-FPM services. Needs a verified app user for the `RunAs` reads.
pub async fn observe_app_state(
    exec: &dyn Executor,
    cancel: &CancelToken,
    slug: &AppSlug,
    app_type: AppType,
) -> Result<AppObservation, String> {
    let slug = v(app_slug(slug.as_str()))?;
    let user = v(app_user(&slug))?;
    let root = v(AppRoot::new(&slug))?;
    let uses_php = app_type.info().uses_php;

    let readlink = v(ra(
        &user,
        READLINK,
        &["--", &format!("{}/current", root.as_str())],
        SHORT,
    ))?;
    let (code, r) = run(exec, cancel, &readlink).await?;
    let current = match code {
        0 => parse_current_link(&root, &r.stdout),
        1 => CurrentLink::Absent,
        c => return Err(format!("current could not be read (exit {c})")),
    };
    let ls = v(ra(
        &user,
        LS,
        &["-1", "--", &format!("{}/releases", root.as_str())],
        SHORT,
    ))?;
    let releases =
        parse_release_listing(&run_ok(exec, cancel, &ls, "listing releases").await?.stdout);

    let site = file_state(
        exec,
        cancel,
        &format!("{NGINX_SITES_AVAILABLE}{}", slug.as_str()),
        &nginx_marker(&slug),
    )
    .await?;
    let pool = if uses_php {
        Some(
            file_state(
                exec,
                cancel,
                &format!("{FPM_POOL_DIR}{}.conf", slug.as_str()),
                &crate::create::fpm_marker(&slug),
            )
            .await?,
        )
    } else {
        None
    };
    let units: &[&str] = if uses_php {
        &["php8.3-fpm", "nginx"]
    } else {
        &["nginx"]
    };
    let mut args = vec!["is-active"];
    args.extend_from_slice(units);
    let active = v(CommandSpec::new(SYSTEMCTL, &args, SHORT))?;
    // Exit 3 means "some unit is not active"; the lines still tell which.
    let (_, r) = run(exec, cancel, &active).await?;
    let states = parse_is_active(&r.stdout);
    let (fpm_active, nginx_active) = match (uses_php, states.as_slice()) {
        (true, [f, n]) => (Some(*f), Some(*n)),
        (false, [n]) => (None, Some(*n)),
        _ => (None, None),
    };
    Ok(AppObservation {
        current,
        releases,
        site,
        pool,
        nginx_active,
        fpm_active,
    })
}

#[cfg(test)]
mod tests;
