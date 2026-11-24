//! Host key pinning decisions. Fingerprints are produced by the SSH library;
//! this module only compares keys.

use serde::Serialize;

/// The key a server presented during the handshake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresentedHostKey {
    pub key_type: String,
    #[serde(skip)]
    pub public_key: Vec<u8>,
    /// `SHA256:<base64 without padding>`
    pub sha256: String,
}

/// A key the user trusted earlier for a server endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PinnedHostKey {
    pub key_type: String,
    #[serde(skip)]
    pub public_key: Vec<u8>,
    pub sha256: String,
}

impl From<PresentedHostKey> for PinnedHostKey {
    fn from(k: PresentedHostKey) -> Self {
        Self {
            key_type: k.key_type,
            public_key: k.public_key,
            sha256: k.sha256,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyDecision {
    Trusted,
    Unknown(PresentedHostKey),
    Mismatch {
        pinned: PinnedHostKey,
        presented: PresentedHostKey,
    },
}

/// `Trusted` only when a pin exists and both key type and key bytes match.
pub fn decide(pinned: Option<&PinnedHostKey>, presented: &PresentedHostKey) -> HostKeyDecision {
    match pinned {
        None => HostKeyDecision::Unknown(presented.clone()),
        Some(p) if p.key_type == presented.key_type && p.public_key == presented.public_key => {
            HostKeyDecision::Trusted
        }
        Some(p) => HostKeyDecision::Mismatch {
            pinned: p.clone(),
            presented: presented.clone(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(t: &str, b: &[u8]) -> PresentedHostKey {
        PresentedHostKey {
            key_type: t.into(),
            public_key: b.to_vec(),
            sha256: format!("SHA256:{t}{}", b.len()),
        }
    }

    #[test]
    fn decisions() {
        let k = key("ssh-ed25519", b"abc");
        assert_eq!(decide(None, &k), HostKeyDecision::Unknown(k.clone()));
        let pin: PinnedHostKey = k.clone().into();
        assert_eq!(decide(Some(&pin), &k), HostKeyDecision::Trusted);

        let other_bytes = key("ssh-ed25519", b"abd");
        assert!(matches!(
            decide(Some(&pin), &other_bytes),
            HostKeyDecision::Mismatch { .. }
        ));

        let other_type = key("ecdsa-sha2-nistp256", b"abc");
        match decide(Some(&pin), &other_type) {
            HostKeyDecision::Mismatch { pinned, presented } => {
                assert_eq!(pinned, pin);
                assert_eq!(presented, other_type);
            }
            d => panic!("expected mismatch, got {d:?}"),
        }
    }

    #[test]
    fn matching_fingerprint_text_alone_is_not_trust() {
        let pin: PinnedHostKey = key("ssh-ed25519", b"abc").into();
        let mut forged = key("ssh-ed25519", b"zzz");
        forged.sha256 = pin.sha256.clone();
        assert!(matches!(
            decide(Some(&pin), &forged),
            HostKeyDecision::Mismatch { .. }
        ));
    }
}
