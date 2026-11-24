//! Server profile management and the connection workflow, shared by every
//! Ossiframe desktop application.

use crate::session::{ConnState, Connection, SessionEvents, Sessions};
use crate::store::{AuthConfig, ServerRecord, Store, StoreError, now};
use crate::tokens::{TokenError, TokenKind, TrustTokens};
use crate::vault::{SecretVault, VaultError};
use async_trait::async_trait;
use ossiframe_core::host_key::{PinnedHostKey, PresentedHostKey};
use ossiframe_core::{CancelToken, ExecError, Hostname, Port, SecretString, ServerName, SshUser};
use ossiframe_ssh::{AuthMethod, ConnectError, ConnectOptions, ConnectTarget};
use std::future::Future;
use std::sync::Arc;

/// Opens connections. The SSH implementation is [`SshConnector`]; tests use fakes.
#[async_trait]
pub trait Connector: Send + Sync {
    async fn connect(
        &self,
        target: &ConnectTarget,
        auth: &AuthMethod,
        pinned: Option<PinnedHostKey>,
    ) -> Result<Arc<dyn Connection>, ConnectError>;
}

#[derive(Default)]
pub struct SshConnector {
    pub options: ConnectOptions,
}

#[async_trait]
impl Connector for SshConnector {
    async fn connect(
        &self,
        target: &ConnectTarget,
        auth: &AuthMethod,
        pinned: Option<PinnedHostKey>,
    ) -> Result<Arc<dyn Connection>, ConnectError> {
        let exec = ossiframe_ssh::connect(target, auth, pinned, &self.options).await?;
        Ok(Arc::new(exec))
    }
}

/// Validated profile fields for add and edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileInput {
    pub name: ServerName,
    pub host: Hostname,
    pub port: Port,
    pub username: SshUser,
    pub auth: AuthConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialKind {
    Password,
    Passphrase,
}

#[derive(Debug, Clone)]
pub struct ServerView {
    pub record: ServerRecord,
    pub pin: Option<PinnedHostKey>,
    pub state: ConnState,
}

#[derive(Debug, Clone)]
pub enum ConnectOutcome {
    /// `warning` reports a non-fatal problem, such as an old keychain entry
    /// that could not be deleted.
    Connected { warning: Option<String> },
    /// The server's key is not pinned yet. The user must compare the
    /// fingerprint and call `trust_host_key` with `token`.
    HostKeyUnknown {
        token: String,
        key: PresentedHostKey,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ServiceError {
    #[error("server not found")]
    NotFound,
    #[error("{0}")]
    Storage(String),
    #[error("{0}")]
    Keychain(String),
    #[error("{0}")]
    Connect(ConnectError),
    #[error("credential required")]
    CredentialRequired(CredentialKind),
    #[error("host key changed")]
    HostKeyMismatch {
        pinned: Box<PinnedHostKey>,
        presented: Box<PresentedHostKey>,
        replace_token: String,
    },
    #[error("{0}")]
    Token(TokenError),
    #[error("a host key is already pinned for this server")]
    AlreadyPinned,
    #[error("the typed host name does not match")]
    ConfirmationMismatch,
    #[error("not connected")]
    NotConnected,
    #[error("{0}")]
    Exec(ExecError),
}

impl From<StoreError> for ServiceError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::NotFound => ServiceError::NotFound,
            StoreError::AlreadyPinned => ServiceError::AlreadyPinned,
            StoreError::Db(m) => ServiceError::Storage(m),
        }
    }
}

impl From<VaultError> for ServiceError {
    fn from(e: VaultError) -> Self {
        ServiceError::Keychain(e.to_string())
    }
}

impl From<TokenError> for ServiceError {
    fn from(e: TokenError) -> Self {
        ServiceError::Token(e)
    }
}

pub struct ServerService {
    store: Store,
    vault: Arc<dyn SecretVault>,
    connector: Arc<dyn Connector>,
    events: Arc<dyn SessionEvents>,
    sessions: Sessions,
    tokens: TrustTokens,
}

impl ServerService {
    pub fn new(
        store: Store,
        vault: Arc<dyn SecretVault>,
        connector: Arc<dyn Connector>,
        events: Arc<dyn SessionEvents>,
    ) -> Self {
        Self {
            store,
            vault,
            connector,
            events,
            sessions: Sessions::default(),
            tokens: TrustTokens::default(),
        }
    }

    fn view(&self, record: ServerRecord) -> Result<ServerView, ServiceError> {
        let pin = self.store.get_pin(&record.id)?;
        let state = if self.sessions.is_connected(&record.id, &*self.events) {
            ConnState::Connected
        } else {
            ConnState::Disconnected
        };
        Ok(ServerView { record, pin, state })
    }

    pub fn list(&self) -> Result<Vec<ServerView>, ServiceError> {
        self.store
            .list()?
            .into_iter()
            .map(|r| self.view(r))
            .collect()
    }

    pub fn get(&self, id: &str) -> Result<ServerView, ServiceError> {
        let r = self.store.get(id)?;
        self.view(r)
    }

    /// Adds a profile. Secrets are never accepted here; they are saved only
    /// after a successful connection when the user opts in.
    pub fn add(&self, input: ProfileInput) -> Result<ServerView, ServiceError> {
        let ts = now();
        let record = ServerRecord {
            id: uuid::Uuid::new_v4().to_string(),
            name: input.name,
            host: input.host,
            port: input.port,
            username: input.username,
            auth: input.auth,
            secret_ref: None,
            created_at: ts.clone(),
            updated_at: ts,
        };
        self.store.insert(&record)?;
        self.view(record)
    }

    /// Edits a profile.
    /// - Name only: pin and session are kept.
    /// - Host or port: session closed, pending approvals invalidated, pin
    ///   cleared and saved secret deleted.
    /// - User, auth kind or key path: saved secret deleted, session closed.
    ///
    /// A secret saved for one endpoint and login is never sent to another.
    pub async fn update(&self, id: &str, input: ProfileInput) -> Result<ServerView, ServiceError> {
        self.sessions.cancel_op(id);
        let guard = self.sessions.guard(id);
        let _g = guard.lock().await;
        let old = self.store.get(id)?;
        let endpoint_changed = old.host != input.host || old.port != input.port;
        let auth_changed = old.auth != input.auth || old.username != input.username;

        let mut rec = old.clone();
        rec.name = input.name;
        rec.host = input.host;
        rec.port = input.port;
        rec.username = input.username;
        rec.auth = input.auth;
        rec.updated_at = now();
        let obsolete_secret = if auth_changed || endpoint_changed {
            rec.secret_ref.take()
        } else {
            None
        };

        self.store.update(&rec, endpoint_changed)?;
        if endpoint_changed {
            self.tokens.invalidate_server(id);
        }
        if endpoint_changed || auth_changed {
            self.sessions.close(id).await;
            self.events.state_changed(id, ConnState::Disconnected);
        }
        if let Some(r) = obsolete_secret {
            // The profile no longer references it, so a failure here leaves an
            // orphaned keychain entry, never a dangling reference.
            self.vault.delete(&r).map_err(|e| {
                ServiceError::Keychain(format!(
                    "profile updated, but the old saved secret could not be deleted: {e}"
                ))
            })?;
        }
        self.view(rec)
    }

    /// Removes local state only: session, pending approvals, saved secret,
    /// profile and pin. The remote server is never contacted.
    pub async fn remove(&self, id: &str) -> Result<(), ServiceError> {
        self.sessions.cancel_op(id);
        let guard = self.sessions.guard(id);
        let _g = guard.lock().await;
        let rec = self.store.get(id)?;
        self.sessions.close(id).await;
        self.tokens.invalidate_server(id);
        if let Some(r) = &rec.secret_ref {
            self.vault.delete(r)?;
        }
        self.store.delete(id)?;
        self.sessions.forget(id);
        Ok(())
    }

    /// Connects to a profile.
    ///
    /// `credential` is a password or key passphrase entered for this attempt
    /// only. With `save`, it is written to the keychain after authentication
    /// succeeds. Without a credential, a saved one is used if present.
    pub async fn connect(
        &self,
        id: &str,
        credential: Option<SecretString>,
        save: bool,
    ) -> Result<ConnectOutcome, ServiceError> {
        let guard = self.sessions.guard(id);
        let _g = guard.lock().await;
        if self.sessions.is_connected(id, &*self.events) {
            return Ok(ConnectOutcome::Connected { warning: None });
        }
        let rec = self.store.get(id)?;
        let pin = self.store.get_pin(id)?;

        let entered = credential.filter(|c| !c.is_empty());
        let secret = match (&entered, &rec.secret_ref) {
            (Some(c), _) => Some(c.clone()),
            (None, Some(r)) => match self.vault.load(r)? {
                Some(s) => Some(s),
                None => {
                    // The keychain entry is gone; drop the stale reference.
                    self.store.set_secret_ref(id, None)?;
                    None
                }
            },
            (None, None) => None,
        };
        let auth = match &rec.auth {
            AuthConfig::KeyFile(path) => AuthMethod::KeyFile {
                path: path.clone(),
                passphrase: secret,
            },
            AuthConfig::Password => {
                AuthMethod::Password(secret.unwrap_or_else(|| SecretString::new(String::new())))
            }
        };
        let target = ConnectTarget {
            host: rec.host.clone(),
            port: rec.port,
            user: rec.username.clone(),
        };

        self.events.state_changed(id, ConnState::Connecting);
        let result = self.connector.connect(&target, &auth, pin).await;
        drop(auth);
        let conn = match result {
            Ok(c) => c,
            Err(e) => {
                let state = if matches!(e, ConnectError::HostKeyUnknown(_)) {
                    ConnState::Disconnected
                } else {
                    ConnState::Error
                };
                self.events.state_changed(id, state);
                return self.connect_failure(&rec, e);
            }
        };

        let mut warning = None;
        if save && let Some(c) = &entered {
            let entry = uuid::Uuid::new_v4().to_string();
            if let Err(e) = self.vault.save(&entry, c) {
                conn.close().await;
                self.events.state_changed(id, ConnState::Error);
                return Err(ServiceError::Keychain(format!(
                    "authenticated, but saving to the keychain failed, so nothing was saved and the session was closed. Connect without saving or unlock the keychain. ({e})"
                )));
            }
            if let Err(e) = self.store.set_secret_ref(id, Some(&entry)) {
                // Compensate: the profile keeps its previous reference.
                let cleanup = self.vault.delete(&entry);
                conn.close().await;
                self.events.state_changed(id, ConnState::Error);
                let detail = match cleanup {
                    Ok(()) => "the new keychain entry was removed".to_owned(),
                    Err(v) => format!("the new keychain entry could not be removed ({v})"),
                };
                return Err(ServiceError::Storage(format!(
                    "authenticated, but the saved secret could not be recorded locally; {detail} and the session was closed. Try again or connect without saving. ({e})"
                )));
            }
            if let Some(old) = &rec.secret_ref
                && let Err(e) = self.vault.delete(old)
            {
                warning = Some(format!(
                    "The new secret was saved, but the previous keychain entry could not be deleted. You can remove it in Keychain Access (service {}). ({e})",
                    self.keychain_service_hint()
                ));
            }
        }

        self.sessions.insert(id, conn);
        self.events.state_changed(id, ConnState::Connected);
        Ok(ConnectOutcome::Connected { warning })
    }

    fn connect_failure(
        &self,
        rec: &ServerRecord,
        e: ConnectError,
    ) -> Result<ConnectOutcome, ServiceError> {
        match e {
            ConnectError::HostKeyUnknown(key) => {
                let token = self.tokens.issue(
                    &rec.id,
                    &rec.host,
                    rec.port,
                    (*key).clone(),
                    TokenKind::Trust,
                );
                Ok(ConnectOutcome::HostKeyUnknown { token, key: *key })
            }
            ConnectError::HostKeyMismatch { pinned, presented } => {
                let replace_token = self.tokens.issue(
                    &rec.id,
                    &rec.host,
                    rec.port,
                    (*presented).clone(),
                    TokenKind::Replace,
                );
                Err(ServiceError::HostKeyMismatch {
                    pinned,
                    presented,
                    replace_token,
                })
            }
            ConnectError::PasswordRequired => {
                Err(ServiceError::CredentialRequired(CredentialKind::Password))
            }
            ConnectError::PassphraseRequired => {
                Err(ServiceError::CredentialRequired(CredentialKind::Passphrase))
            }
            other => Err(ServiceError::Connect(other)),
        }
    }

    pub async fn disconnect(&self, id: &str) -> Result<(), ServiceError> {
        self.sessions.cancel_op(id);
        let guard = self.sessions.guard(id);
        let _g = guard.lock().await;
        self.store.get(id)?;
        self.sessions.close(id).await;
        self.events.state_changed(id, ConnState::Disconnected);
        Ok(())
    }

    /// Pins the key recorded in a trust token issued by `connect`.
    pub async fn trust_host_key(&self, id: &str, token: &str) -> Result<(), ServiceError> {
        let guard = self.sessions.guard(id);
        let _g = guard.lock().await;
        let rec = self.store.get(id)?;
        let pending = self.tokens.take(token, id, TokenKind::Trust)?;
        if !pending.matches_endpoint(&rec.host, rec.port) {
            return Err(TokenError::Stale.into());
        }
        self.store.pin_host_key(id, &pending.key)?;
        Ok(())
    }

    /// Replaces a pinned key after a mismatch. `confirm_host` must equal the
    /// profile's host. Closes any session and invalidates pending approvals.
    pub async fn replace_host_key(
        &self,
        id: &str,
        token: &str,
        confirm_host: &str,
    ) -> Result<(), ServiceError> {
        self.sessions.cancel_op(id);
        let guard = self.sessions.guard(id);
        let _g = guard.lock().await;
        let rec = self.store.get(id)?;
        if !confirm_host.trim().eq_ignore_ascii_case(rec.host.as_str()) {
            return Err(ServiceError::ConfirmationMismatch);
        }
        let pending = self.tokens.take(token, id, TokenKind::Replace)?;
        if !pending.matches_endpoint(&rec.host, rec.port) {
            return Err(TokenError::Stale.into());
        }
        self.store.replace_host_key(id, &pending.key)?;
        self.tokens.invalidate_server(id);
        self.sessions.close(id).await;
        self.events.state_changed(id, ConnState::Disconnected);
        Ok(())
    }

    fn keychain_service_hint(&self) -> &str {
        self.vault.service_name()
    }

    /// Cancels the running operation for a profile, if any.
    pub fn cancel_operation(&self, id: &str) {
        self.sessions.cancel_op(id);
    }

    /// Runs read-only work with the profile's live connection. Returns
    /// `NotConnected` if there is none, and `ConnectionLost` or `Cancelled`
    /// if the connection dropped or the user cancelled during the work.
    pub async fn with_connection<T, F, Fut>(&self, id: &str, f: F) -> Result<T, ServiceError>
    where
        F: FnOnce(Arc<dyn Connection>, CancelToken) -> Fut + Send,
        Fut: Future<Output = T> + Send,
    {
        let guard = self.sessions.guard(id);
        let _g = guard.lock().await;
        self.store.get(id)?;
        let conn = self
            .sessions
            .get(id, &*self.events)
            .ok_or(ServiceError::NotConnected)?;
        let cancel = self.sessions.begin_op(id);
        let out = f(conn.clone(), cancel.clone()).await;
        self.sessions.end_op(id);
        if conn.is_closed() {
            self.sessions.get(id, &*self.events);
            return Err(ServiceError::Exec(ExecError::ConnectionLost));
        }
        if cancel.is_cancelled() {
            return Err(ServiceError::Exec(ExecError::Cancelled));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::tests::{FakeConn, RecordingEvents};
    use crate::vault::MemoryVault;
    use std::sync::Mutex;
    use std::sync::atomic::Ordering;

    /// Scripted connector: pops the next result and records what it saw.
    #[derive(Default)]
    struct FakeConnector {
        results: Mutex<Vec<Result<(), ConnectError>>>,
        seen_secrets: Mutex<Vec<Option<String>>>,
        conns: Mutex<Vec<Arc<FakeConn>>>,
    }

    impl FakeConnector {
        fn push(&self, r: Result<(), ConnectError>) {
            self.results.lock().unwrap().insert(0, r);
        }
    }

    #[async_trait]
    impl Connector for FakeConnector {
        async fn connect(
            &self,
            _: &ConnectTarget,
            auth: &AuthMethod,
            _: Option<PinnedHostKey>,
        ) -> Result<Arc<dyn Connection>, ConnectError> {
            let secret = match auth {
                AuthMethod::Password(p) => Some(p.expose().to_owned()).filter(|s| !s.is_empty()),
                AuthMethod::KeyFile { passphrase, .. } => {
                    passphrase.as_ref().map(|p| p.expose().to_owned())
                }
            };
            self.seen_secrets.lock().unwrap().push(secret);
            self.results.lock().unwrap().pop().unwrap_or(Ok(()))?;
            let c = Arc::new(FakeConn::default());
            self.conns.lock().unwrap().push(c.clone());
            Ok(c)
        }
    }

    fn key(b: u8) -> PresentedHostKey {
        PresentedHostKey {
            key_type: "ssh-ed25519".into(),
            public_key: vec![b; 32],
            sha256: format!("SHA256:{b}"),
        }
    }

    fn input(host: &str, auth: AuthConfig) -> ProfileInput {
        ProfileInput {
            name: ServerName::parse("web").unwrap(),
            host: Hostname::parse(host).unwrap(),
            port: Port::SSH_DEFAULT,
            username: SshUser::parse("root").unwrap(),
            auth,
        }
    }

    fn setup() -> (ServerService, Arc<MemoryVault>, Arc<FakeConnector>) {
        let vault = Arc::new(MemoryVault::default());
        let conn = Arc::new(FakeConnector::default());
        let svc = ServerService::new(
            Store::open_in_memory().unwrap(),
            vault.clone(),
            conn.clone(),
            Arc::new(RecordingEvents::default()),
        );
        (svc, vault, conn)
    }

    fn secret(s: &str) -> Option<SecretString> {
        Some(SecretString::new(s.into()))
    }

    #[tokio::test]
    async fn trust_flow_pins_the_observed_key_only() {
        let (svc, _, c) = setup();
        let id = svc
            .add(input("example.com", AuthConfig::Password))
            .unwrap()
            .record
            .id;
        c.push(Err(ConnectError::HostKeyUnknown(Box::new(key(1)))));
        let token = match svc.connect(&id, None, false).await.unwrap() {
            ConnectOutcome::HostKeyUnknown { token, key: k } => {
                assert_eq!(k, key(1));
                token
            }
            o => panic!("{o:?}"),
        };
        assert!(
            c.seen_secrets.lock().unwrap()[0].is_none(),
            "no credential before trust"
        );
        svc.trust_host_key(&id, &token).await.unwrap();
        assert_eq!(svc.get(&id).unwrap().pin.unwrap().public_key, vec![1; 32]);
        assert!(matches!(
            svc.trust_host_key(&id, &token).await,
            Err(ServiceError::Token(TokenError::Unknown))
        ));
    }

    #[tokio::test]
    async fn token_for_another_server_is_rejected() {
        let (svc, _, c) = setup();
        let a = svc
            .add(input("a.example.com", AuthConfig::Password))
            .unwrap()
            .record
            .id;
        let b = svc
            .add(input("b.example.com", AuthConfig::Password))
            .unwrap()
            .record
            .id;
        c.push(Err(ConnectError::HostKeyUnknown(Box::new(key(1)))));
        let ConnectOutcome::HostKeyUnknown { token, .. } =
            svc.connect(&a, None, false).await.unwrap()
        else {
            panic!()
        };
        assert!(matches!(
            svc.trust_host_key(&b, &token).await,
            Err(ServiceError::Token(TokenError::WrongServer))
        ));
        assert!(svc.get(&b).unwrap().pin.is_none());
    }

    #[tokio::test]
    async fn host_edit_invalidates_tokens_and_clears_pin() {
        let (svc, _, c) = setup();
        let id = svc
            .add(input("example.com", AuthConfig::Password))
            .unwrap()
            .record
            .id;
        svc.store.pin_host_key(&id, &key(1)).unwrap();
        c.push(Err(ConnectError::HostKeyMismatch {
            pinned: Box::new(key(1).into()),
            presented: Box::new(key(2)),
        }));
        let Err(ServiceError::HostKeyMismatch { replace_token, .. }) =
            svc.connect(&id, None, false).await
        else {
            panic!()
        };

        let v = svc
            .update(&id, input("new.example.com", AuthConfig::Password))
            .await
            .unwrap();
        assert!(v.pin.is_none(), "pin cleared on host change");
        assert!(
            v.record.secret_ref.is_none(),
            "secret not carried to new host"
        );
        assert!(matches!(
            svc.replace_host_key(&id, &replace_token, "new.example.com")
                .await,
            Err(ServiceError::Token(TokenError::Unknown))
        ));
    }

    #[tokio::test]
    async fn replace_requires_typed_host_and_closes_session() {
        let (svc, _, c) = setup();
        let id = svc
            .add(input("example.com", AuthConfig::Password))
            .unwrap()
            .record
            .id;
        svc.store.pin_host_key(&id, &key(1)).unwrap();
        c.push(Err(ConnectError::HostKeyMismatch {
            pinned: Box::new(key(1).into()),
            presented: Box::new(key(2)),
        }));
        let Err(ServiceError::HostKeyMismatch { replace_token, .. }) =
            svc.connect(&id, secret("pw"), false).await
        else {
            panic!()
        };

        assert_eq!(
            svc.replace_host_key(&id, &replace_token, "wrong.com").await,
            Err(ServiceError::ConfirmationMismatch)
        );
        assert_eq!(
            svc.get(&id).unwrap().pin.unwrap().public_key,
            vec![1; 32],
            "pin kept"
        );
        svc.replace_host_key(&id, &replace_token, "EXAMPLE.com")
            .await
            .unwrap();
        assert_eq!(svc.get(&id).unwrap().pin.unwrap().public_key, vec![2; 32]);
    }

    #[tokio::test]
    async fn credentials_saved_only_after_success_and_on_opt_in() {
        let (svc, vault, c) = setup();
        let id = svc
            .add(input("example.com", AuthConfig::Password))
            .unwrap()
            .record
            .id;

        c.push(Err(ConnectError::PasswordRequired));
        assert_eq!(
            svc.connect(&id, None, false).await.unwrap_err(),
            ServiceError::CredentialRequired(CredentialKind::Password)
        );

        c.push(Err(ConnectError::AuthFailed));
        assert!(svc.connect(&id, secret("wrong"), true).await.is_err());
        assert!(vault.is_empty(), "nothing saved after failed auth");

        svc.connect(&id, secret("right"), false).await.unwrap();
        assert!(vault.is_empty(), "not saved without opt-in");
        svc.disconnect(&id).await.unwrap();

        svc.connect(&id, secret("right"), true).await.unwrap();
        assert_eq!(vault.len(), 1);
        svc.disconnect(&id).await.unwrap();

        // Reconnect (e.g. after restart) uses the saved secret.
        svc.connect(&id, None, false).await.unwrap();
        assert_eq!(
            c.seen_secrets.lock().unwrap().last().unwrap().as_deref(),
            Some("right")
        );
    }

    #[tokio::test]
    async fn keychain_failure_saves_nothing_and_closes_session() {
        let (svc, vault, c) = setup();
        let id = svc
            .add(input("example.com", AuthConfig::Password))
            .unwrap()
            .record
            .id;
        vault.fail.store(true, Ordering::SeqCst);
        assert!(matches!(
            svc.connect(&id, secret("pw"), true).await,
            Err(ServiceError::Keychain(_))
        ));
        vault.fail.store(false, Ordering::SeqCst);
        assert!(vault.is_empty());
        assert!(svc.get(&id).unwrap().record.secret_ref.is_none());
        assert!(c.conns.lock().unwrap()[0].is_closed());
        assert_eq!(svc.get(&id).unwrap().state, ConnState::Disconnected);
    }

    #[tokio::test]
    async fn edit_matrix() {
        let (svc, vault, _) = setup();
        let id = svc
            .add(input("example.com", AuthConfig::Password))
            .unwrap()
            .record
            .id;
        svc.store.pin_host_key(&id, &key(1)).unwrap();
        svc.connect(&id, secret("pw"), true).await.unwrap();
        assert_eq!(vault.len(), 1);

        // Name only: pin, secret and session kept.
        let mut i = input("example.com", AuthConfig::Password);
        i.name = ServerName::parse("renamed").unwrap();
        let v = svc.update(&id, i.clone()).await.unwrap();
        assert!(v.pin.is_some() && v.record.secret_ref.is_some());
        assert_eq!(v.state, ConnState::Connected);

        // Auth change: secret deleted, session closed, pin kept.
        let key_path = ossiframe_core::KeyPath::parse("/tmp/id_ed25519").unwrap();
        let mut i2 = i.clone();
        i2.auth = AuthConfig::KeyFile(key_path);
        let v = svc.update(&id, i2).await.unwrap();
        assert!(v.record.secret_ref.is_none() && v.pin.is_some());
        assert!(vault.is_empty());
        assert_eq!(v.state, ConnState::Disconnected);
    }

    #[tokio::test]
    async fn endpoint_change_never_reuses_saved_secret() {
        let (svc, vault, c) = setup();
        let id = svc
            .add(input("example.com", AuthConfig::Password))
            .unwrap()
            .record
            .id;
        svc.connect(&id, secret("old-host-pw"), true).await.unwrap();
        assert_eq!(vault.len(), 1);

        let mut i = input("example.com", AuthConfig::Password);
        i.port = Port::new(2222).unwrap();
        let v = svc.update(&id, i).await.unwrap();
        assert!(v.record.secret_ref.is_none());
        assert!(vault.is_empty(), "old secret deleted");
        assert_eq!(v.state, ConnState::Disconnected);

        c.push(Err(ConnectError::PasswordRequired));
        assert_eq!(
            svc.connect(&id, None, false).await.unwrap_err(),
            ServiceError::CredentialRequired(CredentialKind::Password)
        );
        assert!(
            c.seen_secrets.lock().unwrap().last().unwrap().is_none(),
            "nothing sent to the new endpoint"
        );
    }

    #[tokio::test]
    async fn remove_is_local_only() {
        let (svc, vault, c) = setup();
        let id = svc
            .add(input("example.com", AuthConfig::Password))
            .unwrap()
            .record
            .id;
        svc.connect(&id, secret("pw"), true).await.unwrap();
        let conn = c.conns.lock().unwrap()[0].clone();
        svc.remove(&id).await.unwrap();
        assert!(conn.is_closed());
        assert_eq!(
            conn.runs.load(Ordering::SeqCst),
            0,
            "no remote command on removal"
        );
        assert!(vault.is_empty());
        assert_eq!(svc.get(&id).unwrap_err(), ServiceError::NotFound);
    }

    #[tokio::test]
    async fn remove_keeps_profile_if_keychain_fails() {
        let (svc, vault, _) = setup();
        let id = svc
            .add(input("example.com", AuthConfig::Password))
            .unwrap()
            .record
            .id;
        svc.connect(&id, secret("pw"), true).await.unwrap();
        vault.fail.store(true, Ordering::SeqCst);
        assert!(matches!(
            svc.remove(&id).await,
            Err(ServiceError::Keychain(_))
        ));
        assert!(
            svc.get(&id).unwrap().record.secret_ref.is_some(),
            "reference still valid"
        );
    }

    #[tokio::test]
    async fn with_connection_reports_lost_and_not_connected() {
        let (svc, _, c) = setup();
        let id = svc
            .add(input("example.com", AuthConfig::Password))
            .unwrap()
            .record
            .id;
        assert_eq!(
            svc.with_connection(&id, |_, _| async {}).await.unwrap_err(),
            ServiceError::NotConnected
        );
        svc.connect(&id, secret("pw"), false).await.unwrap();
        svc.with_connection(&id, |_, _| async {}).await.unwrap();
        let conn = c.conns.lock().unwrap()[0].clone();
        let r = svc
            .with_connection(&id, move |_, _| async move {
                conn.closed.store(true, Ordering::SeqCst)
            })
            .await;
        assert_eq!(
            r.unwrap_err(),
            ServiceError::Exec(ExecError::ConnectionLost)
        );
        assert_eq!(svc.get(&id).unwrap().state, ConnState::Disconnected);
    }
}
