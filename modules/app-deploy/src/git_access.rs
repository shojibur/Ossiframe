//! Git access for an app: pinned host keys, a per-app deploy key and the
//! `deploy-key-check` gate. HTTPS URLs use none of it.
//!
//! - `<app_root>/.ssh/known_hosts` (0600) is written as the app user, never
//!   root (binding correction 1), with either the shipped keys of a builtin
//!   host (`data/known_hosts`, copied from the providers' documentation) or
//!   only the scanned keys whose fingerprints the user confirmed. A host that
//!   is neither builtin nor confirmed returns `gitHostKeyUnconfirmed` and
//!   nothing is written.
//! - The deploy key is an ed25519 key generated as the app user. An existing
//!   key is never replaced. Only the `.pub` file is ever read.
//! - Git runs with `StrictHostKeyChecking=yes` and the app's `known_hosts`
//!   only. There is no trust-on-first-use anywhere.
//! - The gate runs `git ls-remote`, which proves both authentication and that
//!   the branch exists. Output is parsed into a fixed [`AccessResult`]; raw
//!   output never leaves this module.
use crate::create::{find, plan_slug, run, run_ok, v};
use crate::identity::{app_slug, app_user};
use crate::paths::{AppPath, AppRoot};
use crate::preflight::{ScannedKey, scan_host_keys, ssh_endpoint};
use crate::write::{self, FileMode};
use ossiframe_core::{
    AppSlug, BranchName, CancelToken, CommandSpec, ExecError, Executor, ExitStatus, FileChange,
    GitUrl, Hostname, OperationError, Plan, PlanStep, StepMode, ValidationError,
};
use serde::Serialize;
use std::time::Duration;

/// Gate: generate the deploy key unless one exists.
pub const STEP_DEPLOY_KEY_GENERATE: &str = "deploy-key-generate";
/// Gate: `git ls-remote` with the deploy key and pinned host keys.
pub const STEP_DEPLOY_KEY_CHECK: &str = "deploy-key-check";
/// Id prefix of the `known_hosts` write (`-install` and `-move` steps).
pub const STEP_KNOWN_HOSTS: &str = "git-known-hosts";

const STEP_TIMEOUT: Duration = Duration::from_secs(60);
const SSH_KEYGEN: &str = "/usr/bin/ssh-keygen";
const GIT: &str = "/usr/bin/git";
const ENV: &str = "/usr/bin/env";
const TEST: &str = "/usr/bin/test";
const CAT: &str = "/usr/bin/cat";
const KEY_FILE: &str = ".ssh/id_ed25519";
const PUB_FILE: &str = ".ssh/id_ed25519.pub";
const KNOWN_HOSTS_FILE: &str = ".ssh/known_hosts";

/// Host keys of the builtin hosts. Each block names its source page and the
/// date it was copied; comment lines start with `#`.
const KNOWN_HOSTS_DATA: &str = include_str!("../data/known_hosts");

/// `<type> <base64>` bodies shipped for `host`, in file order.
pub fn builtin_keys(host: &str) -> Vec<String> {
    KNOWN_HOSTS_DATA
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once(' '))
        .filter(|(h, _)| *h == host)
        .map(|(_, k)| k.to_owned())
        .collect()
}

// ---------------------------------------------------------------------------
// Host key decision
// ---------------------------------------------------------------------------

/// The host keys that will be pinned for an app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeys {
    /// HTTPS URL: no keys, no `known_hosts`.
    None,
    /// Shipped keys of a builtin host (port 22).
    Builtin { host: Hostname, keys: Vec<String> },
    /// Scanned keys whose fingerprints the user confirmed.
    Confirmed {
        host: Hostname,
        port: u16,
        keys: Vec<ScannedKey>,
    },
}

/// The host field of a `known_hosts` line.
fn host_field(host: &Hostname, port: u16) -> String {
    if port == 22 {
        host.as_str().to_owned()
    } else {
        format!("[{}]:{port}", host.as_str())
    }
}

impl HostKeys {
    /// The complete `known_hosts` content, or `None` when nothing is pinned.
    pub fn known_hosts(&self) -> Option<String> {
        let (field, bodies): (String, Vec<&str>) = match self {
            HostKeys::None => return None,
            HostKeys::Builtin { host, keys } => (
                host_field(host, 22),
                keys.iter().map(String::as_str).collect(),
            ),
            HostKeys::Confirmed { host, port, keys } => (
                host_field(host, *port),
                keys.iter().map(|k| k.key.as_str()).collect(),
            ),
        };
        Some(bodies.iter().map(|b| format!("{field} {b}\n")).collect())
    }
}

/// Why git access cannot be planned or is no longer valid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitAccessError {
    /// The host is not builtin and the user has not confirmed any of its
    /// scanned fingerprints. Nothing is written.
    HostKeyUnconfirmed {
        host: Hostname,
        port: u16,
        keys: Vec<ScannedKey>,
    },
    /// The host needs a scan and none was made.
    HostKeysNotScanned { host: Hostname },
    /// The scan at recheck failed.
    ScanFailed(String),
    /// A confirmed key is no longer presented by the host.
    RePlanRequired { host: Hostname },
}

impl GitAccessError {
    /// Stable reason string for the IPC layer.
    pub fn reason(&self) -> &'static str {
        match self {
            GitAccessError::HostKeyUnconfirmed { .. } => "gitHostKeyUnconfirmed",
            GitAccessError::HostKeysNotScanned { .. } | GitAccessError::ScanFailed(_) => {
                "gitHostKeyScanFailed"
            }
            GitAccessError::RePlanRequired { .. } => "rePlanRequired",
        }
    }
}

impl std::fmt::Display for GitAccessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitAccessError::HostKeyUnconfirmed { host, .. } => write!(
                f,
                "The SSH host keys of {} are not confirmed. Compare the fingerprints and confirm them.",
                host.as_str()
            ),
            GitAccessError::HostKeysNotScanned { host } => {
                write!(
                    f,
                    "The SSH host keys of {} were not scanned.",
                    host.as_str()
                )
            }
            GitAccessError::ScanFailed(m) => write!(f, "The host keys could not be scanned: {m}"),
            GitAccessError::RePlanRequired { host } => write!(
                f,
                "The SSH host keys of {} changed since the plan was made. Create a new plan.",
                host.as_str()
            ),
        }
    }
}

impl std::error::Error for GitAccessError {}

/// Decides which host keys to pin. `scanned` is
/// `PreflightReport::host_keys`; `confirmed` are the fingerprints the user
/// accepted.
pub fn plan_host_keys(
    url: &GitUrl,
    scanned: &[ScannedKey],
    confirmed: &[String],
) -> Result<HostKeys, GitAccessError> {
    let Some((host, port)) = ssh_endpoint(url) else {
        return Ok(HostKeys::None);
    };
    if port == 22 {
        let keys = builtin_keys(host.as_str());
        if !keys.is_empty() {
            return Ok(HostKeys::Builtin { host, keys });
        }
    }
    if scanned.is_empty() {
        return Err(GitAccessError::HostKeysNotScanned { host });
    }
    let keys: Vec<ScannedKey> = scanned
        .iter()
        .filter(|k| confirmed.contains(&k.fingerprint))
        .cloned()
        .collect();
    if keys.is_empty() {
        return Err(GitAccessError::HostKeyUnconfirmed {
            host,
            port,
            keys: scanned.to_vec(),
        });
    }
    Ok(HostKeys::Confirmed { host, port, keys })
}

/// Pure part of the recheck: every pinned key must still be presented.
pub fn compare_rescan(planned: &HostKeys, rescan: &[ScannedKey]) -> Result<(), GitAccessError> {
    if let HostKeys::Confirmed { host, keys, .. } = planned
        && keys
            .iter()
            .any(|k| !rescan.iter().any(|r| r.fingerprint == k.fingerprint))
    {
        return Err(GitAccessError::RePlanRequired { host: host.clone() });
    }
    Ok(())
}

/// Rescans a confirmed host and returns `rePlanRequired` when a pinned key
/// changed. Builtin hosts and HTTPS need no rescan.
pub async fn recheck_host_keys(
    exec: &dyn Executor,
    cancel: &CancelToken,
    planned: &HostKeys,
) -> Result<Result<(), GitAccessError>, OperationError> {
    let HostKeys::Confirmed { host, port, .. } = planned else {
        return Ok(Ok(()));
    };
    Ok(match scan_host_keys(exec, cancel, host, *port).await? {
        Ok(rescan) => compare_rescan(planned, &rescan),
        Err(e) => Err(GitAccessError::ScanFailed(e)),
    })
}

// ---------------------------------------------------------------------------
// Paths, commands and plan steps
// ---------------------------------------------------------------------------

/// The `GIT_SSH_COMMAND` value. Git runs it through `sh`, so every word must
/// be safe unquoted: it is built only from fixed options and the app root,
/// which comes from a validated slug (`a-z 0-9 -`). Nothing from the URL, the
/// branch or the user is interpolated. `GlobalKnownHostsFile=/dev/null` keeps
/// the app's own `known_hosts` the only source of trust.
pub fn git_ssh_command(root: &AppRoot) -> String {
    let r = root.as_str();
    format!(
        "/usr/bin/ssh -i {r}/{KEY_FILE} -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes \
         -o UserKnownHostsFile={r}/{KNOWN_HOSTS_FILE} -o GlobalKnownHostsFile=/dev/null \
         -o BatchMode=yes"
    )
}

fn step(id: &str, title: String, command: CommandSpec, mode: StepMode) -> PlanStep {
    PlanStep {
        id: id.to_owned(),
        title,
        command,
        mode,
        deadline: STEP_TIMEOUT,
        requires_stdin: false,
    }
}

fn keygen_command(slug: &AppSlug) -> Result<CommandSpec, ValidationError> {
    let user = app_user(slug)?;
    let root = AppRoot::new(slug)?;
    let key = AppPath::parse(&root, KEY_FILE)?;
    let comment = format!("ossiframe-{}", slug.as_str());
    CommandSpec::run_as(
        user,
        SSH_KEYGEN,
        &[
            "-q",
            "-t",
            "ed25519",
            "-N",
            "",
            "-C",
            &comment,
            "-f",
            key.as_str(),
        ],
        STEP_TIMEOUT,
    )
}

/// `git ls-remote --heads -- <url> <branch>` as the app user. SSH URLs get
/// `GIT_SSH_COMMAND`; HTTPS URLs get none, so a private HTTPS repository
/// fails instead of prompting. The environment is set with `/usr/bin/env`,
/// the same way the S04 apt steps do it.
pub fn ls_remote_command(
    slug: &AppSlug,
    url: &GitUrl,
    branch: &BranchName,
) -> Result<CommandSpec, ValidationError> {
    let user = app_user(slug)?;
    let root = AppRoot::new(slug)?;
    let ssh_env = format!("GIT_SSH_COMMAND={}", git_ssh_command(&root));
    let mut args: Vec<&str> = Vec::new();
    if url.is_ssh() {
        args.push(&ssh_env);
    }
    args.extend([
        "GIT_TERMINAL_PROMPT=0",
        GIT,
        "ls-remote",
        "--heads",
        "--",
        url.as_str(),
        branch.as_str(),
    ]);
    CommandSpec::run_as(user, ENV, &args, STEP_TIMEOUT)
}

/// What the setup adds to a plan.
#[derive(Debug, Clone)]
pub struct GitSetup {
    pub steps: Vec<PlanStep>,
    pub files: Vec<FileChange>,
}

/// Steps after the private directories exist and the app user is verified:
/// write `known_hosts`, then generate the deploy key. Empty for HTTPS.
pub fn git_setup_steps(
    slug: &AppSlug,
    url: &GitUrl,
    keys: &HostKeys,
) -> Result<GitSetup, ValidationError> {
    let Some(content) = keys.known_hosts() else {
        if url.is_ssh() {
            return Err(ValidationError::new(
                "hostKeys",
                "an SSH URL needs pinned host keys",
            ));
        }
        return Ok(GitSetup {
            steps: Vec::new(),
            files: Vec::new(),
        });
    };
    let slug = app_slug(slug.as_str())?;
    let user = app_user(&slug)?;
    let root = AppRoot::new(&slug)?;
    let hosts = write::app_path(&root, KNOWN_HOSTS_FILE)?;
    let key = write::app_path(&root, KEY_FILE)?;
    let mut steps = write::app_write(
        STEP_KNOWN_HOSTS,
        &user,
        &hosts,
        FileMode::M0600,
        content.into_bytes(),
    )?;
    steps.push(step(
        STEP_DEPLOY_KEY_GENERATE,
        format!("Generate the deploy key {} unless it exists", key.as_str()),
        keygen_command(&slug)?,
        StepMode::Gate,
    ));
    Ok(GitSetup {
        steps,
        files: vec![
            FileChange {
                path: hosts.as_str().to_owned(),
                description: "Pinned Git host keys, app user, mode 0600".to_owned(),
            },
            FileChange {
                path: key.as_str().to_owned(),
                description: "Deploy key (private half stays here), app user, mode 0600".to_owned(),
            },
        ],
    })
}

/// The `deploy-key-check` gate step for a deploy plan.
pub fn git_check_step(
    slug: &AppSlug,
    url: &GitUrl,
    branch: &BranchName,
) -> Result<PlanStep, ValidationError> {
    Ok(step(
        STEP_DEPLOY_KEY_CHECK,
        "Check read access to the repository and that the branch exists".to_owned(),
        ls_remote_command(slug, url, branch)?,
        StepMode::Gate,
    ))
}

// ---------------------------------------------------------------------------
// Access check result and parser
// ---------------------------------------------------------------------------

/// Outcome of the access check. Fixed values only; no raw output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AccessResult {
    /// Authenticated and the branch exists.
    Ok,
    /// The server rejected the deploy key.
    AuthDenied,
    /// The host key is unknown or differs from the pinned key.
    HostKeyFailed,
    /// Reachable, but the branch is not in the repository.
    BranchMissing,
    /// The repository does not exist or the key cannot see it.
    RepoNotFound,
    /// A private HTTPS repository.
    PrivateHttps,
    /// DNS, connection or timeout failure.
    Unreachable,
    /// Anything else.
    Failed,
}

impl AccessResult {
    pub fn passed(self) -> bool {
        self == AccessResult::Ok
    }

    pub fn message(self) -> &'static str {
        match self {
            AccessResult::Ok => "The repository is readable and the branch exists.",
            AccessResult::AuthDenied => {
                "The Git host rejected the deploy key. Add the public key to the repository as a read-only deploy key."
            }
            AccessResult::HostKeyFailed => {
                "Host key verification failed. The host key does not match the pinned key; the connection was refused."
            }
            AccessResult::BranchMissing => {
                "The repository was reached but the branch does not exist."
            }
            AccessResult::RepoNotFound => {
                "The repository was not found, or the deploy key has no access to it."
            }
            AccessResult::PrivateHttps => {
                "Private HTTPS repositories are not supported; use the SSH URL with a deploy key."
            }
            AccessResult::Unreachable => "The Git host could not be reached.",
            AccessResult::Failed => "Git could not read the repository.",
        }
    }
}

/// Parses `git ls-remote --heads` results. Exit 0 needs the exact ref
/// `refs/heads/<branch>`; failures are matched on well-known stderr phrases
/// (OpenSSH and git, and the providers' not-found messages).
pub fn parse_ls_remote(
    exit: &ExitStatus,
    stdout: &str,
    stderr: &str,
    branch: &BranchName,
) -> AccessResult {
    if exit.success() {
        let want = format!("refs/heads/{}", branch.as_str());
        let found = stdout
            .lines()
            .any(|l| l.split_whitespace().nth(1) == Some(want.as_str()));
        return if found {
            AccessResult::Ok
        } else {
            AccessResult::BranchMissing
        };
    }
    let e = stderr.to_ascii_lowercase();
    let has = |s: &str| e.contains(s);
    if has("host key verification failed") || has("remote host identification has changed") {
        AccessResult::HostKeyFailed
    } else if has("permission denied (publickey") {
        AccessResult::AuthDenied
    } else if has("terminal prompts disabled")
        || has("could not read username")
        || has("authentication failed")
    {
        AccessResult::PrivateHttps
    } else if has("repository not found")
        || has("does not appear to be a git repository")
        || has("could not be found")
        || has("repository does not exist")
        || (has("repository '") && has("not found"))
    {
        AccessResult::RepoNotFound
    } else if has("could not resolve hostname")
        || has("connection timed out")
        || has("connection refused")
        || has("no route to host")
        || has("network is unreachable")
    {
        AccessResult::Unreachable
    } else {
        AccessResult::Failed
    }
}

async fn run_check(
    exec: &dyn Executor,
    cancel: &CancelToken,
    cmd: &CommandSpec,
    branch: &BranchName,
) -> Result<AccessResult, OperationError> {
    match exec.run(cmd, cancel).await {
        Ok(r) if r.truncated => Ok(AccessResult::Failed),
        Ok(r) => Ok(parse_ls_remote(&r.exit, &r.stdout, &r.stderr, branch)),
        Err(ExecError::Timeout) => Ok(AccessResult::Unreachable),
        Err(ExecError::Channel(_) | ExecError::PrivilegeDenied) => Ok(AccessResult::Failed),
        Err(e @ (ExecError::Cancelled | ExecError::ConnectionLost)) => Err(OperationError::Exec(e)),
    }
}

/// "Check access": runs the gate command and returns its parsed result.
pub async fn check_access(
    exec: &dyn Executor,
    cancel: &CancelToken,
    slug: &AppSlug,
    url: &GitUrl,
    branch: &BranchName,
) -> Result<AccessResult, OperationError> {
    let cmd = ls_remote_command(slug, url, branch)
        .map_err(|e| OperationError::InvalidArguments(e.to_string()))?;
    run_check(exec, cancel, &cmd, branch).await
}

/// Deploy and clone stay blocked for an SSH URL until the access check
/// passed. HTTPS needs no deploy key.
pub fn deploy_allowed(url: &GitUrl, access: Option<AccessResult>) -> bool {
    !url.is_ssh() || access.is_some_and(AccessResult::passed)
}

// ---------------------------------------------------------------------------
// Deploy key status (binding correction 7)
// ---------------------------------------------------------------------------

/// What the UI shows for the deploy-key setup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeployKeyStatus {
    pub host: String,
    /// The public half; safe to show and copy.
    pub public_key: String,
    /// Where to add the key on this host.
    pub instructions: String,
    pub access: AccessResult,
    pub access_message: String,
}

/// Per-host text for adding a read-only deploy key.
pub fn instructions(host: &str) -> String {
    match host {
        "github.com" => "On GitHub open the repository, then Settings → Deploy keys → Add deploy key. Paste the key and leave \"Allow write access\" unchecked (read-only).",
        "gitlab.com" => "On GitLab open the project, then Settings → Repository → Deploy keys. Add a new key, paste it and leave write access off (read-only).",
        "bitbucket.org" | "ssh.bitbucket.org" => "On Bitbucket open the repository, then Repository settings → Access keys → Add key. Paste the key; access keys are read-only.",
        _ => "Add the public key to the repository on your Git host as a read-only deploy key (the setting is usually called Deploy keys or Access keys).",
    }
    .to_owned()
}

/// Checks the shape of `<root>/.ssh/id_ed25519.pub`: one `ssh-ed25519` line.
pub fn parse_public_key(stdout: &str) -> Result<String, String> {
    let mut lines = stdout.lines().filter(|l| !l.trim().is_empty());
    let (Some(line), None) = (lines.next(), lines.next()) else {
        return Err("the public key file is not a single line".to_owned());
    };
    let f: Vec<&str> = line.split_whitespace().collect();
    let ok = (2..=3).contains(&f.len())
        && f[0] == "ssh-ed25519"
        && f[1]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
        && f.get(2)
            .is_none_or(|c| c.len() <= 100 && c.chars().all(|c| !c.is_control()));
    if ok {
        Ok(f.join(" "))
    } else {
        Err("the public key file is not an ed25519 public key".to_owned())
    }
}

/// Reads only the `.pub` file, as the app user.
pub async fn read_public_key(
    exec: &dyn Executor,
    cancel: &CancelToken,
    slug: &AppSlug,
) -> Result<String, String> {
    let user = v(app_user(slug))?;
    let root = v(AppRoot::new(slug))?;
    let path = v(AppPath::parse(&root, PUB_FILE))?;
    let cat = v(CommandSpec::run_as(
        user,
        CAT,
        &["--", path.as_str()],
        STEP_TIMEOUT,
    ))?;
    let r = run_ok(exec, cancel, &cat, "reading the public key").await?;
    parse_public_key(&r.stdout)
}

/// Public key, instructions and the access check, for the deploy-key setup
/// screen. Errors that are not a failed check (lost connection, no key)
/// return as `Err`.
pub async fn deploy_key_status(
    exec: &dyn Executor,
    cancel: &CancelToken,
    slug: &AppSlug,
    url: &GitUrl,
    branch: &BranchName,
) -> Result<DeployKeyStatus, OperationError> {
    let Some((host, _)) = ssh_endpoint(url) else {
        return Err(OperationError::InvalidArguments(
            "an HTTPS repository has no deploy key".to_owned(),
        ));
    };
    let public_key = read_public_key(exec, cancel, slug)
        .await
        .map_err(OperationError::Failed)?;
    let access = check_access(exec, cancel, slug, url, branch).await?;
    Ok(DeployKeyStatus {
        host: host.as_str().to_owned(),
        public_key,
        instructions: instructions(host.as_str()),
        access,
        access_message: access.message().to_owned(),
    })
}

// ---------------------------------------------------------------------------
// Gates
// ---------------------------------------------------------------------------

/// Whether a key was already there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyOutcome {
    Existing,
    Generated,
}

/// Generates the deploy key unless the file (or a link) exists. An existing
/// private key is never overwritten; the public half must be readable.
pub async fn ensure_deploy_key(
    exec: &dyn Executor,
    cancel: &CancelToken,
    slug: &AppSlug,
) -> Result<KeyOutcome, String> {
    let user = v(app_user(slug))?;
    let root = v(AppRoot::new(slug))?;
    let key = v(AppPath::parse(&root, KEY_FILE))?;
    let mut exists = false;
    for flag in ["-e", "-L"] {
        let t = v(CommandSpec::run_as(
            user.clone(),
            TEST,
            &[flag, key.as_str()],
            STEP_TIMEOUT,
        ))?;
        match run(exec, cancel, &t).await?.0 {
            0 => exists = true,
            1 => {}
            _ => return Err("the deploy key could not be checked".to_owned()),
        }
    }
    if exists {
        read_public_key(exec, cancel, slug).await.map_err(|_| {
            "a deploy key file exists but its public key is unreadable; inspect it manually"
                .to_owned()
        })?;
        return Ok(KeyOutcome::Existing);
    }
    let gen_cmd = v(keygen_command(slug))?;
    run_ok(exec, cancel, &gen_cmd, "ssh-keygen").await?;
    read_public_key(exec, cancel, slug).await?;
    Ok(KeyOutcome::Generated)
}

/// Delegation target of `create::run_gate` for the git gates.
pub async fn run_gate(
    exec: &dyn Executor,
    cancel: &CancelToken,
    plan: &Plan,
    step_id: &str,
) -> Result<String, String> {
    let slug = plan_slug(plan)?;
    match step_id {
        STEP_DEPLOY_KEY_GENERATE => match ensure_deploy_key(exec, cancel, &slug).await? {
            KeyOutcome::Existing => Ok("the existing deploy key is kept".to_owned()),
            KeyOutcome::Generated => Ok("a deploy key was generated".to_owned()),
        },
        STEP_DEPLOY_KEY_CHECK => {
            let cmd = &find(plan, step_id)?.command;
            // Only the shape this module builds is run: `env ... git ls-remote`
            // as the app user recorded for this plan.
            let user = v(app_user(&slug))?;
            let shape_ok = cmd.program() == ENV
                && cmd.privilege() == ossiframe_core::Privilege::RunAs(user)
                && cmd.args().iter().any(|a| a == "ls-remote");
            let branch = cmd
                .args()
                .last()
                .and_then(|b| BranchName::parse(b).ok())
                .filter(|_| shape_ok)
                .ok_or("the access check step is not valid; re-plan required")?;
            let result = run_check(exec, cancel, cmd, &branch)
                .await
                .map_err(|e| e.to_string())?;
            if result.passed() {
                Ok(result.message().to_owned())
            } else {
                Err(result.message().to_owned())
            }
        }
        other => Err(format!("unknown check {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_type::AppType;
    use crate::create::{CreateInput, DbPassword, create_steps};
    use crate::paths::EntryFile;
    use async_trait::async_trait;
    use ossiframe_core::{DomainName, ExecResult, Privilege};
    use std::sync::Mutex;

    fn slug() -> AppSlug {
        app_slug("blog").unwrap()
    }

    fn url(s: &str) -> GitUrl {
        GitUrl::parse(s).unwrap()
    }

    fn branch() -> BranchName {
        BranchName::parse("main").unwrap()
    }

    fn scanned(ty: &str, b64: &str, fp: &str) -> ScannedKey {
        ScannedKey {
            key_type: ty.to_owned(),
            fingerprint: fp.to_owned(),
            key: format!("{ty} {b64}"),
        }
    }

    fn all_commands(steps: &[PlanStep]) -> Vec<String> {
        steps.iter().map(|s| s.command.to_command_line()).collect()
    }

    #[test]
    fn builtin_hosts_yield_a_known_hosts_write_as_the_app_user() {
        for (u, host) in [
            ("git@github.com:o/r.git", "github.com"),
            ("git@gitlab.com:o/r.git", "gitlab.com"),
            ("ssh://git@bitbucket.org/o/r.git", "bitbucket.org"),
        ] {
            let keys = plan_host_keys(&url(u), &[], &[]).unwrap();
            let setup = git_setup_steps(&slug(), &url(u), &keys).unwrap();
            let ids: Vec<&str> = setup.steps.iter().map(|s| s.id.as_str()).collect();
            assert_eq!(
                ids,
                [
                    "git-known-hosts-install",
                    "git-known-hosts-move",
                    STEP_DEPLOY_KEY_GENERATE
                ]
            );
            let install = &setup.steps[0].command;
            assert_eq!(
                install.privilege(),
                Privilege::RunAs(app_user(&slug()).unwrap())
            );
            assert_eq!(&install.args()[..2], ["-m", "0600"]);
            assert!(
                install
                    .args()
                    .last()
                    .unwrap()
                    .starts_with("/srv/ossiframe/apps/blog/.ssh/known_hosts")
            );
            let content = String::from_utf8(install.stdin().unwrap().to_vec()).unwrap();
            assert!(content.lines().count() >= 2);
            assert!(content.lines().all(|l| l.starts_with(&format!("{host} "))));
            assert!(content.contains(" ssh-ed25519 "));
            assert_eq!(setup.steps[2].mode, StepMode::Gate);
        }
    }

    #[test]
    fn shipped_data_covers_every_builtin_host_and_nothing_else() {
        use crate::preflight::BUILTIN_GIT_HOSTS;
        for h in BUILTIN_GIT_HOSTS {
            assert!(
                builtin_keys(h)
                    .iter()
                    .any(|k| k.starts_with("ssh-ed25519 ")),
                "{h}"
            );
        }
        let hosts: std::collections::BTreeSet<&str> = KNOWN_HOSTS_DATA
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
            .filter_map(|l| l.split_once(' ').map(|(h, _)| h))
            .collect();
        assert!(hosts.iter().all(|h| BUILTIN_GIT_HOSTS.contains(h)));
        assert!(KNOWN_HOSTS_DATA.contains("2026-11-25"));
    }

    #[test]
    fn no_rendered_command_contains_accept_new() {
        let mut cmds = Vec::new();
        for u in ["git@github.com:o/r.git", "https://github.com/o/r"] {
            let keys = plan_host_keys(&url(u), &[], &[]).unwrap();
            cmds.extend(all_commands(
                &git_setup_steps(&slug(), &url(u), &keys).unwrap().steps,
            ));
            cmds.push(
                git_check_step(&slug(), &url(u), &branch())
                    .unwrap()
                    .command
                    .to_command_line(),
            );
        }
        let create = create_steps(
            &CreateInput {
                app_type: AppType::Laravel,
                slug: slug(),
                domain: DomainName::parse("blog.example.com").unwrap(),
                entry_file: EntryFile::parse("public/index.php").unwrap(),
                with_database: true,
                disable_default_site: false,
            },
            &DbPassword::generate().unwrap(),
        )
        .unwrap();
        cmds.extend(all_commands(&create.steps));
        cmds.push(KNOWN_HOSTS_DATA.to_owned());
        assert!(cmds.len() > 8);
        assert!(cmds.iter().all(|c| !c.contains("accept-new")));
        assert!(cmds.iter().any(|c| c.contains("StrictHostKeyChecking=yes")));
    }

    #[test]
    fn unconfirmed_host_returns_fingerprints_and_writes_nothing() {
        let u = url("git@git.example.com:o/r.git");
        let scan = [
            scanned("ssh-ed25519", "AAAA1", "SHA256:one"),
            scanned("ssh-rsa", "AAAA2", "SHA256:two"),
        ];
        let Err(e) = plan_host_keys(&u, &scan, &[]) else {
            panic!("must not plan");
        };
        assert_eq!(e.reason(), "gitHostKeyUnconfirmed");
        let GitAccessError::HostKeyUnconfirmed { host, port, keys } = e else {
            panic!("wrong error");
        };
        assert_eq!((host.as_str(), port), ("git.example.com", 22));
        assert_eq!(keys.len(), 2);
        // A fingerprint that is not among the scanned ones confirms nothing.
        assert!(plan_host_keys(&u, &scan, &["SHA256:other".to_owned()]).is_err());
        // Without a scan nothing can be confirmed.
        assert_eq!(
            plan_host_keys(&u, &[], &["SHA256:one".to_owned()])
                .unwrap_err()
                .reason(),
            "gitHostKeyScanFailed"
        );
        // An SSH URL cannot be planned without pinned keys.
        assert!(git_setup_steps(&slug(), &u, &HostKeys::None).is_err());
    }

    #[test]
    fn only_confirmed_scanned_keys_are_written() {
        let u = url("ssh://git@git.example.com:2222/o/r.git");
        let scan = [
            scanned("ssh-ed25519", "AAAA1", "SHA256:one"),
            scanned("ssh-rsa", "AAAA2", "SHA256:two"),
        ];
        let keys = plan_host_keys(&u, &scan, &["SHA256:one".to_owned()]).unwrap();
        let setup = git_setup_steps(&slug(), &u, &keys).unwrap();
        let content = String::from_utf8(setup.steps[0].command.stdin().unwrap().to_vec()).unwrap();
        assert_eq!(content, "[git.example.com]:2222 ssh-ed25519 AAAA1\n");
        // A builtin host on another port is scanned, not trusted from data.
        let u2 = url("ssh://git@github.com:443/o/r.git");
        assert_eq!(
            plan_host_keys(&u2, &[], &[]).unwrap_err().reason(),
            "gitHostKeyScanFailed"
        );
    }

    #[test]
    fn changed_key_requires_a_new_plan() {
        let u = url("git@git.example.com:o/r.git");
        let scan = [scanned("ssh-ed25519", "AAAA1", "SHA256:one")];
        let planned = plan_host_keys(&u, &scan, &["SHA256:one".to_owned()]).unwrap();
        assert!(compare_rescan(&planned, &scan).is_ok());
        let changed = [scanned("ssh-ed25519", "AAAA9", "SHA256:nine")];
        assert_eq!(
            compare_rescan(&planned, &changed).unwrap_err().reason(),
            "rePlanRequired"
        );
        assert!(compare_rescan(&HostKeys::None, &[]).is_ok());
    }

    #[test]
    fn git_ssh_command_uses_only_fixed_options_and_the_app_root() {
        let root = AppRoot::new(&slug()).unwrap();
        let c = git_ssh_command(&root);
        assert_eq!(
            c,
            "/usr/bin/ssh -i /srv/ossiframe/apps/blog/.ssh/id_ed25519 -o IdentitiesOnly=yes \
             -o StrictHostKeyChecking=yes \
             -o UserKnownHostsFile=/srv/ossiframe/apps/blog/.ssh/known_hosts \
             -o GlobalKnownHostsFile=/dev/null -o BatchMode=yes"
        );
        // Safe for `sh` without quoting.
        assert!(
            c.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/._=- ".contains(&b))
        );
        let cmd = ls_remote_command(&slug(), &url("git@github.com:o/r.git"), &branch()).unwrap();
        assert_eq!(cmd.program(), ENV);
        assert_eq!(cmd.args()[0], format!("GIT_SSH_COMMAND={c}"));
        assert_eq!(
            &cmd.args()[1..5],
            ["GIT_TERMINAL_PROMPT=0", GIT, "ls-remote", "--heads"]
        );
        assert_eq!(&cmd.args()[5..], ["--", "git@github.com:o/r.git", "main"]);
    }

    #[test]
    fn ls_remote_output_is_classified() {
        let b = branch();
        let code = |c| ExitStatus::Code(c);
        let p = |c, out: &str, err: &str| parse_ls_remote(&code(c), out, err, &b);
        let sha = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(
            p(0, &format!("{sha}\trefs/heads/main\n"), ""),
            AccessResult::Ok
        );
        // `ls-remote` also lists refs/heads/feature/main for the pattern `main`.
        assert_eq!(
            p(0, &format!("{sha}\trefs/heads/feature/main\n"), ""),
            AccessResult::BranchMissing
        );
        assert_eq!(p(0, "", ""), AccessResult::BranchMissing);
        assert_eq!(
            p(
                128,
                "",
                "git@github.com: Permission denied (publickey).\nfatal: Could not read from remote repository."
            ),
            AccessResult::AuthDenied
        );
        assert_eq!(
            p(
                128,
                "",
                "No ED25519 host key is known for x and you have requested strict checking.\nHost key verification failed."
            ),
            AccessResult::HostKeyFailed
        );
        assert_eq!(
            p(
                128,
                "",
                "ERROR: Repository not found.\nfatal: Could not read from remote repository."
            ),
            AccessResult::RepoNotFound
        );
        assert_eq!(
            p(
                128,
                "",
                "fatal: could not read Username for 'https://github.com': terminal prompts disabled"
            ),
            AccessResult::PrivateHttps
        );
        assert_eq!(
            p(128, "", "ssh: Could not resolve hostname x"),
            AccessResult::Unreachable
        );
        assert_eq!(p(1, "", "something else"), AccessResult::Failed);
        assert!(!AccessResult::AuthDenied.passed() && AccessResult::Ok.passed());
    }

    #[test]
    fn https_url_has_no_key_or_known_hosts_step() {
        let u = url("https://github.com/o/r");
        let keys = plan_host_keys(&u, &[], &[]).unwrap();
        assert_eq!(keys, HostKeys::None);
        let setup = git_setup_steps(&slug(), &u, &keys).unwrap();
        assert!(setup.steps.is_empty() && setup.files.is_empty());
        let cmd = ls_remote_command(&slug(), &u, &branch()).unwrap();
        let line = cmd.to_command_line();
        assert!(!line.contains("GIT_SSH_COMMAND") && !line.contains("known_hosts"));
        assert_eq!(cmd.args()[0], "GIT_TERMINAL_PROMPT=0");
        assert!(deploy_allowed(&u, None));
        let s = url("git@github.com:o/r.git");
        assert!(!deploy_allowed(&s, None));
        assert!(!deploy_allowed(&s, Some(AccessResult::AuthDenied)));
        assert!(deploy_allowed(&s, Some(AccessResult::Ok)));
    }

    struct Script {
        key_exists: bool,
        calls: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl Executor for Script {
        async fn run(&self, s: &CommandSpec, _: &CancelToken) -> Result<ExecResult, ExecError> {
            self.calls.lock().unwrap().push(s.program().to_owned());
            let (code, out) = match s.program() {
                TEST if s.args()[0] == "-e" => (i32::from(!self.key_exists), ""),
                TEST => (1, ""),
                CAT => (0, "ssh-ed25519 AAAAC3Nza ossiframe-blog\n"),
                _ => (0, ""),
            };
            Ok(ExecResult {
                exit: ExitStatus::Code(code),
                stdout: out.to_owned(),
                stderr: String::new(),
                truncated: false,
            })
        }
    }

    #[tokio::test]
    async fn existing_deploy_key_is_not_regenerated() {
        let run = |exists| async move {
            let fake = Script {
                key_exists: exists,
                calls: Mutex::new(Vec::new()),
            };
            let out = ensure_deploy_key(&fake, &CancelToken::new(), &slug())
                .await
                .unwrap();
            let generated = fake.calls.lock().unwrap().iter().any(|p| p == SSH_KEYGEN);
            (out, generated)
        };
        assert_eq!(run(true).await, (KeyOutcome::Existing, false));
        assert_eq!(run(false).await, (KeyOutcome::Generated, true));
        let cmd = keygen_command(&slug()).unwrap();
        assert_eq!(
            cmd.args(),
            [
                "-q",
                "-t",
                "ed25519",
                "-N",
                "",
                "-C",
                "ossiframe-blog",
                "-f",
                "/srv/ossiframe/apps/blog/.ssh/id_ed25519"
            ]
        );
    }

    #[test]
    fn public_key_and_instructions() {
        assert_eq!(
            parse_public_key("ssh-ed25519 AAAAC3Nza ossiframe-blog\n").unwrap(),
            "ssh-ed25519 AAAAC3Nza ossiframe-blog"
        );
        assert!(parse_public_key("-----BEGIN OPENSSH PRIVATE KEY-----\n").is_err());
        assert!(parse_public_key("ssh-rsa AAAA x\n").is_err());
        assert!(parse_public_key("ssh-ed25519 AAAA x\nssh-ed25519 BBBB y\n").is_err());
        assert!(instructions("github.com").contains("Deploy keys"));
        assert!(instructions("gitlab.com").contains("Settings → Repository → Deploy keys"));
        assert!(instructions("bitbucket.org").contains("Access keys"));
        assert!(instructions("git.example.com").contains("read-only"));
    }
}
