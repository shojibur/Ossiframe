//! Inputs and outputs of the `app_*` commands.
//!
//! Inbound values are raw strings from the frontend. They are parsed into
//! typed values here, before anything else happens. Outbound types hold no
//! secret: no database password, no secret environment value, no private
//! key, and no command text beyond what the stored plan shows.
use ossiframe_core::{AppSlug, BranchName, DomainName, GitUrl};
use ossiframe_desktop::ipc::IpcError;
use ossiframe_desktop::store::{AppRecord, ReleaseRecord};
use ossiframe_mod_app_deploy::app_type::AppType;
use ossiframe_mod_app_deploy::env::{EnvChange, EnvKey, EnvOp, EnvValue};
use ossiframe_mod_app_deploy::git_access::AccessResult;
use ossiframe_mod_app_deploy::identity::app_slug;
use ossiframe_mod_app_deploy::paths::EntryFile;
use ossiframe_mod_app_deploy::release::ReleaseId;
use serde::{Deserialize, Serialize};

/// Most fingerprints a create request may confirm.
const MAX_FINGERPRINTS: usize = 8;
/// Most variables one `app_set_env` request may change.
pub const MAX_ENV_CHANGES: usize = 64;

fn validation(field: &str, message: &str) -> IpcError {
    IpcError::Validation {
        field: field.to_owned(),
        message: message.to_owned(),
    }
}

/// Ids of application types, as `AppTypeInfo::id`.
pub fn parse_app_type(input: &str) -> Result<AppType, IpcError> {
    AppType::ALL
        .into_iter()
        .find(|t| t.info().id == input)
        .ok_or_else(|| validation("appType", "must be laravel, php or static"))
}

/// `SHA256:` followed by base64, as `ssh-keygen -l` prints it.
fn fingerprint(input: &str) -> Result<String, IpcError> {
    let ok = input.strip_prefix("SHA256:").is_some_and(|b| {
        (20..=64).contains(&b.len())
            && b.bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"+/=".contains(&c))
    });
    if ok {
        Ok(input.to_owned())
    } else {
        Err(validation(
            "trustedGitHostFingerprints",
            "each fingerprint must look like SHA256:...",
        ))
    }
}

/// Raw `app_create_plan` arguments.
#[derive(Debug, Clone)]
pub struct CreateParams {
    pub server_id: String,
    pub app_type: String,
    pub slug: String,
    pub domain: String,
    pub repo_url: String,
    pub repo_branch: String,
    pub entry_file: Option<String>,
    pub with_database: bool,
    pub disable_default_site: bool,
    pub trusted_git_host_fingerprints: Option<Vec<String>>,
}

/// The same arguments as typed values.
#[derive(Debug, Clone)]
pub struct CreateInputs {
    pub app_type: AppType,
    pub slug: AppSlug,
    pub domain: DomainName,
    pub url: GitUrl,
    pub branch: BranchName,
    pub entry: EntryFile,
    pub with_database: bool,
    pub disable_default_site: bool,
    pub confirmed: Vec<String>,
}

impl CreateParams {
    pub fn parse(&self) -> Result<CreateInputs, IpcError> {
        let app_type = parse_app_type(&self.app_type)?;
        let slug = app_slug(&self.slug)?;
        let domain = DomainName::parse(self.domain.trim())?;
        let url = GitUrl::parse(self.repo_url.trim())?;
        let branch = BranchName::parse(self.repo_branch.trim())?;
        let entry = EntryFile::parse(
            self.entry_file
                .as_deref()
                .map_or(app_type.info().entry_file, str::trim),
        )?;
        let given = self.trusted_git_host_fingerprints.as_deref().unwrap_or(&[]);
        if given.len() > MAX_FINGERPRINTS {
            return Err(validation(
                "trustedGitHostFingerprints",
                "too many fingerprints",
            ));
        }
        let confirmed = given
            .iter()
            .map(|f| fingerprint(f))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(CreateInputs {
            app_type,
            slug,
            domain,
            url,
            branch,
            entry,
            with_database: self.with_database,
            disable_default_site: self.disable_default_site,
            confirmed,
        })
    }
}

/// One environment change from the frontend. Only changed keys are sent. A
/// secret's current value never reaches the frontend, so an unchanged secret
/// is simply absent here and is kept by the backend merge.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvChangeDto {
    pub key: String,
    /// The new value. Must be absent when `delete` is true.
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub delete: bool,
}

/// Validates the list into typed changes (bounded, no duplicates).
pub fn parse_env_changes(changes: &[EnvChangeDto]) -> Result<Vec<EnvChange>, IpcError> {
    if changes.is_empty() || changes.len() > MAX_ENV_CHANGES {
        return Err(validation("changes", "send between 1 and 64 changes"));
    }
    let mut out: Vec<EnvChange> = Vec::with_capacity(changes.len());
    for c in changes {
        let key = EnvKey::parse(&c.key)?;
        if out.iter().any(|o| o.key == key) {
            return Err(validation("changes", "a key appears twice"));
        }
        let op = match (&c.value, c.delete) {
            (None, true) => EnvOp::Delete,
            (Some(v), false) => EnvOp::Set(EnvValue::parse(v)?),
            (Some(_), true) => return Err(validation("changes", "a delete carries no value")),
            (None, false) => return Err(validation("changes", "a change needs a value")),
        };
        out.push(EnvChange { key, op });
    }
    Ok(out)
}

// ---- release ids ------------------------------------------------------------------

/// A release row is keyed by app and release id: the release id is a UTC
/// timestamp, so two apps can share one.
pub fn release_row_id(app_id: &str, release: &ReleaseId) -> String {
    format!("{app_id}:{}", release.as_str())
}

/// The release id of a row id that belongs to `app_id`.
pub fn release_of_row(app_id: &str, row_id: &str) -> Option<ReleaseId> {
    row_id
        .strip_prefix(app_id)
        .and_then(|r| r.strip_prefix(':'))
        .and_then(|r| ReleaseId::parse(r).ok())
}

// ---- responses ----------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSummaryDto {
    pub id: String,
    pub slug: String,
    pub app_type: String,
    pub domain: String,
    pub state: String,
    pub current_release_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseDto {
    /// The release id (a UTC timestamp), as used by `app_rollback_plan`.
    pub id: String,
    pub commit_sha: Option<String>,
    pub branch: String,
    pub status: String,
    pub operation_id: Option<String>,
    pub created_at: String,
}

impl ReleaseDto {
    pub fn from_record(r: &ReleaseRecord) -> Self {
        let id = release_of_row(&r.app_id, &r.id)
            .map_or_else(|| r.id.clone(), |i| i.as_str().to_owned());
        Self {
            id,
            commit_sha: r.commit_sha.clone(),
            branch: r.branch.clone(),
            status: r.status.clone(),
            operation_id: r.operation_id.clone(),
            created_at: r.created_at.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppDto {
    pub id: String,
    pub server_id: String,
    pub slug: String,
    pub app_type: String,
    pub domain: String,
    pub repo_url: String,
    pub repo_branch: String,
    pub entry_file: String,
    pub state: String,
    /// Only whether a database exists, never its password.
    pub has_database: bool,
    /// The repository needs a deploy key (an SSH URL).
    pub needs_deploy_key: bool,
    pub current_release: Option<ReleaseDto>,
    pub created_at: String,
    pub updated_at: String,
}

impl AppDto {
    pub fn new(a: &AppRecord, current: Option<&ReleaseRecord>) -> Self {
        Self {
            id: a.id.clone(),
            server_id: a.server_id.clone(),
            slug: a.slug.clone(),
            app_type: a.app_type.clone(),
            domain: a.domain.clone(),
            repo_url: a.repo_url.clone(),
            repo_branch: a.repo_branch.clone(),
            entry_file: a.entry_file.clone(),
            state: a.state.clone(),
            has_database: a.db_enabled,
            needs_deploy_key: !a.repo_url.starts_with("https://"),
            current_release: current.map(ReleaseDto::from_record),
            created_at: a.created_at.clone(),
            updated_at: a.updated_at.clone(),
        }
    }
}

impl AppSummaryDto {
    pub fn new(a: &AppRecord) -> Self {
        Self {
            id: a.id.clone(),
            slug: a.slug.clone(),
            app_type: a.app_type.clone(),
            domain: a.domain.clone(),
            state: a.state.clone(),
            current_release_id: a
                .current_release_id
                .as_deref()
                .and_then(|r| release_of_row(&a.id, r))
                .map(|r| r.as_str().to_owned()),
        }
    }
}

/// Result of `app_check_access`: a fixed outcome and its fixed message.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessCheckDto {
    pub result: AccessResult,
    pub passed: bool,
    pub message: String,
}

impl From<AccessResult> for AccessCheckDto {
    fn from(r: AccessResult) -> Self {
        Self {
            result: r,
            passed: r.passed(),
            message: r.message().to_owned(),
        }
    }
}
