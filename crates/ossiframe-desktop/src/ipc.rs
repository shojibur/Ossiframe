//! IPC data types shared by Ossiframe desktop apps. Inbound DTOs carry raw
//! values and are validated here, at the trust boundary. Outbound DTOs
//! never contain secrets.

use crate::service::{ConnectOutcome, CredentialKind, ProfileInput, ServerView, ServiceError};
use crate::session::ConnState;
use crate::store::AuthConfig;
use crate::tokens::TokenError;
use ossiframe_core::{ExecError, Hostname, KeyPath, Port, ServerName, SshUser, ValidationError};
use ossiframe_ssh::ConnectError;
use serde::{Deserialize, Serialize};

/// Maximum length of any message or excerpt sent to the frontend.
pub const MAX_MESSAGE: usize = 2048;

pub fn cap(s: &str) -> String {
    if s.len() <= MAX_MESSAGE {
        return s.to_owned();
    }
    let mut end = MAX_MESSAGE;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_owned()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AuthKindDto {
    KeyFile,
    Password,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileInputDto {
    pub name: String,
    pub host: String,
    pub port: u32,
    pub username: String,
    pub auth_kind: AuthKindDto,
    pub key_path: Option<String>,
}

impl ProfileInputDto {
    pub fn validate(&self) -> Result<ProfileInput, IpcError> {
        let auth = match self.auth_kind {
            AuthKindDto::Password => AuthConfig::Password,
            AuthKindDto::KeyFile => {
                let p = self.key_path.as_deref().unwrap_or("");
                AuthConfig::KeyFile(KeyPath::parse(p)?)
            }
        };
        Ok(ProfileInput {
            name: ServerName::parse(&self.name)?,
            host: Hostname::parse(self.host.trim())?,
            port: Port::new(self.port)?,
            username: SshUser::parse(self.username.trim())?,
            auth,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostKeyDto {
    pub key_type: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerDto {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_kind: AuthKindDto,
    pub key_path: Option<String>,
    pub has_saved_secret: bool,
    pub pinned_host_key: Option<HostKeyDto>,
    pub state: ConnState,
}

impl From<ServerView> for ServerDto {
    fn from(v: ServerView) -> Self {
        let r = v.record;
        let (auth_kind, key_path) = match r.auth {
            AuthConfig::KeyFile(p) => (AuthKindDto::KeyFile, Some(p.to_string())),
            AuthConfig::Password => (AuthKindDto::Password, None),
        };
        Self {
            id: r.id,
            name: r.name.into(),
            host: r.host.into(),
            port: r.port.get(),
            username: r.username.into(),
            auth_kind,
            key_path,
            has_saved_secret: r.secret_ref.is_some(),
            pinned_host_key: v.pin.map(|p| HostKeyDto {
                key_type: p.key_type,
                sha256: p.sha256,
            }),
            state: v.state,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ConnectOutcomeDto {
    Connected {
        warning: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    HostKeyUnknown {
        token: String,
        key_type: String,
        sha256: String,
    },
}

impl From<ConnectOutcome> for ConnectOutcomeDto {
    fn from(o: ConnectOutcome) -> Self {
        match o {
            ConnectOutcome::Connected { warning } => Self::Connected {
                warning: warning.map(|w| cap(&w)),
            },
            ConnectOutcome::HostKeyUnknown { token, key } => Self::HostKeyUnknown {
                token,
                key_type: key.key_type,
                sha256: key.sha256,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CredentialKindDto {
    Password,
    Passphrase,
}

/// Fixed message for a privileged command refused by the read-only guard.
pub const PRIVILEGE_DENIED_MESSAGE: &str = "privileged command was not allowed";

/// Why a plan or operation request was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PlanReason {
    UnknownPlan,
    Superseded,
    Expired,
    Mismatch,
    WrongServer,
    Busy,
    ServerInUse,
    Blocked,
    RePlanRequired,
    NotSupported,
    /// The application id is unknown (S05).
    AppNotFound,
    /// The domain is already used by another application.
    DomainInUse,
    /// The slug is already used on this server.
    SlugInUse,
    /// The Git host's SSH keys are not confirmed; the error carries their
    /// fingerprints.
    GitHostKeyUnconfirmed,
}

/// A scanned SSH host key of a Git host, shown so the user can compare its
/// fingerprint with the host's published one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHostFingerprint {
    pub host: String,
    pub port: u16,
    pub key_type: String,
    /// `SHA256:...`
    pub sha256: String,
}

/// Errors returned to the frontend. Messages are short and sanitized.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum IpcError {
    #[error("{field}: {message}")]
    Validation { field: String, message: String },
    #[error("server not found")]
    NotFound,
    #[error("unreachable: {message}")]
    Unreachable { message: String },
    #[error("connection timed out")]
    ConnectTimeout,
    #[error("authentication timed out")]
    AuthTimeout,
    #[error("host key changed")]
    HostKeyMismatch {
        pinned_key_type: String,
        pinned_sha256: String,
        presented_key_type: String,
        presented_sha256: String,
        replace_token: String,
    },
    #[error("authentication failed")]
    AuthFailed,
    #[error("credential required")]
    CredentialRequired { credential: CredentialKindDto },
    #[error("cannot read key file")]
    KeyFileUnreadable { path: String },
    #[error("not connected")]
    NotConnected,
    #[error("command timed out")]
    CommandTimeout,
    #[error("cancelled")]
    Cancelled,
    #[error("connection lost")]
    ConnectionLost,
    #[error("keychain: {message}")]
    Keychain { message: String },
    #[error("storage: {message}")]
    Storage { message: String },
    #[error("host key approval: {message}")]
    HostKeyApproval { message: String },
    #[error("a host key is already pinned")]
    AlreadyPinned,
    #[error("confirmation does not match")]
    ConfirmationMismatch,
    #[error("SSH protocol error: {message}")]
    Protocol { message: String },
    /// A module operation failed. The message is short and never contains
    /// raw command output.
    #[error("operation failed: {message}")]
    OperationFailed { message: String },
    /// A plan, approval or operation request was refused. The message is
    /// short and never contains raw command output.
    #[error("plan: {message}")]
    Plan {
        reason: PlanReason,
        message: String,
        /// Only for `gitHostKeyUnconfirmed`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        fingerprints: Vec<GitHostFingerprint>,
    },
    #[error("internal error")]
    Internal,
}

impl From<ValidationError> for IpcError {
    fn from(e: ValidationError) -> Self {
        IpcError::Validation {
            field: e.field.into(),
            message: cap(&e.reason),
        }
    }
}

impl From<ExecError> for IpcError {
    fn from(e: ExecError) -> Self {
        match e {
            ExecError::Timeout => IpcError::CommandTimeout,
            ExecError::Cancelled => IpcError::Cancelled,
            ExecError::ConnectionLost => IpcError::ConnectionLost,
            ExecError::Channel(m) => IpcError::Protocol { message: cap(&m) },
            ExecError::PrivilegeDenied => IpcError::OperationFailed {
                message: PRIVILEGE_DENIED_MESSAGE.into(),
            },
        }
    }
}

impl From<TokenError> for IpcError {
    fn from(e: TokenError) -> Self {
        IpcError::HostKeyApproval {
            message: e.to_string(),
        }
    }
}

impl From<ServiceError> for IpcError {
    fn from(e: ServiceError) -> Self {
        match e {
            ServiceError::NotFound => IpcError::NotFound,
            ServiceError::Storage(m) => IpcError::Storage { message: cap(&m) },
            ServiceError::Keychain(m) => IpcError::Keychain { message: cap(&m) },
            ServiceError::CredentialRequired(k) => IpcError::CredentialRequired {
                credential: match k {
                    CredentialKind::Password => CredentialKindDto::Password,
                    CredentialKind::Passphrase => CredentialKindDto::Passphrase,
                },
            },
            ServiceError::HostKeyMismatch {
                pinned,
                presented,
                replace_token,
            } => IpcError::HostKeyMismatch {
                pinned_key_type: pinned.key_type,
                pinned_sha256: pinned.sha256,
                presented_key_type: presented.key_type,
                presented_sha256: presented.sha256,
                replace_token,
            },
            ServiceError::Token(t) => t.into(),
            ServiceError::AlreadyPinned => IpcError::AlreadyPinned,
            ServiceError::ConfirmationMismatch => IpcError::ConfirmationMismatch,
            ServiceError::NotConnected => IpcError::NotConnected,
            ServiceError::ServerInUse => IpcError::Plan {
                reason: PlanReason::ServerInUse,
                message: "the server has an operation that is running or needs review".into(),
                fingerprints: Vec::new(),
            },
            ServiceError::Exec(e) => e.into(),
            ServiceError::Connect(c) => match c {
                ConnectError::Unreachable(m) => IpcError::Unreachable { message: cap(&m) },
                ConnectError::ConnectTimeout => IpcError::ConnectTimeout,
                ConnectError::AuthTimeout => IpcError::AuthTimeout,
                ConnectError::AuthFailed => IpcError::AuthFailed,
                ConnectError::KeyFileUnreadable(p) => IpcError::KeyFileUnreadable { path: cap(&p) },
                ConnectError::Protocol(m) => IpcError::Protocol { message: cap(&m) },
                // Handled by the service; mapped defensively.
                ConnectError::PassphraseRequired => IpcError::CredentialRequired {
                    credential: CredentialKindDto::Passphrase,
                },
                ConnectError::PasswordRequired => IpcError::CredentialRequired {
                    credential: CredentialKindDto::Password,
                },
                ConnectError::HostKeyUnknown(_) | ConnectError::HostKeyMismatch { .. } => {
                    IpcError::Internal
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dto() -> ProfileInputDto {
        ProfileInputDto {
            name: "web".into(),
            host: "example.com".into(),
            port: 22,
            username: "root".into(),
            auth_kind: AuthKindDto::Password,
            key_path: None,
        }
    }

    #[test]
    fn invalid_input_names_the_field() {
        let mut d = dto();
        d.host = "-oProxyCommand=x".into();
        assert!(matches!(d.validate(), Err(IpcError::Validation { field, .. }) if field == "host"));
        let mut d = dto();
        d.port = 0;
        assert!(matches!(d.validate(), Err(IpcError::Validation { field, .. }) if field == "port"));
        let mut d = dto();
        d.auth_kind = AuthKindDto::KeyFile;
        assert!(
            matches!(d.validate(), Err(IpcError::Validation { field, .. }) if field == "keyPath")
        );
        assert!(dto().validate().is_ok());
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let r: Result<ProfileInputDto, _> = serde_json::from_str(
            r#"{"name":"a","host":"h","port":22,"username":"u","authKind":"password","keyPath":null,"command":"rm -rf /"}"#,
        );
        assert!(r.is_err());
    }

    #[test]
    fn errors_serialize_with_kind_and_no_internal_detail() {
        assert_eq!(
            serde_json::to_value(IpcError::Internal).unwrap(),
            serde_json::json!({"kind": "internal"})
        );
        let v = serde_json::to_value(IpcError::from(ServiceError::CredentialRequired(
            CredentialKind::Passphrase,
        )))
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({"kind": "credentialRequired", "credential": "passphrase"})
        );
        assert_eq!(
            IpcError::from(ServiceError::Exec(ExecError::Timeout)),
            IpcError::CommandTimeout
        );
        assert_eq!(
            IpcError::from(ServiceError::Connect(ConnectError::AuthFailed)),
            IpcError::AuthFailed
        );
    }

    #[test]
    fn operation_failed_serializes_with_kind() {
        assert_eq!(
            serde_json::to_value(IpcError::OperationFailed {
                message: "result too large".into()
            })
            .unwrap(),
            serde_json::json!({"kind": "operationFailed", "message": "result too large"})
        );
    }

    #[test]
    fn privilege_denied_maps_to_fixed_message() {
        assert_eq!(
            IpcError::from(ExecError::PrivilegeDenied),
            IpcError::OperationFailed {
                message: PRIVILEGE_DENIED_MESSAGE.into()
            }
        );
    }

    #[test]
    fn plan_error_serializes_with_camel_case_reason() {
        let v = serde_json::to_value(IpcError::Plan {
            reason: PlanReason::ServerInUse,
            message: "x".into(),
            fingerprints: Vec::new(),
        })
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({"kind": "plan", "reason": "serverInUse", "message": "x"})
        );
        let v = serde_json::to_value(PlanReason::RePlanRequired).unwrap();
        assert_eq!(v, serde_json::json!("rePlanRequired"));
    }

    #[test]
    fn app_plan_reasons_serialize_and_fingerprints_ride_on_the_plan_error() {
        for (r, name) in [
            (PlanReason::AppNotFound, "appNotFound"),
            (PlanReason::DomainInUse, "domainInUse"),
            (PlanReason::SlugInUse, "slugInUse"),
            (PlanReason::GitHostKeyUnconfirmed, "gitHostKeyUnconfirmed"),
        ] {
            assert_eq!(serde_json::to_value(r).unwrap(), serde_json::json!(name));
        }
        let v = serde_json::to_value(IpcError::Plan {
            reason: PlanReason::GitHostKeyUnconfirmed,
            message: "confirm".into(),
            fingerprints: vec![GitHostFingerprint {
                host: "git.example.org".into(),
                port: 22,
                key_type: "ssh-ed25519".into(),
                sha256: "SHA256:abc".into(),
            }],
        })
        .unwrap();
        assert_eq!(
            v["fingerprints"][0],
            serde_json::json!({"host": "git.example.org", "port": 22, "keyType": "ssh-ed25519", "sha256": "SHA256:abc"})
        );
    }

    #[test]
    fn messages_are_capped() {
        let long = "é".repeat(5000);
        let IpcError::Storage { message } = IpcError::from(ServiceError::Storage(long)) else {
            panic!()
        };
        assert!(message.len() <= MAX_MESSAGE);
    }
}
