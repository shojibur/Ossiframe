//! OS keychain access. This is the only module that touches stored secrets.
//! There is no plaintext fallback: if the keychain fails, the caller gets an
//! error and nothing is written anywhere else.

use ossiframe_core::SecretString;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("keychain unavailable or locked: {0}")]
pub struct VaultError(pub String);

pub trait SecretVault: Send + Sync {
    fn save(&self, entry_id: &str, secret: &SecretString) -> Result<(), VaultError>;
    fn load(&self, entry_id: &str) -> Result<Option<SecretString>, VaultError>;
    /// Deleting an entry that does not exist succeeds.
    fn delete(&self, entry_id: &str) -> Result<(), VaultError>;
    /// Keychain service name shown to users for manual cleanup.
    fn service_name(&self) -> &str {
        "ossiframe"
    }
}

/// Native OS keychain through `keyring-core`. macOS uses the login keychain.
/// Other platforms report the keychain as unavailable in this release.
pub struct KeychainVault {
    service: String,
    store: Result<Arc<keyring_core::CredentialStore>, VaultError>,
}

impl KeychainVault {
    /// `service` names the application, for example `dev.ossiframe.doctor`.
    pub fn new(service: &str) -> Self {
        Self {
            service: service.to_owned(),
            store: platform_store(),
        }
    }

    fn entry(&self, entry_id: &str) -> Result<keyring_core::Entry, VaultError> {
        let store = self.store.as_ref().map_err(Clone::clone)?;
        keyring_core::Entry::new_in_store(store, &self.service, entry_id).map_err(map_err)
    }
}

#[cfg(target_os = "macos")]
fn platform_store() -> Result<Arc<keyring_core::CredentialStore>, VaultError> {
    let store = apple_native_keyring_store::keychain::Store::new().map_err(map_err)?;
    Ok(store)
}

#[cfg(not(target_os = "macos"))]
fn platform_store() -> Result<Arc<keyring_core::CredentialStore>, VaultError> {
    Err(VaultError(
        "no supported keychain on this platform yet".into(),
    ))
}

fn map_err(e: keyring_core::Error) -> VaultError {
    // keyring errors do not contain secret values; keep them short anyway.
    VaultError(e.to_string().chars().take(200).collect())
}

impl SecretVault for KeychainVault {
    fn save(&self, entry_id: &str, secret: &SecretString) -> Result<(), VaultError> {
        self.entry(entry_id)?
            .set_password(secret.expose())
            .map_err(map_err)
    }

    fn load(&self, entry_id: &str) -> Result<Option<SecretString>, VaultError> {
        match self.entry(entry_id)?.get_password() {
            Ok(s) => Ok(Some(SecretString::new(s))),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(e) => Err(map_err(e)),
        }
    }

    fn delete(&self, entry_id: &str) -> Result<(), VaultError> {
        match self.entry(entry_id)?.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(e) => Err(map_err(e)),
        }
    }

    fn service_name(&self) -> &str {
        &self.service
    }
}

/// In-memory vault for tests and examples. Not for production use.
#[derive(Default)]
pub struct MemoryVault {
    entries: Mutex<HashMap<String, SecretString>>,
    /// When set, every call fails as if the keychain were locked.
    pub fail: AtomicBool,
}

impl MemoryVault {
    fn check(&self) -> Result<(), VaultError> {
        if self.fail.load(Ordering::SeqCst) {
            Err(VaultError("locked".into()))
        } else {
            Ok(())
        }
    }

    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<String, SecretString>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn len(&self) -> usize {
        self.map().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl SecretVault for MemoryVault {
    fn save(&self, id: &str, secret: &SecretString) -> Result<(), VaultError> {
        self.check()?;
        self.map().insert(id.into(), secret.clone());
        Ok(())
    }
    fn load(&self, id: &str) -> Result<Option<SecretString>, VaultError> {
        self.check()?;
        Ok(self.map().get(id).cloned())
    }
    fn delete(&self, id: &str) -> Result<(), VaultError> {
        self.check()?;
        self.map().remove(id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_vault_round_trip_and_failure() {
        let v = MemoryVault::default();
        v.save("e1", &SecretString::new("pw".into())).unwrap();
        assert_eq!(v.load("e1").unwrap().unwrap().expose(), "pw");
        v.delete("e1").unwrap();
        v.delete("e1").unwrap();
        assert!(v.load("e1").unwrap().is_none());
        v.fail.store(true, Ordering::SeqCst);
        assert!(v.save("e2", &SecretString::new("x".into())).is_err());
        v.fail.store(false, Ordering::SeqCst);
        assert!(v.is_empty(), "nothing stored after a failed save");
    }
}
