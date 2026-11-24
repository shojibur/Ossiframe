//! Short-lived, single-use host key trust tokens.
//!
//! The backend issues a token when it observes an unknown or changed host
//! key. Trusting uses the key stored in the token, never a key sent by the
//! frontend. A token is bound to the server ID, endpoint and observed key.

use ossiframe_core::host_key::PresentedHostKey;
use ossiframe_core::{Hostname, Port};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const TOKEN_TTL: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// First-time trust of an unknown key.
    Trust,
    /// Replacement of a pinned key after a mismatch.
    Replace,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TokenError {
    #[error("host key approval is unknown or was already used")]
    Unknown,
    #[error("host key approval expired; connect again")]
    Expired,
    #[error("host key approval does not belong to this server")]
    WrongServer,
    #[error("host key approval is for a different action")]
    WrongKind,
    #[error("server address changed since the host key was observed; connect again")]
    Stale,
}

#[derive(Debug, Clone)]
pub struct PendingKey {
    pub server_id: String,
    pub host: Hostname,
    pub port: Port,
    pub key: PresentedHostKey,
    pub kind: TokenKind,
    expires: Instant,
}

impl PendingKey {
    /// True when the token was issued for this exact endpoint.
    pub fn matches_endpoint(&self, host: &Hostname, port: Port) -> bool {
        &self.host == host && self.port == port
    }
}

pub struct TrustTokens {
    ttl: Duration,
    pending: Mutex<HashMap<String, PendingKey>>,
}

impl Default for TrustTokens {
    fn default() -> Self {
        Self::new(TOKEN_TTL)
    }
}

impl TrustTokens {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            pending: Mutex::new(HashMap::new()),
        }
    }

    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<String, PendingKey>> {
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn issue(
        &self,
        server_id: &str,
        host: &Hostname,
        port: Port,
        key: PresentedHostKey,
        kind: TokenKind,
    ) -> String {
        self.issue_at(server_id, host, port, key, kind, Instant::now())
    }

    pub(crate) fn issue_at(
        &self,
        server_id: &str,
        host: &Hostname,
        port: Port,
        key: PresentedHostKey,
        kind: TokenKind,
        now: Instant,
    ) -> String {
        let token = uuid::Uuid::new_v4().to_string();
        let mut map = self.map();
        map.retain(|_, p| p.expires > now);
        // Only the newest pending approval per server stays valid.
        map.retain(|_, p| p.server_id != server_id);
        map.insert(
            token.clone(),
            PendingKey {
                server_id: server_id.into(),
                host: host.clone(),
                port,
                key,
                kind,
                expires: now + self.ttl,
            },
        );
        token
    }

    /// Consumes the token. It cannot be used again, even if checks fail.
    pub fn take(
        &self,
        token: &str,
        server_id: &str,
        kind: TokenKind,
    ) -> Result<PendingKey, TokenError> {
        self.take_at(token, server_id, kind, Instant::now())
    }

    pub(crate) fn take_at(
        &self,
        token: &str,
        server_id: &str,
        kind: TokenKind,
        now: Instant,
    ) -> Result<PendingKey, TokenError> {
        let p = self.map().remove(token).ok_or(TokenError::Unknown)?;
        if p.expires <= now {
            return Err(TokenError::Expired);
        }
        if p.server_id != server_id {
            return Err(TokenError::WrongServer);
        }
        if p.kind != kind {
            return Err(TokenError::WrongKind);
        }
        Ok(p)
    }

    pub fn invalidate_server(&self, server_id: &str) {
        self.map().retain(|_, p| p.server_id != server_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> PresentedHostKey {
        PresentedHostKey {
            key_type: "ssh-ed25519".into(),
            public_key: vec![7; 32],
            sha256: "SHA256:x".into(),
        }
    }
    fn host() -> Hostname {
        Hostname::parse("example.com").unwrap()
    }

    #[test]
    fn single_use_expiry_binding() {
        let t = TrustTokens::new(Duration::from_secs(60));
        let now = Instant::now();
        let a = t.issue_at(
            "s1",
            &host(),
            Port::SSH_DEFAULT,
            key(),
            TokenKind::Trust,
            now,
        );
        let p = t.take_at(&a, "s1", TokenKind::Trust, now).unwrap();
        assert_eq!(p.key, key());
        assert_eq!(
            t.take_at(&a, "s1", TokenKind::Trust, now).unwrap_err(),
            TokenError::Unknown,
            "single use"
        );

        let b = t.issue_at(
            "s1",
            &host(),
            Port::SSH_DEFAULT,
            key(),
            TokenKind::Trust,
            now,
        );
        assert_eq!(
            t.take_at(&b, "s1", TokenKind::Trust, now + Duration::from_secs(61))
                .unwrap_err(),
            TokenError::Expired
        );

        let c = t.issue_at(
            "s1",
            &host(),
            Port::SSH_DEFAULT,
            key(),
            TokenKind::Trust,
            now,
        );
        assert_eq!(
            t.take_at(&c, "s2", TokenKind::Trust, now).unwrap_err(),
            TokenError::WrongServer
        );
        assert_eq!(
            t.take_at(&c, "s1", TokenKind::Trust, now).unwrap_err(),
            TokenError::Unknown,
            "burned by misuse"
        );

        let d = t.issue_at(
            "s1",
            &host(),
            Port::SSH_DEFAULT,
            key(),
            TokenKind::Trust,
            now,
        );
        assert_eq!(
            t.take_at(&d, "s1", TokenKind::Replace, now).unwrap_err(),
            TokenError::WrongKind
        );

        assert_eq!(
            t.take_at("made-up", "s1", TokenKind::Trust, now)
                .unwrap_err(),
            TokenError::Unknown
        );
    }

    #[test]
    fn invalidation_and_newest_wins() {
        let t = TrustTokens::default();
        let a = t.issue("s1", &host(), Port::SSH_DEFAULT, key(), TokenKind::Trust);
        let b = t.issue("s1", &host(), Port::SSH_DEFAULT, key(), TokenKind::Trust);
        assert_eq!(
            t.take(&a, "s1", TokenKind::Trust).unwrap_err(),
            TokenError::Unknown
        );
        t.invalidate_server("s1");
        assert_eq!(
            t.take(&b, "s1", TokenKind::Trust).unwrap_err(),
            TokenError::Unknown
        );
    }

    #[test]
    fn endpoint_binding() {
        let t = TrustTokens::default();
        let a = t.issue("s1", &host(), Port::SSH_DEFAULT, key(), TokenKind::Trust);
        let p = t.take(&a, "s1", TokenKind::Trust).unwrap();
        assert!(p.matches_endpoint(&host(), Port::SSH_DEFAULT));
        assert!(!p.matches_endpoint(&host(), Port::new(2222).unwrap()));
        assert!(!p.matches_endpoint(&Hostname::parse("other.com").unwrap(), Port::SSH_DEFAULT));
    }
}
