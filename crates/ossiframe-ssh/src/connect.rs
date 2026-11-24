use crate::exec::SshExecutor;
use ossiframe_core::host_key::{HostKeyDecision, PinnedHostKey, PresentedHostKey, decide};
use ossiframe_core::{Hostname, KeyPath, Port, SecretString, SshUser};
use russh::client;
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct ConnectTarget {
    pub host: Hostname,
    pub port: Port,
    pub user: SshUser,
}

/// Credentials for one connection attempt. Secrets are zeroized on drop.
#[derive(Debug)]
pub enum AuthMethod {
    KeyFile {
        path: KeyPath,
        passphrase: Option<SecretString>,
    },
    Password(SecretString),
}

#[derive(Debug, Clone)]
pub struct ConnectOptions {
    pub connect_timeout: Duration,
    /// Send a keepalive after this long without traffic from the server.
    pub keepalive_interval: Duration,
    /// Close the connection after this many unanswered keepalives.
    pub keepalive_max: usize,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(15),
            keepalive_interval: Duration::from_secs(15),
            keepalive_max: 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConnectError {
    #[error("server is unreachable: {0}")]
    Unreachable(String),
    #[error("connection timed out")]
    ConnectTimeout,
    #[error("host key is not trusted yet")]
    HostKeyUnknown(Box<PresentedHostKey>),
    #[error("host key does not match the pinned key")]
    HostKeyMismatch {
        pinned: Box<PinnedHostKey>,
        presented: Box<PresentedHostKey>,
    },
    #[error("authentication failed")]
    AuthFailed,
    #[error("the private key is encrypted and needs a passphrase")]
    PassphraseRequired,
    #[error("a password is required")]
    PasswordRequired,
    #[error("cannot read private key file {0}")]
    KeyFileUnreadable(String),
    #[error("SSH protocol error: {0}")]
    Protocol(String),
}

/// Records the host key decision so `connect` can report it after russh
/// aborts the handshake.
pub(crate) struct HostKeyHandler {
    pinned: Option<PinnedHostKey>,
    decision: Arc<Mutex<Option<HostKeyCheck>>>,
}

#[derive(Debug, Clone)]
pub(crate) enum HostKeyCheck {
    Decided(HostKeyDecision),
    CertificateRejected,
}

impl HostKeyHandler {
    fn check(&mut self, key: &PublicKeyOrCertificate) -> bool {
        let check = match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => match presented_key(key) {
                Some(presented) => HostKeyCheck::Decided(decide(self.pinned.as_ref(), &presented)),
                None => HostKeyCheck::CertificateRejected,
            },
            // Host certificates are not supported in S01.
            PublicKeyOrCertificate::Certificate(_) => HostKeyCheck::CertificateRejected,
        };
        let trusted = matches!(check, HostKeyCheck::Decided(HostKeyDecision::Trusted));
        *self.decision.lock().unwrap_or_else(|e| e.into_inner()) = Some(check);
        trusted
    }
}

impl client::Handler for HostKeyHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        Ok(self.check(key))
    }
}

/// Key type, wire-format bytes and SHA-256 fingerprint, as computed by `ssh-key`.
pub(crate) fn presented_key(key: &PublicKey) -> Option<PresentedHostKey> {
    Some(PresentedHostKey {
        key_type: key.algorithm().as_str().to_owned(),
        public_key: key.to_bytes().ok()?,
        sha256: key.fingerprint(HashAlg::Sha256).to_string(),
    })
}

/// Turns a failed handshake into the most specific error.
pub(crate) fn handshake_failure(check: Option<HostKeyCheck>, err: &str) -> ConnectError {
    match check {
        Some(HostKeyCheck::Decided(HostKeyDecision::Unknown(k))) => {
            ConnectError::HostKeyUnknown(Box::new(k))
        }
        Some(HostKeyCheck::Decided(HostKeyDecision::Mismatch { pinned, presented })) => {
            ConnectError::HostKeyMismatch {
                pinned: Box::new(pinned),
                presented: Box::new(presented),
            }
        }
        Some(HostKeyCheck::CertificateRejected) => {
            ConnectError::Protocol("host certificates are not supported".into())
        }
        Some(HostKeyCheck::Decided(HostKeyDecision::Trusted)) | None => {
            ConnectError::Unreachable(short(err))
        }
    }
}

fn short(s: &str) -> String {
    s.chars().take(200).collect()
}

fn load_key(
    path: &KeyPath,
    passphrase: Option<&SecretString>,
) -> Result<russh::keys::PrivateKey, ConnectError> {
    let text = std::fs::read_to_string(path.to_path_buf())
        .map_err(|_| ConnectError::KeyFileUnreadable(path.as_str().to_owned()))?;
    let text = zeroize::Zeroizing::new(text);
    match russh::keys::decode_secret_key(&text, passphrase.map(SecretString::expose)) {
        Ok(k) => Ok(k),
        Err(russh::keys::Error::KeyIsEncrypted) => Err(ConnectError::PassphraseRequired),
        // Wrong passphrase or corrupt key. Do not say which.
        Err(_) if passphrase.is_some() => Err(ConnectError::AuthFailed),
        Err(_) => Err(ConnectError::KeyFileUnreadable(path.as_str().to_owned())),
    }
}

/// Connects, verifies the host key against `pinned`, then authenticates.
///
/// If the key is unknown or does not match the pin, the handshake is aborted
/// inside the host key callback and no credential is sent.
pub async fn connect(
    target: &ConnectTarget,
    auth: &AuthMethod,
    pinned: Option<PinnedHostKey>,
    opts: &ConnectOptions,
) -> Result<SshExecutor, ConnectError> {
    let config = Arc::new(client::Config {
        keepalive_interval: Some(opts.keepalive_interval),
        keepalive_max: opts.keepalive_max,
        // Sessions stay open while idle; keepalives detect dead peers.
        inactivity_timeout: None,
        ..Default::default()
    });
    let decision = Arc::new(Mutex::new(None));
    let handler = HostKeyHandler {
        pinned,
        decision: decision.clone(),
    };
    let addr = (target.host.as_str().to_owned(), target.port.get());

    let mut handle =
        match tokio::time::timeout(opts.connect_timeout, client::connect(config, addr, handler))
            .await
        {
            Err(_) => return Err(ConnectError::ConnectTimeout),
            Ok(Err(e)) => {
                let check = decision.lock().unwrap_or_else(|p| p.into_inner()).take();
                return Err(handshake_failure(check, &e.to_string()));
            }
            Ok(Ok(h)) => h,
        };

    // From here the host key is trusted.
    let user = target.user.as_str();
    let auth_result = match auth {
        AuthMethod::Password(pw) => {
            if pw.is_empty() {
                let _ = handle
                    .disconnect(russh::Disconnect::ByApplication, "", "en")
                    .await;
                return Err(ConnectError::PasswordRequired);
            }
            handle.authenticate_password(user, pw.expose()).await
        }
        AuthMethod::KeyFile { path, passphrase } => {
            let key = match load_key(path, passphrase.as_ref()) {
                Ok(k) => k,
                Err(e) => {
                    let _ = handle
                        .disconnect(russh::Disconnect::ByApplication, "", "en")
                        .await;
                    return Err(e);
                }
            };
            let hash = if key.algorithm().is_rsa() {
                match tokio::time::timeout(Duration::from_secs(5), handle.best_supported_rsa_hash())
                    .await
                {
                    Ok(Ok(h)) => h.flatten(),
                    _ => Some(HashAlg::Sha256),
                }
            } else {
                None
            };
            handle
                .authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
                .await
        }
    };

    match auth_result {
        Ok(r) if r.success() => Ok(SshExecutor::new(handle)),
        Ok(_) => {
            let _ = handle
                .disconnect(russh::Disconnect::ByApplication, "", "en")
                .await;
            Err(ConnectError::AuthFailed)
        }
        Err(e) => Err(ConnectError::Protocol(short(&e.to_string()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ossiframe_core::host_key::PresentedHostKey;

    // An Ed25519 public key in OpenSSH format (test vector, no private key).
    const ED25519: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl test";

    fn key() -> PublicKey {
        PublicKey::from_openssh(ED25519).unwrap()
    }

    #[test]
    fn handler_trusts_only_the_pinned_key() {
        let presented = presented_key(&key()).unwrap();
        let wrapped = PublicKeyOrCertificate::PublicKey {
            key: key(),
            hash_alg: None,
        };

        let state = Arc::new(Mutex::new(None));
        let mut h = HostKeyHandler {
            pinned: None,
            decision: state.clone(),
        };
        assert!(!h.check(&wrapped), "unknown key must be rejected");
        assert!(matches!(
            state.lock().unwrap().clone(),
            Some(HostKeyCheck::Decided(HostKeyDecision::Unknown(_)))
        ));

        let mut h = HostKeyHandler {
            pinned: Some(presented.clone().into()),
            decision: state.clone(),
        };
        assert!(h.check(&wrapped));

        let other = PinnedHostKey {
            key_type: presented.key_type.clone(),
            public_key: vec![1, 2, 3],
            sha256: "x".into(),
        };
        let mut h = HostKeyHandler {
            pinned: Some(other),
            decision: state.clone(),
        };
        assert!(!h.check(&wrapped), "changed key must be rejected");
        assert!(matches!(
            state.lock().unwrap().clone(),
            Some(HostKeyCheck::Decided(HostKeyDecision::Mismatch { .. }))
        ));
    }

    #[test]
    fn handshake_failures_map_to_host_key_errors() {
        let k = presented_key(&key()).unwrap();
        assert!(matches!(
            handshake_failure(
                Some(HostKeyCheck::Decided(HostKeyDecision::Unknown(k.clone()))),
                "x"
            ),
            ConnectError::HostKeyUnknown(_)
        ));
        let m = HostKeyDecision::Mismatch {
            pinned: k.clone().into(),
            presented: k,
        };
        assert!(matches!(
            handshake_failure(Some(HostKeyCheck::Decided(m)), "x"),
            ConnectError::HostKeyMismatch { .. }
        ));
        assert!(matches!(
            handshake_failure(None, "refused"),
            ConnectError::Unreachable(_)
        ));
        assert!(matches!(
            handshake_failure(Some(HostKeyCheck::CertificateRejected), "x"),
            ConnectError::Protocol(_)
        ));
    }

    #[test]
    fn fingerprint_format() {
        let k: PresentedHostKey = presented_key(&key()).unwrap();
        assert_eq!(k.key_type, "ssh-ed25519");
        assert!(k.sha256.starts_with("SHA256:") && !k.sha256.ends_with('='));
    }

    /// Our fingerprint must equal `ssh-keygen -lf` output for the same key.
    #[test]
    fn fingerprint_matches_ssh_keygen() {
        let dir = std::env::temp_dir().join(format!("ossiframe-fp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("k.pub");
        std::fs::write(&f, ED25519).unwrap();
        let out = match std::process::Command::new("ssh-keygen")
            .arg("-lf")
            .arg(&f)
            .output()
        {
            Ok(o) if o.status.success() => o,
            _ => return, // ssh-keygen not available
        };
        let text = String::from_utf8_lossy(&out.stdout);
        let expected = text.split_whitespace().nth(1).unwrap();
        assert_eq!(presented_key(&key()).unwrap().sha256, expected);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn unreadable_key_reports_path_only() {
        let p = KeyPath::parse("/nonexistent/ossiframe/id").unwrap();
        assert_eq!(
            load_key(&p, None).unwrap_err(),
            ConnectError::KeyFileUnreadable("/nonexistent/ossiframe/id".into())
        );
    }

    #[test]
    fn auth_failed_message_is_generic() {
        assert_eq!(
            ConnectError::AuthFailed.to_string(),
            "authentication failed"
        );
    }
}
