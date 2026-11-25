//! Post-switch verification and recovery evidence for an application.
//!
//! Health checks ([`verify_app`]), all read-only and all essential:
//! - `current` points at the release just deployed.
//! - HTTP: `curl` on the server against `127.0.0.1` with the app's `Host`
//!   header. A status in 200..=399 passes only together with the header
//!   `X-Ossiframe-App: <slug>` set by the managed site, so the default site
//!   or another app cannot pass (correction 9). A missing `curl` is
//!   `Skipped`: the app is not verified.
//! - PHP-FPM (Laravel, Php): the service is active and the pool socket
//!   exists.
//! - Laravel: `APP_KEY` is set. The grep reports only an exit code, never
//!   the value.
//!
//! A failed essential check fails the operation. `current` stays on the new
//! release and nothing rolls back automatically.
//!
//! [`reconcile_app`] is the pure decision for recovery (correction 8): the
//! observed app state against the expected release.

use crate::app_type::AppType;
use crate::create::{run, v};
use crate::identity::{app_slug, app_user};
use crate::paths::AppRoot;
use crate::release::{
    AppObservation, CurrentLink, FileState, ReleaseId, STEP_SWITCH_LINK, observe_app_state,
};
use ossiframe_core::{
    AppRecovery, AppRecoveryVerdict, AppSlug, CancelToken, CheckResult, CheckStatus, CommandSpec,
    DomainName, Executor, Plan, ValidationError,
};
use std::time::Duration;

pub const CHECK_CURRENT: &str = "current";
pub const CHECK_HTTP: &str = "http";
pub const CHECK_FPM: &str = "fpm";
pub const CHECK_APP_KEY: &str = "app-key";

const CURL: &str = "/usr/bin/curl";
const TEST: &str = "/usr/bin/test";
const GREP: &str = "/usr/bin/grep";
const SHORT: Duration = Duration::from_secs(20);
/// The shell's "command not found / not executable" exit code.
const EXIT_NOT_FOUND: i32 = 127;
pub const APP_HEADER: &str = "x-ossiframe-app";

/// What the checks need to know about the app.
#[derive(Debug, Clone)]
pub struct VerifyInput {
    pub app_type: AppType,
    pub slug: AppSlug,
    pub domain: DomainName,
    /// The release the operation switched to.
    pub release: ReleaseId,
}

/// The release a deploy or rollback plan switches to, read from its link
/// step.
pub fn release_of_plan(plan: &Plan) -> Result<ReleaseId, String> {
    let slug = crate::create::plan_slug(plan)?;
    let root = v(AppRoot::new(&slug))?;
    let step = crate::create::find(plan, STEP_SWITCH_LINK)?;
    let dir = step
        .command
        .args()
        .get(2)
        .ok_or("the plan has no switch step")?;
    dir.strip_prefix(&format!("{}/releases/", root.as_str()))
        .and_then(|id| ReleaseId::parse(id).ok())
        .ok_or_else(|| "the switch step does not name a release of this app".to_owned())
}

// ---------------------------------------------------------------------------
// Parsers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpOutcome {
    Pass {
        status: u16,
    },
    BadStatus(u16),
    /// A good status without the per-app header: another server block answered.
    WrongSite {
        status: u16,
    },
    Unparsable,
}

/// `curl -D -` output: the last header block decides. The status must be in
/// 200..=399 and `X-Ossiframe-App` must equal the slug.
pub fn parse_http(stdout: &str, slug: &AppSlug) -> HttpOutcome {
    let blocks: Vec<&str> = stdout
        .split("\r\n\r\n")
        .flat_map(|b| b.split("\n\n"))
        .filter(|b| b.trim_start().starts_with("HTTP/"))
        .collect();
    let Some(block) = blocks.last() else {
        return HttpOutcome::Unparsable;
    };
    let mut lines = block.trim_start().lines();
    let status = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse::<u16>().ok());
    let Some(status) = status else {
        return HttpOutcome::Unparsable;
    };
    if !(200..=399).contains(&status) {
        return HttpOutcome::BadStatus(status);
    }
    let tagged = lines.any(|l| {
        l.split_once(':').is_some_and(|(k, val)| {
            k.trim().eq_ignore_ascii_case(APP_HEADER) && val.trim() == slug.as_str()
        })
    });
    if tagged {
        HttpOutcome::Pass { status }
    } else {
        HttpOutcome::WrongSite { status }
    }
}

/// FPM is healthy: the service is active and the pool socket exists.
pub fn fpm_ok(service_active: Option<bool>, socket_exit: i32) -> Result<(), String> {
    match (service_active, socket_exit) {
        (Some(true), 0) => Ok(()),
        (Some(true), 1) => Err("the PHP-FPM pool socket does not exist".to_owned()),
        (Some(false), _) => Err("php8.3-fpm is not active".to_owned()),
        _ => Err("PHP-FPM could not be checked".to_owned()),
    }
}

/// `grep -q` of `APP_KEY=base64:`: 0 set, 1 not set, anything else unknown.
pub fn app_key_ok(grep_exit: i32) -> Result<(), String> {
    match grep_exit {
        0 => Ok(()),
        1 => Err("APP_KEY is not set in shared/.env".to_owned()),
        _ => Err("shared/.env could not be read".to_owned()),
    }
}

// ---------------------------------------------------------------------------
// Checks
// ---------------------------------------------------------------------------

pub fn http_command(domain: &DomainName) -> Result<CommandSpec, ValidationError> {
    let host = format!("Host: {}", domain.as_str());
    CommandSpec::new(
        CURL,
        &[
            "-sS",
            "-o",
            "/dev/null",
            "-D",
            "-",
            "--max-time",
            "5",
            "-H",
            &host,
            "http://127.0.0.1/",
        ],
        SHORT,
    )
}

fn check(id: &str, title: &str, status: CheckStatus, detail: impl Into<String>) -> CheckResult {
    CheckResult {
        component: "app".to_owned(),
        id: id.to_owned(),
        title: title.to_owned(),
        essential: true,
        status,
        detail: detail.into(),
    }
}

fn pass_fail(id: &str, title: &str, r: Result<String, String>) -> CheckResult {
    match r {
        Ok(d) => check(id, title, CheckStatus::Pass, d),
        Err(d) => check(id, title, CheckStatus::Fail, d),
    }
}

async fn http_check(exec: &dyn Executor, cancel: &CancelToken, input: &VerifyInput) -> CheckResult {
    const TITLE: &str = "The site answers for this app";
    let cmd = match http_command(&input.domain) {
        Ok(c) => c,
        Err(e) => return check(CHECK_HTTP, TITLE, CheckStatus::Fail, e.to_string()),
    };
    let (code, r) = match run(exec, cancel, &cmd).await {
        Ok(x) => x,
        Err(e) => return check(CHECK_HTTP, TITLE, CheckStatus::Fail, e),
    };
    if code == EXIT_NOT_FOUND {
        return check(
            CHECK_HTTP,
            TITLE,
            CheckStatus::Skipped,
            "curl is not installed on the server; the site was not verified",
        );
    }
    if code != 0 {
        return check(
            CHECK_HTTP,
            TITLE,
            CheckStatus::Fail,
            format!("curl could not get an answer (exit {code})"),
        );
    }
    match parse_http(&r.stdout, &input.slug) {
        HttpOutcome::Pass { status } => check(
            CHECK_HTTP,
            TITLE,
            CheckStatus::Pass,
            format!("HTTP {status} with the app header"),
        ),
        HttpOutcome::BadStatus(s) => check(
            CHECK_HTTP,
            TITLE,
            CheckStatus::Fail,
            format!("HTTP {s}; expected 200 to 399"),
        ),
        HttpOutcome::WrongSite { status } => check(
            CHECK_HTTP,
            TITLE,
            CheckStatus::Fail,
            format!("HTTP {status} without the X-Ossiframe-App header: another site answered"),
        ),
        HttpOutcome::Unparsable => check(
            CHECK_HTTP,
            TITLE,
            CheckStatus::Fail,
            "the answer could not be read",
        ),
    }
}

/// Runs the health checks. Read-only.
pub async fn verify_app(
    exec: &dyn Executor,
    cancel: &CancelToken,
    input: &VerifyInput,
) -> Vec<CheckResult> {
    let mut out = Vec::new();
    let obs = observe_app_state(exec, cancel, &input.slug, input.app_type).await;
    let current = match &obs {
        Ok(o) if o.current == CurrentLink::Release(input.release.clone()) => Ok(format!(
            "current points to release {}",
            input.release.as_str()
        )),
        Ok(_) => Err("current does not point to the release that was deployed".to_owned()),
        Err(e) => Err(e.clone()),
    };
    out.push(pass_fail(
        CHECK_CURRENT,
        "current points to the new release",
        current,
    ));
    out.push(http_check(exec, cancel, input).await);
    if input.app_type.info().uses_php {
        let socket = format!("/run/php/ossiframe-{}.sock", input.slug.as_str());
        let r = match (
            &obs,
            CommandSpec::new(TEST, &["-S", &socket], SHORT).map_err(|e| e.to_string()),
        ) {
            (Ok(o), Ok(cmd)) => match run(exec, cancel, &cmd).await {
                Ok((code, _)) => fpm_ok(o.fpm_active, code)
                    .map(|()| "php8.3-fpm is active and the pool socket exists".to_owned()),
                Err(e) => Err(e),
            },
            (_, Err(e)) => Err(e),
            (Err(e), _) => Err(e.clone()),
        };
        out.push(pass_fail(CHECK_FPM, "PHP-FPM pool is active", r));
    }
    if input.app_type == AppType::Laravel {
        out.push(pass_fail(
            CHECK_APP_KEY,
            "Laravel APP_KEY is set",
            app_key_check(exec, cancel, &input.slug).await,
        ));
    }
    out
}

async fn app_key_check(
    exec: &dyn Executor,
    cancel: &CancelToken,
    slug: &AppSlug,
) -> Result<String, String> {
    let slug = v(app_slug(slug.as_str()))?;
    let user = v(app_user(&slug))?;
    let root = v(AppRoot::new(&slug))?;
    let cmd = v(CommandSpec::run_as(
        user,
        GREP,
        &[
            "-q",
            "-e",
            "^APP_KEY=base64:.",
            "--",
            &format!("{}/shared/.env", root.as_str()),
        ],
        SHORT,
    ))?;
    let (code, _) = run(exec, cancel, &cmd).await?;
    app_key_ok(code).map(|()| "APP_KEY is set".to_owned())
}

// ---------------------------------------------------------------------------
// Recovery (correction 8)
// ---------------------------------------------------------------------------

/// What an app operation was meant to leave behind.
#[derive(Debug, Clone)]
pub struct ExpectedApp {
    pub app_type: AppType,
    pub release: ReleaseId,
    /// The release `current` pointed to before the operation. `None` for a
    /// first deploy, where `current` does not exist yet.
    pub previous: Option<ReleaseId>,
}

/// Pure decision. `verify_passed` is `Some` only when the checks were run.
///
/// - Deployed: `current` is the expected release, its directory exists, the
///   site (and pool) carry the marker, the services are active and the
///   checks pass.
/// - NotSwitched: `current` is still the previous release (or still absent
///   for a first deploy).
/// - Unknown: everything else, with what was observed.
pub fn reconcile_app(
    obs: &AppObservation,
    exp: &ExpectedApp,
    verify_passed: Option<bool>,
) -> AppRecovery {
    let rec = |verdict, detail: &str| AppRecovery {
        verdict,
        detail: detail.to_owned(),
    };
    if obs.current == CurrentLink::Release(exp.release.clone()) {
        let uses_php = exp.app_type.info().uses_php;
        let mut problems: Vec<&str> = Vec::new();
        if !obs.releases.contains(&exp.release) {
            problems.push("the release directory is missing");
        }
        if obs.site != FileState::Managed {
            problems.push("the NGINX site is missing or altered");
        }
        if uses_php && obs.pool != Some(FileState::Managed) {
            problems.push("the PHP-FPM pool is missing or altered");
        }
        if obs.nginx_active != Some(true) {
            problems.push("nginx is not active");
        }
        if uses_php && obs.fpm_active != Some(true) {
            problems.push("php8.3-fpm is not active");
        }
        match verify_passed {
            Some(true) => {}
            Some(false) => problems.push("the health checks fail"),
            None => problems.push("the health checks were not run"),
        }
        return if problems.is_empty() {
            rec(
                AppRecoveryVerdict::Deployed,
                "current is the expected release and the checks pass",
            )
        } else {
            rec(
                AppRecoveryVerdict::Unknown,
                &format!(
                    "current is the expected release, but {}",
                    problems.join(", ")
                ),
            )
        };
    }
    let unswitched = match (&obs.current, &exp.previous) {
        (CurrentLink::Release(c), Some(p)) => c == p,
        (CurrentLink::Absent, None) => true,
        _ => false,
    };
    if unswitched {
        return rec(
            AppRecoveryVerdict::NotSwitched,
            "current is still the previous release",
        );
    }
    rec(
        AppRecoveryVerdict::Unknown,
        "current is neither the expected nor the previous release",
    )
}

/// Observes the app, runs the checks only when `current` is the expected
/// release, and decides. Read-only.
pub async fn recover_app(
    exec: &dyn Executor,
    cancel: &CancelToken,
    input: &VerifyInput,
    previous: Option<ReleaseId>,
) -> AppRecovery {
    let obs = match observe_app_state(exec, cancel, &input.slug, input.app_type).await {
        Ok(o) => o,
        Err(e) => {
            return AppRecovery {
                verdict: AppRecoveryVerdict::Unknown,
                detail: format!("the application could not be observed: {e}"),
            };
        }
    };
    let exp = ExpectedApp {
        app_type: input.app_type,
        release: input.release.clone(),
        previous,
    };
    let passed = if obs.current == CurrentLink::Release(input.release.clone()) {
        let checks = verify_app(exec, cancel, input).await;
        Some(
            checks
                .iter()
                .filter(|c| c.essential)
                .all(|c| c.status == CheckStatus::Pass),
        )
    } else {
        None
    };
    reconcile_app(&obs, &exp, passed)
}

#[cfg(test)]
mod tests;
