//! Create plan: Linux user, directory layout, PHP-FPM pool, NGINX site and
//! MySQL database for Laravel, Php and Static applications.
//!
//! Step modes (see the S04 engine): plain commands are `Foreground` steps.
//! Work that needs a conditional sequence (back up, write, test, reload or
//! restore) or that carries a secret is a `Gate` step whose command is the
//! `install` command with the content as stdin. The engine calls
//! `Provisioner::gate`, which must delegate to [`run_gate`]; the gate runs the
//! whole sequence with the executor and returns fixed text, never command
//! output.
//!
//! Layout and modes (isolation between apps, `www-data` can serve):
//! - `app_root` `2750 <app user>:www-data`. Root sets owner and mode once, on
//!   the directory `useradd` just created and a gate has checked, before any
//!   app code can run. Setgid makes everything the app user creates inside
//!   belong to group `www-data`, so NGINX can read releases without joining
//!   any app's group. Other app users have no access (`o=---`).
//! - `releases/` and `shared/` `2750` created by the app user (group
//!   `www-data` by setgid, so NGINX can serve `shared/storage/app/public`
//!   through Laravel's `public/storage` link), `.ssh/` `0700`. `shared/.env`
//!   stays `0600` (written by the app user, never readable by `www-data`).
//! - After that, root writes nothing inside the app directory. Root writes
//!   only to root-owned managed paths under `/etc`.
use crate::app_type::{AppType, DatabaseUse};
use crate::identity::{app_slug, app_user};
use crate::paths::{AppRoot, EntryFile, FPM_POOL_DIR, ManagedPath, NGINX_SITES_AVAILABLE};
use crate::preflight::{AppUserExpectation, verify_app_user};
use crate::write::{self, FileMode};
use ossiframe_core::{
    AppSlug, CancelToken, CommandSpec, DomainName, ExecError, ExecResult, Executor, ExitStatus,
    FileChange, LinuxUserName, Plan, PlanStep, SecretString, ServiceEffect, StepMode,
    ValidationError,
};
use std::time::Duration;

pub const STEP_BASE_DIRS: &str = "base-dirs";
pub const STEP_USER_ADD: &str = "user-add";
/// Gate: the app user passes `verify_app_user`. Must precede every `RunAs`
/// step; other plans (redeploy, git access, remove) reuse this id.
pub const STEP_USER_VERIFY: &str = "app-user-verify";
pub const STEP_ROOT_CHECK: &str = "app-root-check";
pub const STEP_ROOT_OWNER: &str = "app-root-owner";
pub const STEP_ROOT_MODE: &str = "app-root-mode";
pub const STEP_RELEASES_DIR: &str = "releases-dir";
pub const STEP_SHARED_DIR: &str = "shared-dir";
pub const STEP_PRIVATE_DIRS: &str = "private-dirs";
pub const STEP_DB_APPLY: &str = "db-apply";
pub const STEP_ENV_WRITE: &str = "env-write";
pub const STEP_POOL_APPLY: &str = "pool-apply";
pub const STEP_SITE_APPLY: &str = "site-apply";
pub const STEP_SITE_APPLY_DISABLE_DEFAULT: &str = "site-apply-disable-default";

const STEP_TIMEOUT: Duration = Duration::from_secs(60);
const APPLY_DEADLINE: Duration = Duration::from_secs(300);
const WWW_DATA: &str = "www-data";
const USERADD: &str = "/usr/sbin/useradd";
const NOLOGIN: &str = "/usr/sbin/nologin";
const MYSQL: &str = "/usr/bin/mysql";
const NGINX: &str = "/usr/sbin/nginx";
const PHP_FPM: &str = "/usr/sbin/php-fpm8.3";
const SYSTEMCTL: &str = "/usr/bin/systemctl";
const DEFAULT_ENABLED: &str = "/etc/nginx/sites-enabled/default";
const DEFAULT_AVAILABLE: &str = "/etc/nginx/sites-available/default";
const MARKER_TEXT: &str = "Managed by Ossiframe. Do not edit manually.";
const ENABLED_PREFIX: &str = "/etc/nginx/sites-enabled/ossiframe-";

/// First line of the NGINX site; also the overwrite guard.
pub fn nginx_marker(slug: &AppSlug) -> String {
    format!("# {MARKER_TEXT} app={}", slug.as_str())
}

/// First line of the FPM pool file.
pub fn fpm_marker(slug: &AppSlug) -> String {
    format!("; {MARKER_TEXT} app={}", slug.as_str())
}

// ---------------------------------------------------------------------------
// Database password
// ---------------------------------------------------------------------------

/// A generated database password: 40 characters of `A-Z a-z 0-9`, safe in a
/// SQL literal, a `.env` value and a URL without any escaping. Held in memory
/// only; `Debug` and `Display` print `<redacted>`.
#[derive(Clone)]
pub struct DbPassword(SecretString);

const ALPHABET: &[u8; 62] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
const PASSWORD_LEN: usize = 40;

impl DbPassword {
    /// Draws from the operating system CSPRNG. Bytes at or above 248 are
    /// dropped so every character is equally likely.
    pub fn generate() -> Result<Self, ValidationError> {
        let mut out = String::with_capacity(PASSWORD_LEN);
        let mut buf = [0u8; 64];
        while out.len() < PASSWORD_LEN {
            getrandom::fill(&mut buf).map_err(|_| {
                ValidationError::new("password", "the system random source is unavailable")
            })?;
            for b in buf {
                if b < 248 && out.len() < PASSWORD_LEN {
                    out.push(ALPHABET[(b % 62) as usize] as char);
                }
            }
        }
        Ok(Self(SecretString::new(out)))
    }

    /// For a password that already exists. At least 32 alphanumerics.
    pub fn parse(input: &str) -> Result<Self, ValidationError> {
        if input.len() < 32 || !input.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(ValidationError::new(
                "password",
                "must be at least 32 letters and digits",
            ));
        }
        Ok(Self(SecretString::new(input.to_owned())))
    }

    fn expose(&self) -> &str {
        self.0.expose()
    }
}

impl std::fmt::Debug for DbPassword {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DbPassword(<redacted>)")
    }
}

// ---------------------------------------------------------------------------
// Input and output
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct CreateInput {
    pub app_type: AppType,
    pub slug: AppSlug,
    pub domain: DomainName,
    pub entry_file: EntryFile,
    /// Opt-in for Php. Ignored for Laravel (always), refused for Static.
    pub with_database: bool,
    /// Remove the `sites-enabled/default` symlink (never the file).
    pub disable_default_site: bool,
}

impl CreateInput {
    /// Whether this app gets a database.
    pub fn database(&self) -> Result<bool, ValidationError> {
        match (self.app_type.info().database, self.with_database) {
            (DatabaseUse::Default, _) => Ok(true),
            (DatabaseUse::OptIn, opt) => Ok(opt),
            (DatabaseUse::Never, false) => Ok(false),
            (DatabaseUse::Never, true) => Err(ValidationError::new(
                "database",
                "a static site does not get a database",
            )),
        }
    }
}

/// What the create plan adds to a `Plan`.
#[derive(Debug, Clone)]
pub struct CreateSteps {
    pub steps: Vec<PlanStep>,
    pub files: Vec<FileChange>,
    pub services: Vec<ServiceEffect>,
}

fn step(id: &str, title: &str, command: CommandSpec, mode: StepMode) -> PlanStep {
    PlanStep {
        id: id.to_owned(),
        title: title.to_owned(),
        command,
        mode,
        deadline: if mode == StepMode::Gate {
            APPLY_DEADLINE
        } else {
            STEP_TIMEOUT
        },
        requires_stdin: false,
    }
}

fn db_names(slug: &AppSlug) -> String {
    format!("ossi_{}", slug.as_str())
}

// ---------------------------------------------------------------------------
// Rendering (every interpolated value is a validated type)
// ---------------------------------------------------------------------------

pub fn render_pool(slug: &AppSlug, user: &LinuxUserName, root: &AppRoot) -> String {
    let s = slug.as_str();
    let u = user.as_str();
    [
        fpm_marker(slug),
        format!("[{u}]"),
        format!("user = {u}"),
        format!("group = {u}"),
        format!("listen = /run/php/ossiframe-{s}.sock"),
        format!("listen.owner = {WWW_DATA}"),
        format!("listen.group = {WWW_DATA}"),
        "listen.mode = 0660".to_owned(),
        "pm = dynamic".to_owned(),
        "pm.max_children = 5".to_owned(),
        "pm.start_servers = 2".to_owned(),
        "pm.min_spare_servers = 1".to_owned(),
        "pm.max_spare_servers = 3".to_owned(),
        "clear_env = yes".to_owned(),
        "security.limit_extensions = .php".to_owned(),
        // The pool can read its own app and temp files only.
        format!("php_admin_value[open_basedir] = {}:/tmp", root.as_str()),
        String::new(),
    ]
    .join("\n")
}

/// Document root: the entry file's directory under `current`
/// (`current/public` for Laravel).
fn document_root(root: &AppRoot, entry: &EntryFile) -> String {
    match entry.dir() {
        Some(d) => format!("{}/current/{d}", root.as_str()),
        None => format!("{}/current", root.as_str()),
    }
}

pub fn render_site(input: &CreateInput, root: &AppRoot) -> String {
    let slug = input.slug.as_str();
    let entry_name = input
        .entry_file
        .as_str()
        .rsplit('/')
        .next()
        .unwrap_or("index.html");
    let uses_php = input.app_type.info().uses_php;
    let mut l: Vec<String> = vec![
        nginx_marker(&input.slug),
        "server {".to_owned(),
        "    listen 80;".to_owned(),
        "    listen [::]:80;".to_owned(),
        format!("    server_name {};", input.domain.as_str()),
        format!("    root {};", document_root(root, &input.entry_file)),
        format!("    index {entry_name};"),
        // Serve a link only when its owner also owns the target: `current`
        // and the release belong to the app user, but a link planted by app
        // code to a file of another user (for example under /etc) is refused.
        "    disable_symlinks if_not_owner;".to_owned(),
        format!("    add_header X-Ossiframe-App {slug} always;"),
        "    location ~ /\\.(?!well-known) { deny all; }".to_owned(),
    ];
    if input.app_type == AppType::Laravel {
        l.push("    location / { try_files $uri $uri/ /index.php?$query_string; }".to_owned());
    } else {
        l.push("    location / { try_files $uri $uri/ =404; }".to_owned());
    }
    if uses_php {
        l.extend([
            "    location ~ \\.php$ {".to_owned(),
            "        try_files $uri =404;".to_owned(),
            "        include fastcgi.conf;".to_owned(),
            format!("        fastcgi_pass unix:/run/php/ossiframe-{slug}.sock;"),
            "    }".to_owned(),
        ]);
    }
    l.push("}".to_owned());
    l.push(String::new());
    l.join("\n")
}

/// The statements for the app database. Only `ossi_<slug>` is named. There is
/// no `ALTER USER`: an existing user's password is never changed.
pub fn render_sql(slug: &AppSlug, password: &DbPassword) -> String {
    let n = db_names(slug);
    format!(
        "CREATE DATABASE IF NOT EXISTS `{n}`;\n\
         CREATE USER IF NOT EXISTS '{n}'@'localhost' IDENTIFIED BY '{}';\n\
         GRANT ALL PRIVILEGES ON `{n}`.* TO '{n}'@'localhost';\n",
        password.expose()
    )
}

pub fn render_env(input: &CreateInput, password: &DbPassword) -> String {
    let n = db_names(&input.slug);
    let mut l: Vec<String> = Vec::new();
    if input.app_type == AppType::Laravel {
        l.extend([
            "APP_ENV=production".to_owned(),
            "APP_DEBUG=false".to_owned(),
            format!("APP_URL=http://{}", input.domain.as_str()),
            // `php artisan key:generate --force` fills this in at deploy.
            "APP_KEY=".to_owned(),
        ]);
    }
    l.extend([
        "DB_CONNECTION=mysql".to_owned(),
        "DB_HOST=127.0.0.1".to_owned(),
        "DB_PORT=3306".to_owned(),
        format!("DB_DATABASE={n}"),
        format!("DB_USERNAME={n}"),
        format!("DB_PASSWORD={}", password.expose()),
        String::new(),
    ]);
    l.join("\n")
}

// ---------------------------------------------------------------------------
// Plan building
// ---------------------------------------------------------------------------

pub fn create_steps(
    input: &CreateInput,
    password: &DbPassword,
) -> Result<CreateSteps, ValidationError> {
    let info = input.app_type.info();
    let with_db = input.database()?;
    if info.entry_file_fixed && input.entry_file.as_str() != info.entry_file {
        return Err(ValidationError::new(
            "entryFile",
            format!("must be {} for this application type", info.entry_file),
        ));
    }
    // Re-applies the length cap even if the slug came from elsewhere.
    let slug = app_slug(input.slug.as_str())?;
    let user = app_user(&slug)?;
    let root = AppRoot::new(&slug)?;
    let r = root.as_str();
    let u = user.as_str();
    let owner = format!("{u}:{WWW_DATA}");
    let t = STEP_TIMEOUT;
    let fg = StepMode::Foreground;

    let mut steps = vec![
        step(
            STEP_BASE_DIRS,
            "Create /srv/ossiframe/apps",
            CommandSpec::privileged(
                "/usr/bin/install",
                &[
                    "-d",
                    "-m",
                    "0755",
                    "-o",
                    "root",
                    "-g",
                    "root",
                    "--",
                    "/srv/ossiframe",
                    "/srv/ossiframe/apps",
                ],
                t,
            )?,
            fg,
        ),
        step(
            STEP_USER_ADD,
            &format!("Create the Linux user {u}"),
            CommandSpec::privileged(
                USERADD,
                &[
                    "--create-home",
                    "--home-dir",
                    r,
                    "--shell",
                    NOLOGIN,
                    "--user-group",
                    "--comment",
                    "ossiframe-app",
                    u,
                ],
                t,
            )?,
            fg,
        ),
        step(
            STEP_USER_VERIFY,
            &format!("Verify {u} is a safe application user"),
            CommandSpec::new("/usr/bin/getent", &["passwd", "--", u], t)?,
            StepMode::Gate,
        ),
        step(
            STEP_ROOT_CHECK,
            "Check the new app directory is a real directory owned by the app user",
            CommandSpec::new("/usr/bin/stat", &["-c", "%F:%U:%G", "--", r], t)?,
            StepMode::Gate,
        ),
        step(
            STEP_ROOT_OWNER,
            &format!("Set the owner of {r} to {owner}"),
            CommandSpec::privileged("/usr/bin/chown", &["--no-dereference", "--", &owner, r], t)?,
            fg,
        ),
        step(
            STEP_ROOT_MODE,
            &format!("Set the mode of {r} to 2750"),
            CommandSpec::privileged("/usr/bin/chmod", &["2750", "--", r], t)?,
            fg,
        ),
        step(
            STEP_RELEASES_DIR,
            "Create releases/ as the app user",
            CommandSpec::run_as(
                user.clone(),
                "/usr/bin/mkdir",
                &["-m", "2750", "--", &format!("{r}/releases")],
                t,
            )?,
            fg,
        ),
        step(
            STEP_SHARED_DIR,
            "Create shared/ (2750, group www-data) as the app user",
            CommandSpec::run_as(
                user.clone(),
                "/usr/bin/mkdir",
                &["-m", "2750", "--", &format!("{r}/shared")],
                t,
            )?,
            fg,
        ),
        step(
            STEP_PRIVATE_DIRS,
            "Create .ssh/ (0700) as the app user",
            CommandSpec::run_as(
                user.clone(),
                "/usr/bin/mkdir",
                &["-m", "0700", "--", &format!("{r}/.ssh")],
                t,
            )?,
            fg,
        ),
    ];
    let mut files = vec![
        FileChange {
            path: r.to_owned(),
            description: format!("Application directory, {owner}, mode 2750"),
        },
        FileChange {
            path: format!("{r}/releases"),
            description: "Release directories, app user, mode 2750".to_owned(),
        },
        FileChange {
            path: format!("{r}/shared"),
            description: "Shared files, mode 2750 (group www-data; .env stays 0600)".to_owned(),
        },
        FileChange {
            path: format!("{r}/.ssh"),
            description: "SSH files, app user only, mode 0700".to_owned(),
        },
    ];
    let mut services = Vec::new();

    if with_db {
        let n = db_names(&slug);
        let mut sql = step(
            STEP_DB_APPLY,
            &format!("Create the MySQL database and user {n} (kept if they already exist)"),
            CommandSpec::privileged(MYSQL, &["--no-defaults"], t)?
                .with_stdin(render_sql(&slug, password).into_bytes())?,
            StepMode::Gate,
        );
        sql.requires_stdin = true;
        steps.push(sql);
        let env_path = write::app_path(&root, "shared/.env")?;
        let mut env = step(
            STEP_ENV_WRITE,
            "Write shared/.env with the database password (never overwrites an existing file)",
            write::app_write(
                "env",
                &user,
                &env_path,
                FileMode::M0600,
                render_env(input, password).into_bytes(),
            )?
            .remove(0)
            .command,
            StepMode::Gate,
        );
        env.requires_stdin = true;
        steps.push(env);
        files.push(FileChange {
            path: env_path.as_str().to_owned(),
            description: "Environment file with the database password, app user, mode 0600"
                .to_owned(),
        });
        services.push(ServiceEffect {
            unit: "mysql".to_owned(),
            effect: format!("database and user {n} are created if missing"),
        });
    }

    if info.uses_php {
        let path = ManagedPath::parse(&format!("{FPM_POOL_DIR}{}.conf", slug.as_str()))?;
        let mut pool = step(
            STEP_POOL_APPLY,
            "Write the PHP-FPM pool, test it and reload (restored on failure)",
            write::root_write(
                "pool",
                &path,
                FileMode::M0644,
                render_pool(&slug, &user, &root).into_bytes(),
            )?
            .remove(0)
            .command,
            StepMode::Gate,
        );
        pool.requires_stdin = true;
        steps.push(pool);
        files.push(FileChange {
            path: path.as_str().to_owned(),
            description: "PHP-FPM pool, root, mode 0644".to_owned(),
        });
        services.push(ServiceEffect {
            unit: "php8.3-fpm".to_owned(),
            effect: "reload".to_owned(),
        });
    }

    let site = ManagedPath::parse(&format!("{NGINX_SITES_AVAILABLE}{}", slug.as_str()))?;
    let mut site_step = step(
        if input.disable_default_site {
            STEP_SITE_APPLY_DISABLE_DEFAULT
        } else {
            STEP_SITE_APPLY
        },
        if input.disable_default_site {
            "Write the NGINX site, enable it, disable the default site, test and reload (restored on failure)"
        } else {
            "Write the NGINX site, enable it, test and reload (restored on failure)"
        },
        write::root_write(
            "site",
            &site,
            FileMode::M0644,
            render_site(input, &root).into_bytes(),
        )?
        .remove(0)
        .command,
        StepMode::Gate,
    );
    site_step.requires_stdin = true;
    steps.push(site_step);
    files.push(FileChange {
        path: site.as_str().to_owned(),
        description: "NGINX site, root, mode 0644".to_owned(),
    });
    files.push(FileChange {
        path: format!("{ENABLED_PREFIX}{}", slug.as_str()),
        description: format!("Symlink to {}", site.as_str()),
    });
    if input.disable_default_site {
        files.push(FileChange {
            path: DEFAULT_ENABLED.to_owned(),
            description: "Symlink removed (the default site file is kept)".to_owned(),
        });
    }
    services.push(ServiceEffect {
        unit: "nginx".to_owned(),
        effect: "reload".to_owned(),
    });
    Ok(CreateSteps {
        steps,
        files,
        services,
    })
}

// ---------------------------------------------------------------------------
// Gate dispatch
// ---------------------------------------------------------------------------

/// What the database gate decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvState {
    NoFile,
    NoPassword,
    HasPassword,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbDecision {
    /// Run the SQL; `.env` is written next.
    Create,
    /// The user and its recorded password exist: do nothing.
    Keep,
    /// Never guess: a person must look.
    Block(&'static str),
}

/// Never resets a password and never overwrites a `.env`. The case of a
/// `.env` password with a missing user is blocked rather than read back into
/// the backend.
pub fn decide_db(user_exists: bool, env: EnvState) -> DbDecision {
    match (user_exists, env) {
        (false, EnvState::NoFile) => DbDecision::Create,
        (true, EnvState::HasPassword) => DbDecision::Keep,
        (true, _) => DbDecision::Block(
            "the database user exists without a recorded password; inspect it manually",
        ),
        (false, EnvState::HasPassword) => DbDecision::Block(
            "shared/.env has a DB_PASSWORD but the database user is missing; inspect manually",
        ),
        (false, EnvState::NoPassword) => DbDecision::Block(
            "shared/.env exists without a DB_PASSWORD and is not overwritten; inspect manually",
        ),
    }
}

pub(crate) fn v<T>(r: Result<T, ValidationError>) -> Result<T, String> {
    r.map_err(|e| e.to_string())
}

fn clean(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(200).collect()
}

/// Runs one command. Transport failures are `Err`; the exit code is data.
pub(crate) async fn run(
    exec: &dyn Executor,
    cancel: &CancelToken,
    spec: &CommandSpec,
) -> Result<(i32, ExecResult), String> {
    match exec.run(spec, cancel).await {
        Ok(r) => match r.exit {
            ExitStatus::Code(c) => Ok((c, r)),
            _ => Err(format!("{} ended without an exit code", spec.program())),
        },
        Err(ExecError::ConnectionLost) => Err(
            "the connection was lost; the state on the server is unknown, inspect it".to_owned(),
        ),
        Err(e) => Err(clean(&e.to_string())),
    }
}

pub(crate) async fn run_ok(
    exec: &dyn Executor,
    cancel: &CancelToken,
    spec: &CommandSpec,
    what: &str,
) -> Result<ExecResult, String> {
    let (code, r) = run(exec, cancel, spec).await?;
    if code == 0 {
        Ok(r)
    } else {
        Err(format!("{what} failed (exit {code})"))
    }
}

pub(crate) fn find<'a>(plan: &'a Plan, id: &str) -> Result<&'a PlanStep, String> {
    plan.steps
        .iter()
        .find(|s| s.id == id)
        .ok_or_else(|| format!("the plan has no step {id}"))
}

/// The app slug, read back from the verify step's user argument.
pub(crate) fn plan_slug(plan: &Plan) -> Result<AppSlug, String> {
    let step = find(plan, STEP_USER_VERIFY)?;
    let name = step.command.args().last().map(String::as_str).unwrap_or("");
    let slug = name.strip_prefix("ossi-").ok_or("unexpected app user")?;
    v(app_slug(slug))
}

/// Delegation target of `Provisioner::gate` for the create plan.
pub async fn run_gate(
    exec: &dyn Executor,
    cancel: &CancelToken,
    plan: &Plan,
    step_id: &str,
) -> Result<String, String> {
    match step_id {
        STEP_USER_VERIFY => verify_user_gate(exec, cancel, plan).await,
        STEP_ROOT_CHECK => root_check_gate(exec, cancel, plan).await,
        STEP_DB_APPLY => db_gate(exec, cancel, plan).await,
        STEP_ENV_WRITE => env_gate(exec, cancel, plan).await,
        crate::git_access::STEP_DEPLOY_KEY_GENERATE | crate::git_access::STEP_DEPLOY_KEY_CHECK => {
            crate::git_access::run_gate(exec, cancel, plan, step_id).await
        }
        id if crate::release::is_gate(id) => {
            crate::release::run_gate(exec, cancel, plan, step_id).await
        }
        id if crate::remove::is_gate(id) => {
            crate::remove::run_gate(exec, cancel, plan, step_id).await
        }
        STEP_POOL_APPLY => {
            let slug = plan_slug(plan)?;
            let a = Apply {
                install: &find(plan, step_id)?.command,
                marker: fpm_marker(&slug),
                link: None,
                disable_default: false,
                test: v(CommandSpec::privileged(PHP_FPM, &["-t"], STEP_TIMEOUT))?,
                reload: v(CommandSpec::privileged(
                    SYSTEMCTL,
                    &["reload", "php8.3-fpm"],
                    STEP_TIMEOUT,
                ))?,
                what: "php-fpm8.3 -t",
            };
            apply_managed(exec, &a).await
        }
        STEP_SITE_APPLY | STEP_SITE_APPLY_DISABLE_DEFAULT => {
            let slug = plan_slug(plan)?;
            let a = Apply {
                install: &find(plan, step_id)?.command,
                marker: nginx_marker(&slug),
                link: Some(v(ManagedPath::parse(&format!(
                    "{ENABLED_PREFIX}{}",
                    slug.as_str()
                )))?),
                disable_default: step_id == STEP_SITE_APPLY_DISABLE_DEFAULT,
                test: v(CommandSpec::privileged(NGINX, &["-t"], STEP_TIMEOUT))?,
                reload: v(CommandSpec::privileged(
                    SYSTEMCTL,
                    &["reload", "nginx"],
                    STEP_TIMEOUT,
                ))?,
                what: "nginx -t",
            };
            apply_managed(exec, &a).await
        }
        other => Err(format!("unknown check {other}")),
    }
}

async fn verify_user_gate(
    exec: &dyn Executor,
    cancel: &CancelToken,
    plan: &Plan,
) -> Result<String, String> {
    let slug = plan_slug(plan)?;
    let user = v(app_user(&slug))?;
    let conflicts = verify_app_user(
        exec,
        cancel,
        &slug,
        &AppUserExpectation::Existing {
            recorded_user: user.clone(),
        },
    )
    .await
    .map_err(|e| clean(&e.to_string()))?;
    if conflicts.is_empty() {
        Ok(format!("{} is a safe application user", user.as_str()))
    } else {
        Err(conflicts
            .iter()
            .map(|c| c.message.as_str())
            .collect::<Vec<_>>()
            .join(" "))
    }
}

async fn root_check_gate(
    exec: &dyn Executor,
    cancel: &CancelToken,
    plan: &Plan,
) -> Result<String, String> {
    let slug = plan_slug(plan)?;
    let u = v(app_user(&slug))?;
    let step = find(plan, STEP_ROOT_CHECK)?;
    let r = run_ok(exec, cancel, &step.command, "stat").await?;
    let want = format!("directory:{0}:{0}", u.as_str());
    if r.stdout.trim() == want {
        Ok("the app directory is a real directory owned by the app user".to_owned())
    } else {
        Err("the app directory already existed or is not a plain directory owned by the new app user; nothing was changed".to_owned())
    }
}

fn sql_count(slug: &AppSlug) -> Result<CommandSpec, String> {
    let q = format!(
        "SELECT COUNT(*) FROM mysql.user WHERE User='{}' AND Host='localhost'",
        db_names(slug)
    );
    v(CommandSpec::privileged(
        MYSQL,
        &["--no-defaults", "-N", "-B", "-e", &q],
        STEP_TIMEOUT,
    ))
}

async fn env_state(
    exec: &dyn Executor,
    cancel: &CancelToken,
    user: &LinuxUserName,
    env_path: &str,
) -> Result<EnvState, String> {
    let exists = v(CommandSpec::run_as(
        user.clone(),
        "/usr/bin/test",
        &["-f", env_path],
        STEP_TIMEOUT,
    ))?;
    match run(exec, cancel, &exists).await?.0 {
        0 => {}
        1 => return Ok(EnvState::NoFile),
        _ => return Err("shared/.env could not be checked".to_owned()),
    }
    let grep = v(CommandSpec::run_as(
        user.clone(),
        "/usr/bin/grep",
        &["-q", "-e", "^DB_PASSWORD=.", "--", env_path],
        STEP_TIMEOUT,
    ))?;
    match run(exec, cancel, &grep).await?.0 {
        0 => Ok(EnvState::HasPassword),
        1 => Ok(EnvState::NoPassword),
        _ => Err("shared/.env could not be read".to_owned()),
    }
}

async fn db_gate(exec: &dyn Executor, cancel: &CancelToken, plan: &Plan) -> Result<String, String> {
    let slug = plan_slug(plan)?;
    let user = v(app_user(&slug))?;
    let root = v(AppRoot::new(&slug))?;
    let env_path = format!("{}/shared/.env", root.as_str());
    let step = find(plan, STEP_DB_APPLY)?;
    if !step.command.has_stdin() {
        return Err(
            "the plan no longer holds its input: re-plan required. Nothing was run".to_owned(),
        );
    }
    let count = run_ok(exec, cancel, &sql_count(&slug)?, "mysql").await?;
    let user_exists = match count.stdout.trim() {
        "0" => false,
        "1" => true,
        _ => return Err("the database user could not be checked".to_owned()),
    };
    let env = env_state(exec, cancel, &user, &env_path).await?;
    match decide_db(user_exists, env) {
        DbDecision::Keep => {
            Ok("the database user and its recorded password already exist; kept".to_owned())
        }
        DbDecision::Block(m) => Err(m.to_owned()),
        DbDecision::Create => {
            // The output is dropped: a client error can quote the statement.
            match run(exec, cancel, &step.command).await? {
                (0, _) => Ok("database and user created".to_owned()),
                (c, _) => Err(format!("mysql failed (exit {c}); its output is not shown")),
            }
        }
    }
}

async fn env_gate(
    exec: &dyn Executor,
    cancel: &CancelToken,
    plan: &Plan,
) -> Result<String, String> {
    let slug = plan_slug(plan)?;
    let user = v(app_user(&slug))?;
    let root = v(AppRoot::new(&slug))?;
    let env_path = format!("{}/shared/.env", root.as_str());
    let step = find(plan, STEP_ENV_WRITE)?;
    if !step.command.has_stdin() {
        return Err(
            "the plan no longer holds its input: re-plan required. Nothing was run".to_owned(),
        );
    }
    match env_state(exec, cancel, &user, &env_path).await? {
        EnvState::NoFile => {}
        EnvState::HasPassword => {
            return Ok("shared/.env already exists; kept".to_owned());
        }
        EnvState::NoPassword => {
            return Err(
                "shared/.env exists without a DB_PASSWORD and is not overwritten".to_owned(),
            );
        }
    }
    run_ok(exec, cancel, &step.command, "write .env").await?;
    let mv = v(write::move_for(&step.command))?;
    run_ok(exec, cancel, &mv, "write .env").await?;
    Ok("shared/.env written".to_owned())
}

// ---------------------------------------------------------------------------
// Write, test, reload or restore (correction 2)
// ---------------------------------------------------------------------------

struct Apply<'a> {
    install: &'a CommandSpec,
    marker: String,
    /// The `sites-enabled` link to create (site only).
    link: Option<ManagedPath>,
    disable_default: bool,
    test: CommandSpec,
    reload: CommandSpec,
    what: &'static str,
}

#[derive(Default)]
struct Touched {
    wrote: bool,
    link_created: bool,
    default_removed: bool,
}

fn priv_cmd(program: &str, args: &[&str]) -> Result<CommandSpec, String> {
    v(CommandSpec::privileged(program, args, STEP_TIMEOUT))
}

fn user_cmd(program: &str, args: &[&str]) -> Result<CommandSpec, String> {
    v(CommandSpec::new(program, args, STEP_TIMEOUT))
}

async fn apply_managed(exec: &dyn Executor, a: &Apply<'_>) -> Result<String, String> {
    let cancel = CancelToken::new();
    let content = a
        .install
        .stdin()
        .ok_or("the plan no longer holds its input: re-plan required. Nothing was run")?;
    if content.split(|b| *b == b'\n').next() != Some(a.marker.as_bytes()) {
        return Err("the content does not start with the Ossiframe marker line".to_owned());
    }
    let path = v(write::final_path_of(a.install))?;
    let tmp = format!("{path}{}", crate::paths::TMP_SUFFIX);
    let bak = format!("{path}{}", crate::paths::BAK_SUFFIX);
    let managed = v(ManagedPath::parse(&path))?;

    // Read-only checks first; nothing has changed if one fails.
    let is_link = |p: &str| user_cmd("/usr/bin/test", &["-L", p]);
    let exists = |p: &str| user_cmd("/usr/bin/test", &["-e", p]);
    if run(exec, &cancel, &is_link(&path)?).await?.0 == 0 {
        return Err(format!("{path} is a symbolic link and is not touched"));
    }
    let existed = run(exec, &cancel, &exists(&path)?).await?.0 == 0;
    if existed {
        let head = user_cmd("/usr/bin/head", &["-n", "1", "--", &path])?;
        let r = run_ok(exec, &cancel, &head, "reading the first line").await?;
        if r.stdout.lines().next() != Some(a.marker.as_str()) {
            return Err(format!(
                "{path} exists without this application's Ossiframe marker and is not overwritten"
            ));
        }
    }
    let mut link_exists = false;
    if let Some(link) = &a.link {
        let l = link.as_str();
        if run(exec, &cancel, &is_link(l)?).await?.0 == 0 {
            let rl = user_cmd("/usr/bin/readlink", &["--", l])?;
            let r = run_ok(exec, &cancel, &rl, "reading the link").await?;
            if r.stdout.trim() != path {
                return Err(format!("{l} points somewhere else and is not touched"));
            }
            link_exists = true;
        } else if run(exec, &cancel, &exists(l)?).await?.0 == 0 {
            return Err(format!("{l} exists and is not a symbolic link"));
        }
    }
    let mut default_present = false;
    if a.disable_default {
        if run(exec, &cancel, &is_link(DEFAULT_ENABLED)?).await?.0 == 0 {
            let rl = user_cmd("/usr/bin/readlink", &["--", DEFAULT_ENABLED])?;
            let r = run_ok(exec, &cancel, &rl, "reading the default site link").await?;
            if r.stdout.trim() != DEFAULT_AVAILABLE {
                return Err(format!(
                    "{DEFAULT_ENABLED} does not point to {DEFAULT_AVAILABLE}; the default site is not touched"
                ));
            }
            default_present = true;
        } else if run(exec, &cancel, &exists(DEFAULT_ENABLED)?).await?.0 == 0 {
            return Err(format!(
                "{DEFAULT_ENABLED} is not a symbolic link; not touched"
            ));
        }
    }

    if existed {
        let b = v(write::backup_step("backup", &managed))?;
        run_ok(exec, &cancel, &b.command, "backup").await?;
    }

    let mut t = Touched::default();
    let failure = match mutate(
        exec,
        &cancel,
        a,
        &path,
        default_present,
        link_exists,
        &mut t,
    )
    .await
    {
        Err(e) => Some(e),
        Ok(()) => match run(exec, &cancel, &a.test).await {
            Ok((0, _)) => None,
            Ok((_, r)) => Some(format!(
                "{} failed: {}",
                a.what,
                clean(r.stderr.lines().find(|l| !l.is_empty()).unwrap_or(""))
            )),
            Err(e) => Some(e),
        },
    };
    if let Some(reason) = failure {
        let outcome = restore(exec, a, &path, &tmp, &bak, existed, &t).await;
        return Err(format!("{reason}. {outcome}. Nothing was reloaded"));
    }
    if existed {
        let rm = priv_cmd("/usr/bin/rm", &["-f", "--", &bak])?;
        let _ = run(exec, &cancel, &rm).await;
    }
    match run(exec, &cancel, &a.reload).await {
        Ok((0, _)) => Ok(format!("{path} written, {} passed, reloaded", a.what)),
        Ok((c, _)) => Err(format!(
            "the new configuration is in place and valid, but the reload failed (exit {c})"
        )),
        Err(e) => Err(e),
    }
}

async fn mutate(
    exec: &dyn Executor,
    cancel: &CancelToken,
    a: &Apply<'_>,
    path: &str,
    default_present: bool,
    link_exists: bool,
    t: &mut Touched,
) -> Result<(), String> {
    run_ok(exec, cancel, a.install, "writing the temporary file").await?;
    let mv = v(write::move_for(a.install))?;
    t.wrote = true;
    run_ok(exec, cancel, &mv, "replacing the file").await?;
    if let Some(link) = &a.link
        && !link_exists
    {
        let ln = priv_cmd("/usr/bin/ln", &["-s", "-T", "--", path, link.as_str()])?;
        t.link_created = true;
        run_ok(exec, cancel, &ln, "enabling the site").await?;
    }
    if a.disable_default && default_present {
        let rm = priv_cmd("/usr/bin/rm", &["--", DEFAULT_ENABLED])?;
        t.default_removed = true;
        run_ok(exec, cancel, &rm, "disabling the default site").await?;
    }
    Ok(())
}

/// Puts the server back as it was, then tests again. Uses a fresh token so a
/// cancel request does not stop the restore. Returns a status sentence.
async fn restore(
    exec: &dyn Executor,
    a: &Apply<'_>,
    path: &str,
    tmp: &str,
    bak: &str,
    existed: bool,
    t: &Touched,
) -> String {
    let cancel = CancelToken::new();
    let mut ok = true;
    let mut cmds: Vec<Result<CommandSpec, String>> =
        vec![priv_cmd("/usr/bin/rm", &["-f", "--", tmp])];
    if t.default_removed {
        cmds.push(priv_cmd(
            "/usr/bin/ln",
            &["-s", "-T", "--", DEFAULT_AVAILABLE, DEFAULT_ENABLED],
        ));
    }
    if t.link_created
        && let Some(l) = &a.link
    {
        cmds.push(priv_cmd("/usr/bin/rm", &["--", l.as_str()]));
    }
    if existed && t.wrote {
        cmds.push(priv_cmd("/usr/bin/mv", &["-T", "--", bak, path]));
    } else if existed {
        cmds.push(priv_cmd("/usr/bin/rm", &["-f", "--", bak]));
    } else if t.wrote {
        cmds.push(priv_cmd("/usr/bin/rm", &["-f", "--", path]));
    }
    for c in cmds {
        match c {
            Ok(c) => ok &= matches!(run(exec, &cancel, &c).await, Ok((0, _))),
            Err(_) => ok = false,
        }
    }
    let retest = matches!(run(exec, &cancel, &a.test).await, Ok((0, _)));
    match (ok, retest) {
        (true, true) => format!(
            "The previous state was restored and {} passes again",
            a.what
        ),
        (true, false) => format!(
            "The previous state was restored but {} still fails; inspect the server",
            a.what
        ),
        (false, _) => {
            "Restoring the previous state did not complete; inspect the server".to_owned()
        }
    }
}

#[cfg(test)]
mod tests;
