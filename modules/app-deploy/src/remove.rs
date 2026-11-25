//! Remove plan: site and pool by default, database and files only on opt-in.
//!
//! - Default: remove the `sites-enabled` link and `sites-available` file, then
//!   the FPM pool file. Each is a `Gate` step: read-only marker check first
//!   (a file without this app's marker is never removed), backup, remove,
//!   `nginx -t` / `php-fpm8.3 -t`, then reload. A failing test restores the
//!   backup (and the link), tests again and fails without reload. The user,
//!   the app directory and the database stay.
//! - `drop_database`: `DROP DATABASE` and `DROP USER` through `mysql` with the
//!   statements on stdin (names derive from the slug only).
//! - `delete_files`: after the pool is removed and reloaded, check that no
//!   process of the app user runs and that the app root is a real directory
//!   (not a symlink) owned by the app user, then `userdel` and
//!   `rm -rf --one-file-system` of exactly the app root. `rm -r` does not
//!   follow symlinks.
//!
//! Marking the app `Removed` is a local store update done by the desktop
//! layer when the operation succeeds ([`RemoveSteps::mark_removed`]). The
//! remote state is confirmed by [`verify_removed`].

use crate::app_type::AppType;
use crate::create::{STEP_USER_VERIFY, find, fpm_marker, nginx_marker, plan_slug, run, run_ok, v};
use crate::identity::{app_slug, app_user};
use crate::paths::{
    AppRoot, FPM_POOL_DIR, ManagedPath, NGINX_SITES_AVAILABLE, NGINX_SITES_ENABLED,
};
use crate::write;
use ossiframe_core::{
    AppSlug, CancelToken, CheckResult, CheckStatus, CommandSpec, Executor, FileChange, Plan,
    PlanStep, ServiceEffect, StepMode, ValidationError,
};
use std::time::Duration;

pub const STEP_SITE_REMOVE: &str = "site-remove";
pub const STEP_POOL_REMOVE: &str = "pool-remove";
pub const STEP_DB_DROP: &str = "db-drop";
pub const STEP_PROCESSES_CHECK: &str = "app-processes-check";
pub const STEP_ROOT_REAL_DIR: &str = "app-root-real-dir";
pub const STEP_USER_DELETE: &str = "user-delete";
pub const STEP_FILES_DELETE: &str = "files-delete";

const SHORT: Duration = Duration::from_secs(60);
const DELETE_DEADLINE: Duration = Duration::from_secs(600);
const APPLY_DEADLINE: Duration = Duration::from_secs(300);
const MYSQL: &str = "/usr/bin/mysql";
const NGINX: &str = "/usr/sbin/nginx";
const PHP_FPM: &str = "/usr/sbin/php-fpm8.3";
const SYSTEMCTL: &str = "/usr/bin/systemctl";
const USERDEL: &str = "/usr/sbin/userdel";
const RM: &str = "/usr/bin/rm";
const LN: &str = "/usr/bin/ln";
const MV: &str = "/usr/bin/mv";
const STAT: &str = "/usr/bin/stat";
const PGREP: &str = "/usr/bin/pgrep";
const TEST: &str = "/usr/bin/test";
const HEAD: &str = "/usr/bin/head";
const READLINK: &str = "/usr/bin/readlink";

#[derive(Debug, Clone)]
pub struct RemoveInput {
    pub app_type: AppType,
    pub slug: AppSlug,
    /// Opt-in. Refused for an app without a database.
    pub drop_database: bool,
    /// Opt-in. Deletes the Linux user and the whole app directory.
    pub delete_files: bool,
}

/// What a remove plan adds to a `Plan`.
#[derive(Debug, Clone)]
pub struct RemoveSteps {
    pub steps: Vec<PlanStep>,
    pub files: Vec<FileChange>,
    pub services: Vec<ServiceEffect>,
    /// The desktop layer marks the app `Removed` when the operation succeeds.
    pub mark_removed: bool,
}

fn st(id: &str, title: &str, command: CommandSpec, mode: StepMode, d: Duration) -> PlanStep {
    PlanStep {
        id: id.to_owned(),
        title: title.to_owned(),
        command,
        mode,
        deadline: d,
        requires_stdin: false,
    }
}

fn db_names(slug: &AppSlug) -> String {
    format!("ossi_{}", slug.as_str())
}

/// The statements for `drop_database`. Only `ossi_<slug>` is named.
pub fn render_drop_sql(slug: &AppSlug) -> String {
    let n = db_names(slug);
    format!("DROP DATABASE IF EXISTS `{n}`;\nDROP USER IF EXISTS '{n}'@'localhost';\n")
}

pub fn remove_steps(input: &RemoveInput) -> Result<RemoveSteps, ValidationError> {
    let info = input.app_type.info();
    let has_db = info.database != crate::app_type::DatabaseUse::Never;
    if input.drop_database && !has_db {
        return Err(ValidationError::new(
            "dropDatabase",
            "this application type has no database",
        ));
    }
    let slug = app_slug(input.slug.as_str())?;
    let user = app_user(&slug)?;
    let root = AppRoot::new(&slug)?;
    let s = slug.as_str();
    let site = ManagedPath::parse(&format!("{NGINX_SITES_AVAILABLE}{s}"))?;
    let link = ManagedPath::parse(&format!("{NGINX_SITES_ENABLED}{s}"))?;
    let pool = ManagedPath::parse(&format!("{FPM_POOL_DIR}{s}.conf"))?;

    let mut steps = vec![
        st(
            STEP_USER_VERIFY,
            &format!("Verify {} is a safe application user", user.as_str()),
            CommandSpec::new("/usr/bin/getent", &["passwd", "--", user.as_str()], SHORT)?,
            StepMode::Gate,
            SHORT,
        ),
        st(
            STEP_SITE_REMOVE,
            "Remove the NGINX site and its link, test and reload (restored on failure)",
            CommandSpec::privileged(RM, &["--", link.as_str(), site.as_str()], SHORT)?,
            StepMode::Gate,
            APPLY_DEADLINE,
        ),
    ];
    let mut files = vec![
        FileChange {
            path: link.as_str().to_owned(),
            description: "Symlink removed".to_owned(),
        },
        FileChange {
            path: site.as_str().to_owned(),
            description: "NGINX site removed (only with this app's marker)".to_owned(),
        },
    ];
    let mut services = vec![ServiceEffect {
        unit: "nginx".to_owned(),
        effect: "reload".to_owned(),
    }];
    if info.uses_php {
        steps.push(st(
            STEP_POOL_REMOVE,
            "Remove the PHP-FPM pool, test and reload (restored on failure)",
            CommandSpec::privileged(RM, &["--", pool.as_str()], SHORT)?,
            StepMode::Gate,
            APPLY_DEADLINE,
        ));
        files.push(FileChange {
            path: pool.as_str().to_owned(),
            description: "PHP-FPM pool removed (only with this app's marker)".to_owned(),
        });
        services.push(ServiceEffect {
            unit: "php8.3-fpm".to_owned(),
            effect: "reload".to_owned(),
        });
    }
    if input.drop_database {
        let n = db_names(&slug);
        let mut drop = st(
            STEP_DB_DROP,
            &format!("Drop the MySQL database and user {n} (cannot be recovered)"),
            CommandSpec::privileged(MYSQL, &["--no-defaults"], SHORT)?
                .with_stdin(render_drop_sql(&slug).into_bytes())?,
            StepMode::Gate,
            SHORT,
        );
        drop.requires_stdin = true;
        steps.push(drop);
        services.push(ServiceEffect {
            unit: "mysql".to_owned(),
            effect: format!("database and user {n} are dropped"),
        });
    }
    if input.delete_files {
        let r = root.as_str();
        steps.push(st(
            STEP_PROCESSES_CHECK,
            "Check no process of the app user is running",
            CommandSpec::new(PGREP, &["-c", "-u", user.as_str()], SHORT)?,
            StepMode::Gate,
            SHORT,
        ));
        steps.push(st(
            STEP_ROOT_REAL_DIR,
            "Check the app directory is a real directory owned by the app user",
            CommandSpec::new(STAT, &["-c", "%F:%U", "--", r], SHORT)?,
            StepMode::Gate,
            SHORT,
        ));
        steps.push(st(
            STEP_USER_DELETE,
            &format!("Delete the Linux user {}", user.as_str()),
            CommandSpec::privileged(USERDEL, &[user.as_str()], SHORT)?,
            StepMode::Foreground,
            SHORT,
        ));
        steps.push(st(
            STEP_FILES_DELETE,
            &format!("Delete {r} (cannot be recovered)"),
            CommandSpec::privileged(RM, &["-rf", "--one-file-system", "--", r], DELETE_DEADLINE)?,
            StepMode::Foreground,
            DELETE_DEADLINE,
        ));
        files.push(FileChange {
            path: r.to_owned(),
            description: "Application directory deleted with everything in it".to_owned(),
        });
    }
    Ok(RemoveSteps {
        steps,
        files,
        services,
        mark_removed: true,
    })
}

// ---------------------------------------------------------------------------
// Gates
// ---------------------------------------------------------------------------

pub fn is_gate(step_id: &str) -> bool {
    matches!(
        step_id,
        STEP_SITE_REMOVE
            | STEP_POOL_REMOVE
            | STEP_DB_DROP
            | STEP_PROCESSES_CHECK
            | STEP_ROOT_REAL_DIR
    )
}

fn priv_cmd(program: &str, args: &[&str]) -> Result<CommandSpec, String> {
    v(CommandSpec::privileged(program, args, SHORT))
}

fn user_cmd(program: &str, args: &[&str]) -> Result<CommandSpec, String> {
    v(CommandSpec::new(program, args, SHORT))
}

/// What the `pgrep -c -u <user>` probe says.
pub fn parse_processes(code: i32, stdout: &str) -> Result<(), String> {
    match code {
        1 => Ok(()),
        0 => Err(format!(
            "{} process(es) of the app user still run (PHP-FPM workers may still be finishing); try again in a moment",
            stdout.trim().chars().take(8).collect::<String>()
        )),
        c => Err(format!(
            "the app user's processes could not be checked (exit {c})"
        )),
    }
}

/// `stat -c %F:%U` of the app root: a real directory owned by the app user.
/// A symlink prints `symbolic link`, so it never matches.
pub fn parse_root_stat(stdout: &str, user: &str) -> Result<(), String> {
    if stdout.trim() == format!("directory:{user}") {
        Ok(())
    } else {
        Err("the app directory is not a plain directory owned by the app user (a symbolic link or another owner); nothing was deleted".to_owned())
    }
}

/// Delegation target of `create::run_gate` for the remove gates.
pub async fn run_gate(
    exec: &dyn Executor,
    cancel: &CancelToken,
    plan: &Plan,
    step_id: &str,
) -> Result<String, String> {
    let slug = plan_slug(plan)?;
    let user = v(app_user(&slug))?;
    let root = v(AppRoot::new(&slug))?;
    let s = slug.as_str();
    match step_id {
        STEP_SITE_REMOVE => {
            let r = Removal {
                file: v(ManagedPath::parse(&format!("{NGINX_SITES_AVAILABLE}{s}")))?,
                link: Some(v(ManagedPath::parse(&format!("{NGINX_SITES_ENABLED}{s}")))?),
                marker: nginx_marker(&slug),
                test: priv_cmd(NGINX, &["-t"])?,
                reload: priv_cmd(SYSTEMCTL, &["reload", "nginx"])?,
                what: "nginx -t",
            };
            remove_managed(exec, &r).await
        }
        STEP_POOL_REMOVE => {
            let r = Removal {
                file: v(ManagedPath::parse(&format!("{FPM_POOL_DIR}{s}.conf")))?,
                link: None,
                marker: fpm_marker(&slug),
                test: priv_cmd(PHP_FPM, &["-t"])?,
                reload: priv_cmd(SYSTEMCTL, &["reload", "php8.3-fpm"])?,
                what: "php-fpm8.3 -t",
            };
            remove_managed(exec, &r).await
        }
        STEP_DB_DROP => {
            let step = find(plan, STEP_DB_DROP)?;
            if !step.command.has_stdin() {
                return Err(
                    "the plan no longer holds its input: re-plan required. Nothing was run"
                        .to_owned(),
                );
            }
            if step.command.program() != MYSQL {
                return Err("the step is not valid; re-plan required".to_owned());
            }
            // The output is dropped: a client error can quote the statement.
            match run(exec, cancel, &step.command).await? {
                (0, _) => Ok("database and user dropped".to_owned()),
                (c, _) => Err(format!("mysql failed (exit {c}); its output is not shown")),
            }
        }
        STEP_PROCESSES_CHECK => {
            let cmd = user_cmd(PGREP, &["-c", "-u", user.as_str()])?;
            let (code, r) = run(exec, cancel, &cmd).await?;
            parse_processes(code, &r.stdout)?;
            Ok("no process of the app user is running".to_owned())
        }
        STEP_ROOT_REAL_DIR => {
            let cmd = user_cmd(STAT, &["-c", "%F:%U", "--", root.as_str()])?;
            let r = run_ok(exec, cancel, &cmd, "stat").await?;
            parse_root_stat(&r.stdout, user.as_str())?;
            Ok("the app directory is a real directory owned by the app user".to_owned())
        }
        other => Err(format!("unknown check {other}")),
    }
}

struct Removal {
    file: ManagedPath,
    /// The `sites-enabled` link (site only).
    link: Option<ManagedPath>,
    marker: String,
    test: CommandSpec,
    reload: CommandSpec,
    what: &'static str,
}

#[derive(Default)]
struct Touched {
    link_removed: bool,
    file_removed: bool,
}

/// Read-only checks, backup, remove, test, reload; on a failed test restore
/// and test again without reloading.
async fn remove_managed(exec: &dyn Executor, r: &Removal) -> Result<String, String> {
    let cancel = CancelToken::new();
    let path = r.file.as_str().to_owned();
    let bak = format!("{path}{}", crate::paths::BAK_SUFFIX);
    let is_link = |p: &str| user_cmd(TEST, &["-L", p]);
    let exists = |p: &str| user_cmd(TEST, &["-e", p]);

    // Read-only first; nothing has changed if one fails.
    if run(exec, &cancel, &is_link(&path)?).await?.0 == 0 {
        return Err(format!("{path} is a symbolic link and is not touched"));
    }
    let existed = run(exec, &cancel, &exists(&path)?).await?.0 == 0;
    if existed {
        let head = user_cmd(HEAD, &["-n", "1", "--", &path])?;
        let out = run_ok(exec, &cancel, &head, "reading the first line").await?;
        if out.stdout.lines().next() != Some(r.marker.as_str()) {
            return Err(format!(
                "{path} exists without this application's Ossiframe marker and is not removed"
            ));
        }
    }
    let mut link_present = false;
    if let Some(link) = &r.link {
        let l = link.as_str();
        if run(exec, &cancel, &is_link(l)?).await?.0 == 0 {
            let rl = user_cmd(READLINK, &["--", l])?;
            let out = run_ok(exec, &cancel, &rl, "reading the link").await?;
            if out.stdout.trim() != path {
                return Err(format!("{l} points somewhere else and is not touched"));
            }
            link_present = true;
        } else if run(exec, &cancel, &exists(l)?).await?.0 == 0 {
            return Err(format!("{l} exists and is not a symbolic link"));
        }
    }
    if !existed && !link_present {
        return Ok(format!("{path} is already removed"));
    }

    if existed {
        let b = v(write::backup_step("backup", &r.file))?;
        run_ok(exec, &cancel, &b.command, "backup").await?;
    }
    let mut t = Touched::default();
    let failure = match mutate(exec, &cancel, r, &path, link_present, existed, &mut t).await {
        Err(e) => Some(e),
        Ok(()) => match run(exec, &cancel, &r.test).await {
            Ok((0, _)) => None,
            Ok((_, out)) => Some(format!(
                "{} failed: {}",
                r.what,
                out.stderr
                    .lines()
                    .find(|l| !l.is_empty())
                    .unwrap_or("")
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(200)
                    .collect::<String>()
            )),
            Err(e) => Some(e),
        },
    };
    if let Some(reason) = failure {
        let outcome = restore(exec, r, &path, &bak, existed, &t).await;
        return Err(format!("{reason}. {outcome}. Nothing was reloaded"));
    }
    if existed {
        let rm = priv_cmd(RM, &["-f", "--", &bak])?;
        let _ = run(exec, &cancel, &rm).await;
    }
    match run(exec, &cancel, &r.reload).await {
        Ok((0, _)) => Ok(format!("{path} removed, {} passed, reloaded", r.what)),
        Ok((c, _)) => Err(format!(
            "the file is removed and the configuration is valid, but the reload failed (exit {c})"
        )),
        Err(e) => Err(e),
    }
}

async fn mutate(
    exec: &dyn Executor,
    cancel: &CancelToken,
    r: &Removal,
    path: &str,
    link_present: bool,
    existed: bool,
    t: &mut Touched,
) -> Result<(), String> {
    if let Some(link) = &r.link
        && link_present
    {
        let rm = priv_cmd(RM, &["--", link.as_str()])?;
        t.link_removed = true;
        run_ok(exec, cancel, &rm, "removing the link").await?;
    }
    if existed {
        let rm = priv_cmd(RM, &["--", path])?;
        t.file_removed = true;
        run_ok(exec, cancel, &rm, "removing the file").await?;
    }
    Ok(())
}

/// Puts the server back as it was, then tests again. Uses a fresh token so a
/// cancel request does not stop the restore.
async fn restore(
    exec: &dyn Executor,
    r: &Removal,
    path: &str,
    bak: &str,
    existed: bool,
    t: &Touched,
) -> String {
    let cancel = CancelToken::new();
    let mut cmds: Vec<Result<CommandSpec, String>> = Vec::new();
    if existed && t.file_removed {
        cmds.push(priv_cmd(MV, &["-T", "--", bak, path]));
    } else if existed {
        cmds.push(priv_cmd(RM, &["-f", "--", bak]));
    }
    if t.link_removed
        && let Some(l) = &r.link
    {
        cmds.push(priv_cmd(LN, &["-s", "-T", "--", path, l.as_str()]));
    }
    let mut ok = true;
    for c in cmds {
        match c {
            Ok(c) => ok &= matches!(run(exec, &cancel, &c).await, Ok((0, _))),
            Err(_) => ok = false,
        }
    }
    let retest = matches!(run(exec, &cancel, &r.test).await, Ok((0, _)));
    match (ok, retest) {
        (true, true) => format!(
            "The previous state was restored and {} passes again",
            r.what
        ),
        (true, false) => format!(
            "The previous state was restored but {} still fails; inspect the server",
            r.what
        ),
        (false, _) => {
            "Restoring the previous state did not complete; inspect the server".to_owned()
        }
    }
}

// ---------------------------------------------------------------------------
// Verification of a removal
// ---------------------------------------------------------------------------

/// Read-only: the site, link and (for PHP types) pool are gone.
pub async fn verify_removed(
    exec: &dyn Executor,
    cancel: &CancelToken,
    slug: &AppSlug,
    app_type: AppType,
) -> Vec<CheckResult> {
    let s = slug.as_str();
    let mut paths = vec![
        format!("{NGINX_SITES_ENABLED}{s}"),
        format!("{NGINX_SITES_AVAILABLE}{s}"),
    ];
    if app_type.info().uses_php {
        paths.push(format!("{FPM_POOL_DIR}{s}.conf"));
    }
    let mut gone = true;
    let mut detail = String::from("site and pool files are gone");
    for p in &paths {
        let Ok(cmd) = CommandSpec::new(TEST, &["-e", p], SHORT) else {
            gone = false;
            detail = "the check could not be built".into();
            break;
        };
        // `-e` follows links; a dangling link is checked with `-L` too.
        let link = CommandSpec::new(TEST, &["-L", p], SHORT);
        let present = match (exec.run(&cmd, cancel).await, link) {
            (Ok(a), Ok(l)) => match exec.run(&l, cancel).await {
                Ok(b) => Some(a.exit.success() || b.exit.success()),
                Err(_) => None,
            },
            _ => None,
        };
        match present {
            Some(false) => {}
            Some(true) => {
                gone = false;
                detail = format!("{p} still exists");
                break;
            }
            None => {
                gone = false;
                detail = "the files could not be checked".into();
                break;
            }
        }
    }
    vec![CheckResult {
        component: "app".to_owned(),
        id: "removed".to_owned(),
        title: "Site and pool are removed".to_owned(),
        essential: true,
        status: if gone {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        detail,
    }]
}

#[cfg(test)]
mod tests;
