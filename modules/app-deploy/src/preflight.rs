//! Preflight for an application: read-only fact collection and a pure
//! evaluation into [`Conflict`]s. Every probe is a structured command with an
//! absolute program path that runs as the login user (no `sudo`). Missing
//! evidence is never read as "no conflict": a probe that failed, timed out,
//! was cut at the output cap or could not be parsed produces a Blocker.
use crate::app_type::{AppType, DatabaseUse};
use crate::identity::app_user;
use crate::paths::NGINX_SITES_ENABLED;
use ossiframe_core::{
    AppSlug, CancelToken, CommandSpec, Conflict, DomainName, ExecError, ExecResult, Executor,
    ExitStatus, GitUrl, Hostname, LinuxUserName, OperationError, Severity, ValidationError,
};
use std::collections::BTreeMap;
use std::time::Duration;

const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
const SCAN_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_SITE_FILES: usize = 500;
const MAX_NOTE: usize = 200;
/// Free space on `/srv` below which a new app is blocked.
pub const MIN_FREE_DISK_MB: u64 = 1024;
const SITES_ENABLED_DIR: &str = "/etc/nginx/sites-enabled";
/// Git hosts whose keys ship with Ossiframe (`data/known_hosts`); no scan is
/// needed for them on port 22.
pub const BUILTIN_GIT_HOSTS: [&str; 4] = [
    "github.com",
    "gitlab.com",
    "bitbucket.org",
    "ssh.bitbucket.org",
];
/// Groups an app user must not belong to.
const FORBIDDEN_GROUPS: [&str; 5] = ["sudo", "admin", "root", "wheel", "adm"];
const NOLOGIN_SHELLS: [&str; 2] = ["/usr/sbin/nologin", "/bin/false"];

/// A fact, or the reason it could not be established.
pub type Evidence<T> = Result<T, String>;

// ---------------------------------------------------------------------------
// Running commands
// ---------------------------------------------------------------------------

fn clean(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .take(MAX_NOTE)
        .collect()
}

fn user(program: &str, args: &[&str]) -> Result<CommandSpec, ValidationError> {
    CommandSpec::new(program, args, PROBE_TIMEOUT)
}

/// Cancel and lost connection abort; every other failure is text.
async fn ran(
    exec: &dyn Executor,
    cancel: &CancelToken,
    spec: Result<CommandSpec, ValidationError>,
) -> Result<Evidence<ExecResult>, OperationError> {
    let spec = match spec {
        Ok(s) => s,
        Err(e) => return Ok(Err(e.to_string())),
    };
    match exec.run(&spec, cancel).await {
        Ok(r) if r.truncated => Ok(Err("output was cut at the size limit".to_owned())),
        Ok(r) => Ok(Ok(r)),
        Err(ExecError::Timeout) => Ok(Err(
            "timed out; the command may still be running on the server".to_owned(),
        )),
        Err(ExecError::Channel(m)) => Ok(Err(clean(&m))),
        Err(ExecError::PrivilegeDenied) => Ok(Err("privileged command was not allowed".to_owned())),
        Err(e @ (ExecError::Cancelled | ExecError::ConnectionLost)) => Err(OperationError::Exec(e)),
    }
}

fn code_of(r: &ExecResult) -> Option<i32> {
    match r.exit {
        ExitStatus::Code(c) => Some(c),
        _ => None,
    }
}

fn first_stderr(r: &ExecResult) -> String {
    clean(r.stderr.lines().next().unwrap_or(""))
}

/// Complete stdout when the exit code is one of `ok_codes`.
async fn stdout_of(
    exec: &dyn Executor,
    cancel: &CancelToken,
    spec: Result<CommandSpec, ValidationError>,
    ok_codes: &[i32],
) -> Result<Evidence<String>, OperationError> {
    Ok(ran(exec, cancel, spec)
        .await?
        .and_then(|r| match code_of(&r) {
            Some(c) if ok_codes.contains(&c) => Ok(r.stdout),
            _ => Err(format!("exit {:?}: {}", r.exit, first_stderr(&r))),
        }))
}

// ---------------------------------------------------------------------------
// Parsers
// ---------------------------------------------------------------------------

/// Parses `dpkg-query -W -f '${Package}\t${db:Status-Abbrev}\n'`. Requested
/// packages missing from the output are absent; only status `ii` is present.
pub fn parse_installed(stdout: &str, requested: &[&str]) -> BTreeMap<String, bool> {
    let mut map: BTreeMap<String, bool> =
        requested.iter().map(|p| ((*p).to_owned(), false)).collect();
    for line in stdout.lines() {
        if let Some((name, status)) = line.split_once('\t')
            && let Some(slot) = map.get_mut(name)
        {
            *slot = status.trim() == "ii";
        }
    }
    map
}

/// Available KiB from `df -Pk` (last data line, column 4).
pub fn parse_df_available_mb(stdout: &str) -> Result<u64, String> {
    let line = stdout
        .lines()
        .rfind(|l| !l.trim().is_empty())
        .ok_or("empty df output")?;
    let cols: Vec<&str> = line.split_whitespace().collect();
    if cols.len() < 6 {
        return Err("unrecognized df output".to_owned());
    }
    let kb: u64 = cols[cols.len() - 3]
        .parse()
        .map_err(|_| "unrecognized df output".to_owned())?;
    Ok(kb / 1024)
}

/// File names, one per line, from `ls -1`.
pub fn parse_names(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

/// File base names from `grep -rl` output (full paths, one per line).
pub fn parse_grep_files(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| l.rsplit('/').next().unwrap_or(l).to_owned())
        .collect()
}

/// One line of `getent passwd`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswdEntry {
    pub name: String,
    pub uid: u32,
    pub shell: String,
}

pub fn parse_passwd(line: &str) -> Result<PasswdEntry, String> {
    let f: Vec<&str> = line.trim_end_matches('\n').split(':').collect();
    if f.len() != 7 {
        return Err("unrecognized passwd entry".to_owned());
    }
    let uid = f[2]
        .parse::<u32>()
        .map_err(|_| "unrecognized uid in passwd entry".to_owned())?;
    Ok(PasswdEntry {
        name: f[0].to_owned(),
        uid,
        shell: f[6].to_owned(),
    })
}

/// A host key from `ssh-keyscan`, with its SHA256 fingerprint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedKey {
    /// For example `ssh-ed25519`.
    pub key_type: String,
    /// `SHA256:...` as printed by `ssh-keygen -l`.
    pub fingerprint: String,
    /// `<type> <base64>`, ready for a `known_hosts` line body.
    pub key: String,
}

/// Key lines (`<type> <base64>`) from `ssh-keyscan` output. Comment lines are
/// skipped; any other malformed line is an error.
pub fn parse_keyscan_keys(stdout: &str) -> Result<Vec<(String, String)>, String> {
    let mut keys = Vec::new();
    for line in stdout.lines().filter(|l| !l.trim().is_empty()) {
        if line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        let [_host, ty, b64] = f[..] else {
            return Err("unrecognized ssh-keyscan line".to_owned());
        };
        let ok = ty.starts_with("ssh-") || ty.starts_with("ecdsa-");
        if !ok
            || !b64
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
        {
            return Err("unrecognized ssh-keyscan line".to_owned());
        }
        keys.push((ty.to_owned(), b64.to_owned()));
    }
    Ok(keys)
}

/// Fingerprints from `ssh-keygen -lf` output, one per key line.
pub fn parse_keygen_fingerprints(stdout: &str) -> Result<Vec<String>, String> {
    stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            l.split_whitespace()
                .nth(1)
                .filter(|fp| fp.starts_with("SHA256:") && fp.len() > 7)
                .map(str::to_owned)
                .ok_or_else(|| "unrecognized ssh-keygen output".to_owned())
        })
        .collect()
}

/// Pairs scanned keys with fingerprints. Counts must match.
pub fn pair_keys(
    keys: Vec<(String, String)>,
    fingerprints: Vec<String>,
) -> Result<Vec<ScannedKey>, String> {
    if keys.is_empty() {
        return Err("the host presented no keys".to_owned());
    }
    if keys.len() != fingerprints.len() {
        return Err("fingerprint count does not match key count".to_owned());
    }
    Ok(keys
        .into_iter()
        .zip(fingerprints)
        .map(|((key_type, b64), fingerprint)| ScannedKey {
            key: format!("{key_type} {b64}"),
            key_type,
            fingerprint,
        })
        .collect())
}

/// Host and port of an SSH Git URL. `None` for HTTPS.
pub fn ssh_endpoint(url: &GitUrl) -> Option<(Hostname, u16)> {
    let s = url.as_str();
    let (authority, default) = if let Some(rest) = s.strip_prefix("ssh://git@") {
        (rest.split('/').next()?, true)
    } else {
        (s.strip_prefix("git@")?.split(':').next()?, false)
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if default => (h, p.parse::<u16>().ok()?),
        _ => (authority, 22),
    };
    Some((Hostname::parse(host).ok()?, port))
}

fn is_builtin_host(host: &Hostname) -> bool {
    BUILTIN_GIT_HOSTS.contains(&host.as_str().to_ascii_lowercase().as_str())
}

// ---------------------------------------------------------------------------
// App user verification (binding correction 4)
// ---------------------------------------------------------------------------

/// What the caller knows about the app's user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppUserExpectation {
    /// A new app: the user must not exist yet.
    New,
    /// An existing app: the user recorded for it must exist and be safe.
    Existing { recorded_user: LinuxUserName },
}

/// Observed state of the app user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppUserFacts {
    /// `None`: the user does not exist.
    pub passwd: Evidence<Option<PasswdEntry>>,
    pub groups: Evidence<Vec<String>>,
}

/// Pure evaluation. An empty result means the user passes.
pub fn evaluate_app_user(
    facts: &AppUserFacts,
    expected_name: &str,
    expectation: &AppUserExpectation,
) -> Vec<Conflict> {
    let block = |id: &str, m: String| Conflict {
        id: id.to_owned(),
        severity: Severity::Blocker,
        message: m,
    };
    let entry = match &facts.passwd {
        Err(e) => {
            return vec![block(
                "app-user-evidence",
                format!("User {expected_name} could not be checked: {e}"),
            )];
        }
        Ok(e) => e,
    };
    match (expectation, entry) {
        (AppUserExpectation::New, None) => return Vec::new(),
        (AppUserExpectation::New, Some(_)) => {
            return vec![block(
                "app-user-exists",
                format!(
                    "User {expected_name} already exists and does not belong to a known Ossiframe app."
                ),
            )];
        }
        (AppUserExpectation::Existing { .. }, None) => {
            return vec![block(
                "app-user-missing",
                format!("User {expected_name} does not exist."),
            )];
        }
        _ => {}
    }
    let (AppUserExpectation::Existing { recorded_user }, Some(e)) = (expectation, entry) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if recorded_user.as_str() != expected_name || e.name != expected_name {
        out.push(block(
            "app-user-mismatch",
            format!("User {expected_name} is not the user recorded for this app."),
        ));
    }
    if e.uid < 1000 || e.uid == 0 {
        out.push(block(
            "app-user-uid",
            format!(
                "User {expected_name} has uid {}; a system uid is not accepted.",
                e.uid
            ),
        ));
    }
    if !NOLOGIN_SHELLS.contains(&e.shell.as_str()) {
        out.push(block(
            "app-user-shell",
            format!(
                "User {expected_name} has shell {}; it must be /usr/sbin/nologin or /bin/false.",
                clean(&e.shell)
            ),
        ));
    }
    match &facts.groups {
        Err(err) => out.push(block(
            "app-user-evidence",
            format!("Groups of {expected_name} could not be checked: {err}"),
        )),
        Ok(groups) => {
            let bad: Vec<&str> = groups
                .iter()
                .map(String::as_str)
                .filter(|g| FORBIDDEN_GROUPS.contains(g))
                .collect();
            if !bad.is_empty() {
                out.push(block(
                    "app-user-groups",
                    format!(
                        "User {expected_name} belongs to privileged group(s): {}.",
                        bad.join(", ")
                    ),
                ));
            }
        }
    }
    out
}

/// Reads the user facts: `getent passwd <user>` and `id -Gn <user>`.
pub async fn collect_app_user(
    exec: &dyn Executor,
    cancel: &CancelToken,
    name: &LinuxUserName,
) -> Result<AppUserFacts, OperationError> {
    // getent exits 2 when the key is not found.
    let passwd = stdout_of(
        exec,
        cancel,
        user("/usr/bin/getent", &["passwd", "--", name.as_str()]),
        &[0, 2],
    )
    .await?
    .and_then(|s| {
        if s.trim().is_empty() {
            Ok(None)
        } else {
            parse_passwd(&s).map(Some)
        }
    });
    let groups = if matches!(passwd, Ok(Some(_))) {
        stdout_of(
            exec,
            cancel,
            user("/usr/bin/id", &["-Gn", "--", name.as_str()]),
            &[0],
        )
        .await?
        .map(|s| s.split_whitespace().map(str::to_owned).collect())
    } else {
        Ok(Vec::new())
    };
    Ok(AppUserFacts { passwd, groups })
}

/// Read-only identity check used by preflight (existing apps) and recheck.
/// An empty result means the app user is safe to use for `RunAs` steps.
pub async fn verify_app_user(
    exec: &dyn Executor,
    cancel: &CancelToken,
    slug: &AppSlug,
    expectation: &AppUserExpectation,
) -> Result<Vec<Conflict>, OperationError> {
    let name = match app_user(slug) {
        Ok(n) => n,
        Err(e) => {
            return Ok(vec![Conflict {
                id: "app-user-name".to_owned(),
                severity: Severity::Blocker,
                message: e.to_string(),
            }]);
        }
    };
    let facts = collect_app_user(exec, cancel, &name).await?;
    Ok(evaluate_app_user(&facts, name.as_str(), expectation))
}

// ---------------------------------------------------------------------------
// Facts and evaluation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreflightInput {
    pub app_type: AppType,
    /// Create the app's MySQL database (Laravel default, Php opt-in).
    pub with_database: bool,
    pub slug: AppSlug,
    pub domain: DomainName,
    pub repository: GitUrl,
    pub user: AppUserExpectation,
    /// The file name under `sites-enabled` that belongs to this app, so a
    /// re-plan does not collide with itself. `None` for a new app.
    pub own_site_file: Option<String>,
}

/// Everything observed on the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    /// Package name to installed.
    pub packages: Evidence<BTreeMap<String, bool>>,
    pub free_disk_mb: Evidence<u64>,
    /// Names in `sites-enabled`.
    pub sites: Evidence<Vec<String>>,
    /// File names under `sites-enabled` that mention the domain.
    pub domain_files: Evidence<Vec<String>>,
    pub app_user: AppUserFacts,
    /// `curl` exists, needed by the HTTP health check. Fact only.
    pub curl_present: Evidence<bool>,
    /// Scanned host keys; `None` when no scan was needed.
    pub host_keys: Option<Evidence<Vec<ScannedKey>>>,
    /// Host the keys belong to.
    pub ssh_host: Option<Hostname>,
}

/// The preflight result handed to plan building.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreflightReport {
    pub conflicts: Vec<Conflict>,
    pub free_disk_mb: Option<u64>,
    pub curl_present: Option<bool>,
    /// SSH host and fingerprints for a non-builtin host.
    pub ssh_host: Option<Hostname>,
    pub host_keys: Vec<ScannedKey>,
}

impl PreflightReport {
    pub fn can_proceed(&self) -> bool {
        !self
            .conflicts
            .iter()
            .any(|c| c.severity == Severity::Blocker)
    }
}

/// Packages the app needs, as (package, label).
pub fn required_packages(app_type: AppType, with_database: bool) -> Vec<&'static str> {
    let info = app_type.info();
    let mut v = vec!["git", "nginx"];
    if info.uses_php {
        v.push("php8.3-fpm");
        v.push("composer");
    }
    let db = match info.database {
        DatabaseUse::Default => true,
        DatabaseUse::OptIn => with_database,
        DatabaseUse::Never => false,
    };
    if db {
        v.push("mysql-server");
    }
    v
}

pub async fn collect_facts(
    exec: &dyn Executor,
    cancel: &CancelToken,
    input: &PreflightInput,
) -> Result<Facts, OperationError> {
    let required = required_packages(input.app_type, input.with_database);
    let mut args = vec!["-W", "-f", "${Package}\t${db:Status-Abbrev}\n", "--"];
    args.extend(required.iter().copied());
    // Exit 1 when a package is unknown to dpkg; the rest is still listed.
    let packages = stdout_of(exec, cancel, user("/usr/bin/dpkg-query", &args), &[0, 1])
        .await?
        .map(|s| parse_installed(&s, &required));

    let free_disk_mb = stdout_of(
        exec,
        cancel,
        user("/usr/bin/df", &["-Pk", "--", "/srv"]),
        &[0],
    )
    .await?
    .and_then(|s| parse_df_available_mb(&s));

    let sites = stdout_of(
        exec,
        cancel,
        user("/usr/bin/ls", &["-1", "--", SITES_ENABLED_DIR]),
        &[0],
    )
    .await?
    .and_then(|s| {
        let v = parse_names(&s);
        if v.len() > MAX_SITE_FILES {
            Err("too many site files to check".to_owned())
        } else {
            Ok(v)
        }
    });

    // Exit 0 match, 1 no match, 2 error (also set by unreadable files).
    let domain_files = stdout_of(
        exec,
        cancel,
        user(
            "/usr/bin/grep",
            &["-Rlsw", "--", input.domain.as_str(), SITES_ENABLED_DIR],
        ),
        &[0, 1],
    )
    .await?
    .map(|s| parse_grep_files(&s));

    let name = app_user(&input.slug);
    let app_user_facts = match &name {
        Ok(n) => collect_app_user(exec, cancel, n).await?,
        Err(e) => AppUserFacts {
            passwd: Err(e.to_string()),
            groups: Err(e.to_string()),
        },
    };

    let curl_present = ran(
        exec,
        cancel,
        user("/usr/bin/test", &["-x", "/usr/bin/curl"]),
    )
    .await?
    .and_then(|r| match code_of(&r) {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(format!("exit {:?}: {}", r.exit, first_stderr(&r))),
    });

    let (ssh_host, host_keys) = match ssh_endpoint(&input.repository) {
        // The shipped keys are for port 22 only.
        Some((host, port)) if !(is_builtin_host(&host) && port == 22) => {
            let keys = scan_host_keys(exec, cancel, &host, port).await?;
            (Some(host), Some(keys))
        }
        _ => (None, None),
    };

    Ok(Facts {
        packages,
        free_disk_mb,
        sites,
        domain_files,
        app_user: app_user_facts,
        curl_present,
        host_keys,
        ssh_host,
    })
}

/// `ssh-keyscan` as the login user, then fingerprints from
/// `ssh-keygen -lf /dev/stdin` with the scan output on stdin.
pub async fn scan_host_keys(
    exec: &dyn Executor,
    cancel: &CancelToken,
    host: &Hostname,
    port: u16,
) -> Result<Evidence<Vec<ScannedKey>>, OperationError> {
    let port_s = port.to_string();
    let scan = CommandSpec::new(
        "/usr/bin/ssh-keyscan",
        &["-T", "10", "-p", &port_s, "--", host.as_str()],
        SCAN_TIMEOUT,
    );
    let out = match stdout_of(exec, cancel, scan, &[0]).await? {
        Ok(o) => o,
        Err(e) => return Ok(Err(e)),
    };
    let keys = match parse_keyscan_keys(&out) {
        Ok(k) if !k.is_empty() => k,
        Ok(_) => return Ok(Err("the host presented no keys".to_owned())),
        Err(e) => return Ok(Err(e)),
    };
    let lines: String = keys
        .iter()
        .map(|(t, b)| format!("{t} {b}\n"))
        .collect::<String>();
    let keygen = user("/usr/bin/ssh-keygen", &["-lf", "/dev/stdin"])
        .and_then(|c| c.with_stdin(lines.into_bytes()));
    let fp = match stdout_of(exec, cancel, keygen, &[0]).await? {
        Ok(o) => o,
        Err(e) => return Ok(Err(e)),
    };
    Ok(parse_keygen_fingerprints(&fp).and_then(|f| pair_keys(keys, f)))
}

fn blocker(id: impl Into<String>, message: impl Into<String>) -> Conflict {
    Conflict {
        id: id.into(),
        severity: Severity::Blocker,
        message: message.into(),
    }
}

/// Pure evaluation of the facts.
pub fn evaluate(input: &PreflightInput, facts: &Facts) -> PreflightReport {
    let mut c: Vec<Conflict> = Vec::new();

    // Base stack.
    let required = required_packages(input.app_type, input.with_database);
    match &facts.packages {
        Err(e) => c.push(blocker(
            "packages-evidence",
            format!("Installed packages could not be checked: {e}"),
        )),
        Ok(map) => {
            for p in required {
                if !map.get(p).copied().unwrap_or(false) {
                    c.push(blocker(
                        format!("missing-{p}"),
                        format!("{p} is not installed. Provision the base stack first."),
                    ));
                }
            }
        }
    }

    // Disk.
    match &facts.free_disk_mb {
        Err(e) => c.push(blocker(
            "disk-unavailable",
            format!("Free disk space unknown: {e}"),
        )),
        Ok(mb) if *mb < MIN_FREE_DISK_MB => c.push(blocker(
            "disk-low",
            format!("Free disk is {mb} MB; at least {MIN_FREE_DISK_MB} MB is needed."),
        )),
        Ok(_) => {}
    }

    // Domain collision.
    let domain = input.domain.as_str();
    let own = input.own_site_file.as_deref();
    match &facts.sites {
        Err(e) => c.push(blocker(
            "sites-evidence",
            format!("NGINX sites could not be listed: {e}"),
        )),
        Ok(sites) => {
            let prefix = NGINX_SITES_ENABLED
                .rsplit('/')
                .next()
                .unwrap_or("ossiframe-");
            for s in sites
                .iter()
                .filter(|s| s.starts_with(prefix) && Some(s.as_str()) != own)
            {
                if s == &format!("{prefix}{domain}") {
                    c.push(blocker(
                        "domain-in-use",
                        format!("{domain} is already used by another Ossiframe site ({s})."),
                    ));
                }
            }
        }
    }
    match &facts.domain_files {
        Err(e) => c.push(blocker(
            "domain-evidence",
            format!("Existing NGINX sites could not be checked for {domain}: {e}"),
        )),
        Ok(files) => {
            for f in files.iter().filter(|f| Some(f.as_str()) != own) {
                if f.starts_with("ossiframe-") {
                    c.push(blocker(
                        "domain-in-use",
                        format!("{domain} is already used by another Ossiframe site ({f})."),
                    ));
                } else {
                    c.push(blocker(
                        "domain-unmanaged",
                        format!("Domain configured outside Ossiframe: {domain} appears in {f}."),
                    ));
                }
            }
        }
    }

    // App user.
    match app_user(&input.slug) {
        Ok(n) => c.extend(evaluate_app_user(&facts.app_user, n.as_str(), &input.user)),
        Err(e) => c.push(blocker("app-user-name", e.to_string())),
    }

    // SSH host keys.
    if let Some(Err(e)) = &facts.host_keys {
        c.push(blocker(
            "git-host-keys",
            format!("Host keys of the Git server could not be scanned: {e}"),
        ));
    }

    let mut seen = std::collections::BTreeSet::new();
    c.retain(|x| seen.insert((x.id.clone(), x.message.clone())));

    PreflightReport {
        conflicts: c,
        free_disk_mb: facts.free_disk_mb.clone().ok(),
        curl_present: facts.curl_present.clone().ok(),
        ssh_host: facts.ssh_host.clone(),
        host_keys: facts
            .host_keys
            .as_ref()
            .and_then(|k| k.clone().ok())
            .unwrap_or_default(),
    }
}

/// Collects facts and evaluates them. Cancel and a lost connection return an
/// error and no report.
pub async fn run(
    exec: &dyn Executor,
    cancel: &CancelToken,
    input: &PreflightInput,
) -> Result<PreflightReport, OperationError> {
    let facts = collect_facts(exec, cancel, input).await?;
    Ok(evaluate(input, &facts))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(t: AppType) -> PreflightInput {
        PreflightInput {
            app_type: t,
            with_database: false,
            slug: AppSlug::parse("blog").unwrap(),
            domain: DomainName::parse("blog.example.com").unwrap(),
            repository: GitUrl::parse("https://github.com/o/r").unwrap(),
            user: AppUserExpectation::New,
            own_site_file: None,
        }
    }

    fn pkgs(missing: &[&str]) -> Evidence<BTreeMap<String, bool>> {
        Ok(["git", "nginx", "php8.3-fpm", "composer", "mysql-server"]
            .iter()
            .map(|p| ((*p).to_owned(), !missing.contains(p)))
            .collect())
    }

    fn good_facts() -> Facts {
        Facts {
            packages: pkgs(&[]),
            free_disk_mb: Ok(20_000),
            sites: Ok(vec!["default".to_owned()]),
            domain_files: Ok(Vec::new()),
            app_user: AppUserFacts {
                passwd: Ok(None),
                groups: Ok(Vec::new()),
            },
            curl_present: Ok(true),
            host_keys: None,
            ssh_host: None,
        }
    }

    fn ids(r: &PreflightReport) -> Vec<&str> {
        r.conflicts.iter().map(|c| c.id.as_str()).collect()
    }

    #[test]
    fn clean_server_passes() {
        let r = evaluate(&input(AppType::Laravel), &good_facts());
        assert!(r.can_proceed(), "{:?}", r.conflicts);
        assert_eq!(r.curl_present, Some(true));
    }

    #[test]
    fn missing_component_blocks_with_its_name() {
        let mut f = good_facts();
        f.packages = pkgs(&["php8.3-fpm"]);
        let r = evaluate(&input(AppType::Php), &f);
        assert_eq!(ids(&r), ["missing-php8.3-fpm"]);
        assert!(r.conflicts[0].message.contains("php8.3-fpm"));
        // Static does not need PHP.
        assert!(evaluate(&input(AppType::Static), &f).can_proceed());
        // MySQL only when a database is wanted.
        f.packages = pkgs(&["mysql-server"]);
        assert!(evaluate(&input(AppType::Php), &f).can_proceed());
        assert_eq!(
            ids(&evaluate(&input(AppType::Laravel), &f)),
            ["missing-mysql-server"]
        );
        f.packages = Err("exit 2".to_owned());
        assert!(!evaluate(&input(AppType::Static), &f).can_proceed());
    }

    #[test]
    fn dpkg_output_parses() {
        let m = parse_installed("git\tii \nnginx\trc \n", &["git", "nginx", "composer"]);
        assert!(m["git"]);
        assert!(!m["nginx"]);
        assert!(!m["composer"]);
    }

    #[test]
    fn low_or_unknown_disk_blocks() {
        let mut f = good_facts();
        f.free_disk_mb = Ok(100);
        assert_eq!(ids(&evaluate(&input(AppType::Static), &f)), ["disk-low"]);
        f.free_disk_mb = Err("x".to_owned());
        assert!(!evaluate(&input(AppType::Static), &f).can_proceed());
        let df = "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/sda1 100000 50000 2097152 3% /\n";
        assert_eq!(parse_df_available_mb(df).unwrap(), 2048);
    }

    #[test]
    fn domain_collision_from_listing() {
        let mut f = good_facts();
        f.sites = Ok(parse_names("default\nossiframe-blog.example.com\n"));
        assert_eq!(
            ids(&evaluate(&input(AppType::Static), &f)),
            ["domain-in-use"]
        );
        // The app's own site does not collide with itself.
        let mut i = input(AppType::Static);
        i.own_site_file = Some("ossiframe-blog.example.com".to_owned());
        assert!(evaluate(&i, &f).can_proceed());
        // Evidence missing blocks.
        f.sites = Err("denied".to_owned());
        assert!(!evaluate(&input(AppType::Static), &f).can_proceed());
    }

    #[test]
    fn unmanaged_domain_match_blocks() {
        let mut f = good_facts();
        f.domain_files = Ok(parse_grep_files("/etc/nginx/sites-enabled/legacy.conf\n"));
        let r = evaluate(&input(AppType::Static), &f);
        assert_eq!(ids(&r), ["domain-unmanaged"]);
        assert!(r.conflicts[0].message.contains("outside Ossiframe"));
        f.domain_files = Err("exit 2".to_owned());
        assert!(!evaluate(&input(AppType::Static), &f).can_proceed());
    }

    fn passwd(name: &str, uid: u32, shell: &str) -> Evidence<Option<PasswdEntry>> {
        Ok(Some(PasswdEntry {
            name: name.to_owned(),
            uid,
            shell: shell.to_owned(),
        }))
    }

    #[test]
    fn existing_user_for_new_app_blocks() {
        let mut f = good_facts();
        f.app_user.passwd = passwd("ossi-blog", 1001, "/usr/sbin/nologin");
        assert_eq!(
            ids(&evaluate(&input(AppType::Static), &f)),
            ["app-user-exists"]
        );
        f.app_user.passwd = Err("timed out".to_owned());
        assert!(!evaluate(&input(AppType::Static), &f).can_proceed());
    }

    #[test]
    fn identity_table() {
        let rec = |n: &str| AppUserExpectation::Existing {
            recorded_user: LinuxUserName::parse(n).unwrap(),
        };
        let facts = |uid, shell: &str, groups: &[&str]| AppUserFacts {
            passwd: passwd("ossi-blog", uid, shell),
            groups: Ok(groups.iter().map(|g| (*g).to_owned()).collect()),
        };
        let nologin = "/usr/sbin/nologin";
        let good = facts(1001, nologin, &["ossi-blog"]);
        assert!(evaluate_app_user(&good, "ossi-blog", &rec("ossi-blog")).is_empty());
        assert!(
            evaluate_app_user(
                &facts(1001, "/bin/false", &[]),
                "ossi-blog",
                &rec("ossi-blog")
            )
            .is_empty()
        );
        let cases = [
            (facts(0, nologin, &[]), "ossi-blog", "app-user-uid"),
            (facts(999, nologin, &[]), "ossi-blog", "app-user-uid"),
            (
                facts(1001, nologin, &["ossi-blog", "sudo"]),
                "ossi-blog",
                "app-user-groups",
            ),
            (facts(1001, "/bin/bash", &[]), "ossi-blog", "app-user-shell"),
            (facts(1001, nologin, &[]), "ossi-other", "app-user-mismatch"),
        ];
        for (f, recorded, id) in cases {
            let c = evaluate_app_user(&f, "ossi-blog", &rec(recorded));
            assert!(c.iter().any(|x| x.id == id), "{id}: {c:?}");
            assert!(c.iter().all(|x| x.severity == Severity::Blocker));
        }
        // Missing user and groups evidence block.
        let none = AppUserFacts {
            passwd: Ok(None),
            groups: Ok(Vec::new()),
        };
        assert!(!evaluate_app_user(&none, "ossi-blog", &rec("ossi-blog")).is_empty());
        let mut g = good.clone();
        g.groups = Err("x".to_owned());
        assert!(!evaluate_app_user(&g, "ossi-blog", &rec("ossi-blog")).is_empty());
        assert_eq!(
            parse_passwd("ossi-blog:x:1001:1001::/srv/ossiframe/apps/blog:/usr/sbin/nologin\n")
                .unwrap()
                .uid,
            1001
        );
        assert!(parse_passwd("garbage").is_err());
    }

    #[test]
    fn keyscan_output_becomes_fingerprints() {
        let scan = "# git.example.com:22 SSH-2.0-OpenSSH_9.6\n\
git.example.com ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIabc\n\
git.example.com ecdsa-sha2-nistp256 AAAAE2VjZHNhAAA=\n";
        let keys = parse_keyscan_keys(scan).unwrap();
        assert_eq!(keys.len(), 2);
        let keygen = "256 SHA256:AAAA1111 git.example.com (ED25519)\n256 SHA256:BBBB2222 git.example.com (ECDSA)\n";
        let paired = pair_keys(keys.clone(), parse_keygen_fingerprints(keygen).unwrap()).unwrap();
        assert_eq!(paired[0].fingerprint, "SHA256:AAAA1111");
        assert_eq!(paired[0].key_type, "ssh-ed25519");
        assert_eq!(paired[1].key, "ecdsa-sha2-nistp256 AAAAE2VjZHNhAAA=");
        assert!(pair_keys(keys, vec!["SHA256:x".to_owned()]).is_err());
        assert!(pair_keys(Vec::new(), Vec::new()).is_err());
        assert!(parse_keyscan_keys("one two\n").is_err());
        assert!(parse_keygen_fingerprints("256 MD5:aa x\n").is_err());
    }

    #[test]
    fn ssh_endpoint_and_builtin_hosts() {
        let ep = |u: &str| ssh_endpoint(&GitUrl::parse(u).unwrap());
        let (h, p) = ep("git@git.example.com:o/r.git").unwrap();
        assert_eq!((h.as_str(), p), ("git.example.com", 22));
        let (h, p) = ep("ssh://git@git.example.com:2222/o/r").unwrap();
        assert_eq!((h.as_str(), p), ("git.example.com", 2222));
        assert!(ep("https://github.com/o/r").is_none());
        assert!(is_builtin_host(&ep("git@github.com:o/r").unwrap().0));
        let mut f = good_facts();
        f.host_keys = Some(Err("no route".to_owned()));
        assert_eq!(
            ids(&evaluate(&input(AppType::Static), &f)),
            ["git-host-keys"]
        );
    }
}
