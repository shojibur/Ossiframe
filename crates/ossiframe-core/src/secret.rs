//! A string secret that is zeroized on drop and never printed or serialized.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use zeroize::Zeroizing;

/// Holds a password or passphrase in memory only.
///
/// `Debug`, `Display` and `Serialize` all produce `<redacted>`. The value is
/// only reachable through [`SecretString::expose`].
#[derive(Clone)]
pub struct SecretString(Zeroizing<String>);

impl SecretString {
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }

    /// Returns the secret. Callers must not log or persist the result.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl Serialize for SecretString {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("<redacted>")
    }
}

impl<'de> Deserialize<'de> for SecretString {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::new(String::deserialize(deserializer)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_prints_or_serializes_the_value() {
        let s = SecretString::new("hunter2-secret".into());
        assert_eq!(format!("{s:?}"), "<redacted>");
        assert_eq!(format!("{s}"), "<redacted>");
        assert_eq!(serde_json::to_string(&s).unwrap(), "\"<redacted>\"");
        assert_eq!(s.expose(), "hunter2-secret");
    }

    #[test]
    fn deserializes_from_plain_string() {
        let s: SecretString = serde_json::from_str("\"pw\"").unwrap();
        assert_eq!(s.expose(), "pw");
    }
}
