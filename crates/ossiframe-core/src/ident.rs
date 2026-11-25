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

/// Generic Linux user name: `^[a-z_][a-z0-9_-]{0,31}$`, never `root`. Used as
/// the target of `Privilege::RunAs`. Naming conventions live in modules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct LinuxUserName(String);
string_newtype!(LinuxUserName);

impl LinuxUserName {
    pub fn parse(input: &str) -> Result<Self, ValidationError> {
        const F: &str = "linuxUser";
        // Same grammar as the SSH login name, minus `root`.
        SshUser::parse(input).map_err(|_| {
            ValidationError::new(
                F,
                "must start with a lowercase letter or '_' and contain only a-z, 0-9, '_' or '-' (max 32)",
            )
        })?;
        if input == "root" {
            return Err(ValidationError::new(F, "must not be root"));
        }
        Ok(Self(input.to_owned()))
    }
}

/// Application slug: 3 to 32 characters of a-z, 0-9 and '-'. It must start
/// with a letter and not end with '-', so generated names stay valid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AppSlug(String);
string_newtype!(AppSlug);

impl AppSlug {
    pub fn parse(input: &str) -> Result<Self, ValidationError> {
        const F: &str = "slug";
        let b = input.as_bytes();
        if !(3..=32).contains(&b.len()) {
            return Err(ValidationError::new(F, "must be 3 to 32 characters"));
        }
        if !b
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
        {
            return Err(ValidationError::new(F, "only a-z, 0-9 and '-' are allowed"));
        }
        if !b[0].is_ascii_lowercase() || b[b.len() - 1] == b'-' {
            return Err(ValidationError::new(
                F,
                "must start with a letter and not end with '-'",
            ));
        }
        Ok(Self(input.to_owned()))
    }
}

/// A public DNS name with at least two labels. No wildcard, scheme, port,
/// path, IP address or trailing dot. Stored lowercase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DomainName(String);
string_newtype!(DomainName);

impl DomainName {
    pub fn parse(input: &str) -> Result<Self, ValidationError> {
        const F: &str = "domain";
        if input.is_empty() || input.len() > 253 {
            return Err(ValidationError::new(F, "must be 1 to 253 characters"));
        }
        let labels: Vec<&str> = input.split('.').collect();
        if labels.len() < 2 {
            return Err(ValidationError::new(F, "must have at least two labels"));
        }
        for label in &labels {
            let b = label.as_bytes();
            if b.is_empty() || b.len() > 63 {
                return Err(ValidationError::new(
                    F,
                    "each label must be 1 to 63 characters",
                ));
            }
            if !b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'-') {
                return Err(ValidationError::new(
                    F,
                    "only letters, digits, '-' and '.' are allowed",
                ));
            }
            if b[0] == b'-' || b[b.len() - 1] == b'-' {
                return Err(ValidationError::new(
                    F,
                    "labels must not start or end with '-'",
                ));
            }
        }
        // An all-numeric last label is an IPv4 address, not a domain.
        if labels[labels.len() - 1].bytes().all(|c| c.is_ascii_digit()) {
            return Err(ValidationError::new(F, "must not be an IP address"));
        }
        Ok(Self(input.to_ascii_lowercase()))
    }
}

/// A Git remote: `https://host[:port]/path`, `ssh://git@host[:port]/path` or
/// `git@host:path`. No userinfo on https, no local paths, no `file://`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct GitUrl(String);
string_newtype!(GitUrl);

impl GitUrl {
    pub fn parse(input: &str) -> Result<Self, ValidationError> {
        const F: &str = "repositoryUrl";
        let bad = |r: &str| Err(ValidationError::new(F, r.to_owned()));
        if input.is_empty() || input.len() > 2048 {
            return bad("must be 1 to 2048 characters");
        }
        if input.starts_with('-') {
            return bad("must not start with '-'");
        }
        let rest = if let Some(r) = input.strip_prefix("https://") {
            r
        } else if let Some(r) = input.strip_prefix("ssh://git@") {
            r
        } else if let Some(r) = input.strip_prefix("git@") {
            // scp form: host:path
            let Some((host, path)) = r.split_once(':') else {
                return bad("expected git@host:owner/repo");
            };
            if Hostname::parse(host).is_err() || host.starts_with('-') {
                return bad("invalid host");
            }
            Self::check_path(path)?;
            return Ok(Self(input.to_owned()));
        } else {
            return bad("must be an https:// URL, ssh://git@host/path or git@host:path");
        };
        let (authority, path) = match rest.split_once('/') {
            Some((a, p)) => (a, p),
            None => return bad("missing repository path"),
        };
        let host = match authority.rsplit_once(':') {
            Some((h, port)) => {
                if port.is_empty()
                    || port.len() > 5
                    || !port.bytes().all(|c| c.is_ascii_digit())
                    || port
                        .parse::<u32>()
                        .ok()
                        .and_then(|p| Port::new(p).ok())
                        .is_none()
                {
                    return bad("invalid port");
                }
                h
            }
            None => authority,
        };
        // Rejects userinfo ('@'), brackets and anything else outside a host name.
        if Hostname::parse(host).is_err() {
            return bad("invalid host");
        }
        Self::check_path(path)?;
        Ok(Self(input.to_owned()))
    }

    fn check_path(path: &str) -> Result<(), ValidationError> {
        let bad = |r: &str| Err(ValidationError::new("repositoryUrl", r.to_owned()));
        if path.is_empty() || path.starts_with('/') || path.ends_with('/') {
            return bad("invalid repository path");
        }
        if !path
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-' | b'/' | b'~'))
        {
            return bad("repository path has unsupported characters");
        }
        if path
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == "..")
        {
            return bad("repository path must not contain empty, '.' or '..' segments");
        }
        if path.starts_with('-') {
            return bad("must not start with '-'");
        }
        Ok(())
    }

    /// True for `ssh://` and `git@host:path` forms.
    pub fn is_ssh(&self) -> bool {
        !self.0.starts_with("https://")
    }
}

/// A Git branch name following the basics of `git check-ref-format`:
/// a-z, A-Z, 0-9, `._/-`, no leading '-', no `..`, no empty segment, no
/// segment starting with '.' or ending with `.lock`, no trailing '.'.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct BranchName(String);
string_newtype!(BranchName);

impl BranchName {
    pub fn parse(input: &str) -> Result<Self, ValidationError> {
        const F: &str = "branch";
        let bad = |r: &str| Err(ValidationError::new(F, r.to_owned()));
        if input.is_empty() || input.len() > 255 {
            return bad("must be 1 to 255 characters");
        }
        if input.starts_with('-') {
            return bad("must not start with '-'");
        }
        if !input
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-' | b'/'))
        {
            return bad("only letters, digits, '.', '_', '-' and '/' are allowed");
        }
        if input.contains("..") || input.ends_with('.') {
            return bad("must not contain '..' or end with '.'");
        }
        if input
            .split('/')
            .any(|s| s.is_empty() || s.starts_with('.') || s.ends_with(".lock"))
        {
            return bad("invalid path segment");
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
    fn linux_user_names() {
        for ok in ["ossi-blog", "_svc", "a", "www-data"] {
            assert!(LinuxUserName::parse(ok).is_ok(), "{ok}");
        }
        for bad in [
            "root",
            "",
            "Root",
            "1a",
            "-u",
            "a b",
            "a;b",
            "a$",
            &"a".repeat(33),
        ] {
            assert!(LinuxUserName::parse(bad).is_err(), "{bad:?}");
        }
        assert!(serde_json::from_str::<LinuxUserName>("\"root\"").is_err());
    }

    #[test]
    fn app_slugs() {
        for ok in ["abc", "my-app-2", &format!("a{}", "b".repeat(31))] {
            assert!(AppSlug::parse(ok).is_ok(), "{ok}");
        }
        let long = "a".repeat(33);
        for bad in [
            "",
            "ab",
            long.as_str(),
            "My-App",
            "-abc",
            "abc-",
            "1abc",
            "a_b",
            "a/b",
            "../x",
            "a b",
            "a;b",
            "a.b",
            "abc\n",
        ] {
            assert!(AppSlug::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn domains() {
        assert_eq!(
            DomainName::parse("Blog.Example.com").unwrap().as_str(),
            "blog.example.com"
        );
        let long = format!("{}.com", "a".repeat(64));
        for bad in [
            "",
            "localhost",
            "*.example.com",
            "https://example.com",
            "example.com/path",
            "example.com:8080",
            "example.com.",
            "a..com",
            "-a.com",
            "a-.com",
            "10.0.0.1",
            "a b.com",
            "a;b.com",
            "ex\nample.com",
            "../etc.com",
            long.as_str(),
        ] {
            assert!(DomainName::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn git_urls() {
        for ok in [
            "https://github.com/owner/repo.git",
            "https://git.example.com:8443/o/r",
            "git@github.com:owner/repo.git",
            "git@gitlab.com:group/sub/repo",
            "ssh://git@host.example.com:2222/owner/repo.git",
        ] {
            assert!(GitUrl::parse(ok).is_ok(), "{ok}");
        }
        assert!(GitUrl::parse("git@github.com:o/r").unwrap().is_ssh());
        assert!(!GitUrl::parse("https://github.com/o/r").unwrap().is_ssh());
        for bad in [
            "",
            "file:///etc/passwd",
            "/srv/repo",
            "./repo",
            "http://github.com/o/r",
            "ftp://github.com/o/r",
            "-oProxyCommand=x",
            "--upload-pack=x",
            "https://user:pw@github.com/o/r",
            "https://user@github.com/o/r",
            "https://github.com",
            "https://github.com/",
            "https://github.com/o/../r",
            "https://github.com/o/r r",
            "https://github.com/o/r;id",
            "https://github.com/o/$(id)",
            "https://github.com:99999/o/r",
            "https://-evil.com/o/r",
            "git@-evil.com:o/r",
            "git@github.com:/abs/path",
            "git@github.com:o/../r",
            "git@github.com:o/r\n",
            "git@github.com",
            "root@github.com:o/r",
            "ssh://root@host.com/o/r",
            "ssh://git@host.com",
            "ext::sh -c id",
        ] {
            assert!(GitUrl::parse(bad).is_err(), "{bad:?}");
        }
        assert!(GitUrl::parse(&format!("https://a.com/{}", "a".repeat(2048))).is_err());
    }

    #[test]
    fn branches() {
        for ok in ["main", "feature/x-1", "release_1.2", "v1.0.0"] {
            assert!(BranchName::parse(ok).is_ok(), "{ok}");
        }
        let long = "a".repeat(256);
        for bad in [
            "",
            "-main",
            "--upload-pack=x",
            "a..b",
            "/a",
            "a/",
            "a//b",
            ".hidden",
            "a/.b",
            "a.lock",
            "a/b.lock",
            "a.",
            "a b",
            "a;b",
            "a$(id)",
            "a~1",
            "a^",
            "a:b",
            "a?b",
            "a*b",
            "a[b",
            "a\\b",
            "a@{b",
            "a\n",
            long.as_str(),
        ] {
            assert!(BranchName::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn serde_validates_at_the_boundary() {
        assert!(serde_json::from_str::<GitUrl>("\"file:///x\"").is_err());
        assert!(serde_json::from_str::<AppSlug>("\"../x\"").is_err());
        assert!(serde_json::from_str::<Hostname>("\"-oProxyCommand=x\"").is_err());
        assert!(serde_json::from_str::<Port>("0").is_err());
        assert_eq!(serde_json::from_str::<Port>("2222").unwrap().get(), 2222);
    }
}
