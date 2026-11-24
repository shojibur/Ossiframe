//! Validated identifiers. Every user-supplied value that reaches SSH or
//! storage is parsed into one of these types first.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{field}: {reason}")]
pub struct ValidationError {
    pub field: &'static str,
    pub reason: String,
}

impl ValidationError {
    pub fn new(field: &'static str, reason: impl Into<String>) -> Self {
        Self {
            field,
            reason: reason.into(),
        }
    }
}

macro_rules! string_newtype {
    ($name:ident) => {
        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
        impl TryFrom<String> for $name {
            type Error = ValidationError;
            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::parse(&value)
            }
        }
        impl From<$name> for String {
            fn from(value: $name) -> String {
                value.0
            }
        }
    };
}

/// Display name of a server profile: 1 to 64 characters, no control characters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ServerName(String);
string_newtype!(ServerName);

impl ServerName {
    pub fn parse(input: &str) -> Result<Self, ValidationError> {
        let v = input.trim();
        let len = v.chars().count();
        if len == 0 {
            return Err(ValidationError::new("name", "must not be empty"));
        }
        if len > 64 {
            return Err(ValidationError::new(
                "name",
                "must be at most 64 characters",
            ));
        }
        if v.chars().any(char::is_control) {
            return Err(ValidationError::new(
                "name",
                "must not contain control characters",
            ));
        }
        Ok(Self(v.to_owned()))
    }
}

/// An IP address literal or a DNS host name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Hostname(String);
string_newtype!(Hostname);

impl Hostname {
    pub fn parse(input: &str) -> Result<Self, ValidationError> {
        const F: &str = "host";
        if input.is_empty() {
            return Err(ValidationError::new(F, "must not be empty"));
        }
        if input.starts_with('-') {
            return Err(ValidationError::new(F, "must not start with '-'"));
        }
        if input.parse::<IpAddr>().is_ok() {
            return Ok(Self(input.to_owned()));
        }
        if input.len() > 253 {
            return Err(ValidationError::new(F, "must be at most 253 characters"));
        }
        for label in input.split('.') {
            if label.is_empty() || label.len() > 63 {
                return Err(ValidationError::new(
                    F,
                    "each label must be 1 to 63 characters",
                ));
            }
            if !label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            {
                return Err(ValidationError::new(
                    F,
                    "only letters, digits, '-' and '.' are allowed",
                ));
            }
            if label.starts_with('-') || label.ends_with('-') {
                return Err(ValidationError::new(
                    F,
                    "labels must not start or end with '-'",
                ));
            }
        }
        Ok(Self(input.to_ascii_lowercase()))
    }
}

/// TCP port, 1 to 65535.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u16")]
pub struct Port(u16);

impl Port {
    pub const SSH_DEFAULT: Port = Port(22);

    pub fn new(value: u32) -> Result<Self, ValidationError> {
        match u16::try_from(value) {
            Ok(p) if p != 0 => Ok(Self(p)),
            _ => Err(ValidationError::new("port", "must be between 1 and 65535")),
        }
    }

    pub fn get(self) -> u16 {
        self.0
    }
}

impl TryFrom<u32> for Port {
    type Error = ValidationError;
    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<Port> for u16 {
    fn from(p: Port) -> u16 {
        p.0
    }
}

impl fmt::Display for Port {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// SSH login name: `^[a-z_][a-z0-9_-]{0,31}$`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SshUser(String);
string_newtype!(SshUser);

impl SshUser {
    pub fn parse(input: &str) -> Result<Self, ValidationError> {
        let mut bytes = input.bytes();
        let ok_first = matches!(bytes.next(), Some(b) if b.is_ascii_lowercase() || b == b'_');
        let ok_rest =
            bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
        if !ok_first || !ok_rest || input.len() > 32 {
            return Err(ValidationError::new(
                "username",
                "must start with a lowercase letter or '_' and contain only a-z, 0-9, '_' or '-' (max 32)",
            ));
        }
        Ok(Self(input.to_owned()))
    }
}

/// Absolute path to a private key file. `~/` is expanded using `HOME`.
/// Existence is checked when connecting, not here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct KeyPath(String);
string_newtype!(KeyPath);

impl KeyPath {
    pub fn parse(input: &str) -> Result<Self, ValidationError> {
        const F: &str = "keyPath";
        let input = input.trim();
        if input.is_empty() {
            return Err(ValidationError::new(F, "must not be empty"));
        }
        if input.contains('\0') {
            return Err(ValidationError::new(F, "must not contain NUL"));
        }
        let expanded = match input.strip_prefix("~/") {
            Some(rest) => {
                let home = std::env::var("HOME")
                    .map_err(|_| ValidationError::new(F, "cannot expand '~': HOME is not set"))?;
                format!("{}/{}", home.trim_end_matches('/'), rest)
            }
            None => input.to_owned(),
        };
        if !Path::new(&expanded).is_absolute() {
            return Err(ValidationError::new(
                F,
                "must be an absolute path or start with '~/'",
            ));
        }
        Ok(Self(expanded))
    }

    pub fn to_path_buf(&self) -> PathBuf {
        PathBuf::from(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostnames() {
        for ok in [
            "example.com",
            "a.b-c.io",
            "10.0.0.1",
            "::1",
            "2001:db8::1",
            "Server1.Example.COM",
        ] {
            assert!(Hostname::parse(ok).is_ok(), "{ok}");
        }
        let long_label = format!("{}.com", "a".repeat(64));
        let too_long = vec!["a"; 127].join(".") + ".com";
        for bad in [
            "",
            "-oProxyCommand=x",
            "a b",
            "a;b",
            "a/b",
            "-a.com",
            "a-.com",
            "a..b",
            "host\n",
            "$(id)",
            "a`b`",
            long_label.as_str(),
            too_long.as_str(),
            "user@host",
        ] {
            assert!(Hostname::parse(bad).is_err(), "{bad:?}");
        }
        assert_eq!(
            Hostname::parse("Server1.Example.COM").unwrap().as_str(),
            "server1.example.com"
        );
    }

    #[test]
    fn ports() {
        assert!(Port::new(1).is_ok());
        assert!(Port::new(65535).is_ok());
        assert!(Port::new(0).is_err());
        assert!(Port::new(65536).is_err());
    }

    #[test]
    fn users() {
        for ok in ["root", "ubuntu", "_svc", "deploy-1", "a_b"] {
            assert!(SshUser::parse(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "Root",
            "1abc",
            "-oX",
            "a b",
            "a;b",
            "a$",
            &"a".repeat(33),
        ] {
            assert!(SshUser::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn names() {
        assert_eq!(ServerName::parse("  web 01 ").unwrap().as_str(), "web 01");
        assert!(ServerName::parse("   ").is_err());
        assert!(ServerName::parse("a\nb").is_err());
        assert!(ServerName::parse(&"x".repeat(65)).is_err());
        assert!(ServerName::parse(&"é".repeat(64)).is_ok());
    }

    #[test]
    fn key_paths() {
        assert!(KeyPath::parse("/Users/me/.ssh/id_ed25519").is_ok());
        assert!(KeyPath::parse("relative/key").is_err());
        assert!(KeyPath::parse("/a\0b").is_err());
        assert!(KeyPath::parse("").is_err());
        if std::env::var("HOME").is_ok() {
            let p = KeyPath::parse("~/.ssh/id").unwrap();
            assert!(p.as_str().ends_with("/.ssh/id") && !p.as_str().starts_with('~'));
        }
    }

    #[test]
    fn serde_validates_at_the_boundary() {
        assert!(serde_json::from_str::<Hostname>("\"-oProxyCommand=x\"").is_err());
        assert!(serde_json::from_str::<Port>("0").is_err());
        assert_eq!(serde_json::from_str::<Port>("2222").unwrap().get(), 2222);
    }
}
