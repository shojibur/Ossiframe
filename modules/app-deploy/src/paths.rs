//! Typed paths. Root writes accept only [`ManagedPath`]; writes inside an
//! app directory use [`AppPath`] relative to an [`AppRoot`] and run as the
//! app user.

use crate::identity::MAX_APP_SLUG_LEN;
use ossiframe_core::{AppSlug, ValidationError};

/// Suffixes used by the write and backup steps. A managed path may not end in
/// one of them, so a temp or backup file is never itself a target.
pub const TMP_SUFFIX: &str = ".ossiframe-tmp";
pub const BAK_SUFFIX: &str = ".ossiframe-bak";

pub const NGINX_SITES_AVAILABLE: &str = "/etc/nginx/sites-available/ossiframe-";
pub const FPM_POOL_DIR: &str = "/etc/php/8.3/fpm/pool.d/ossiframe-";
pub const NGINX_SITES_ENABLED: &str = "/etc/nginx/sites-enabled/ossiframe-";

/// Root of all app directories.
pub const APPS_ROOT: &str = "/srv/ossiframe/apps";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedKind {
    NginxSite,
    FpmPool,
    /// A `sites-enabled` symlink. Never the target of a content write.
    NginxEnabledLink,
}

/// An absolute, root-owned path Ossiframe may manage: one of
/// `/etc/nginx/sites-available/ossiframe-<name>`,
/// `/etc/php/8.3/fpm/pool.d/ossiframe-<name>` or
/// `/etc/nginx/sites-enabled/ossiframe-<name>`, where `<name>` is a non-empty
/// run of `a-z 0-9 . _ -` without `..`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedPath {
    path: String,
    kind: ManagedKind,
}

impl ManagedPath {
    pub fn parse(input: &str) -> Result<Self, ValidationError> {
        const F: &str = "path";
        let (prefix, kind) = [
            (NGINX_SITES_AVAILABLE, ManagedKind::NginxSite),
            (FPM_POOL_DIR, ManagedKind::FpmPool),
            (NGINX_SITES_ENABLED, ManagedKind::NginxEnabledLink),
        ]
        .into_iter()
        .find(|(p, _)| input.starts_with(p))
        .ok_or_else(|| ValidationError::new(F, "is not an Ossiframe-managed path"))?;
        let name = &input[prefix.len()..];
        let ok = !name.is_empty()
            && name.len() <= 128
            && !name.contains("..")
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b));
        if !ok {
            return Err(ValidationError::new(
                F,
                "managed file name must use only a-z, 0-9, '.', '_' and '-'",
            ));
        }
        if input.ends_with(TMP_SUFFIX) || input.ends_with(BAK_SUFFIX) {
            return Err(ValidationError::new(F, "must not be a temp or backup name"));
        }
        Ok(Self {
            path: input.to_owned(),
            kind,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.path
    }

    pub fn kind(&self) -> ManagedKind {
        self.kind
    }
}

/// A validated relative path: non-empty `/`-separated segments of
/// `A-Z a-z 0-9 . _ -`, no `.` or `..` segment, no leading `/`, at most 128
/// characters. No backslash, NUL, space or shell metacharacter.
fn validate_relative(field: &'static str, input: &str) -> Result<(), ValidationError> {
    if input.is_empty() || input.len() > 128 {
        return Err(ValidationError::new(field, "must be 1 to 128 characters"));
    }
    if input.starts_with('/') {
        return Err(ValidationError::new(field, "must be relative"));
    }
    for seg in input.split('/') {
        let ok = !seg.is_empty()
            && seg != "."
            && seg != ".."
            && !seg.starts_with('-')
            && seg
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
        if !ok {
            return Err(ValidationError::new(
                field,
                "segments may use only letters, digits, '.', '_' and '-', and may not be empty, '.' or '..'",
            ));
        }
    }
    Ok(())
}

/// The entry file of an application, relative to a release directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryFile(String);

impl EntryFile {
    pub fn parse(input: &str) -> Result<Self, ValidationError> {
        validate_relative("entryFile", input)?;
        Ok(Self(input.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Directory part (`public` for `public/index.php`), `None` at the root.
    pub fn dir(&self) -> Option<&str> {
        self.0.rsplit_once('/').map(|(d, _)| d)
    }
}

/// `/srv/ossiframe/apps/<slug>`, built only from a capped slug.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppRoot(String);

impl AppRoot {
    pub fn new(slug: &AppSlug) -> Result<Self, ValidationError> {
        if slug.as_str().len() > MAX_APP_SLUG_LEN {
            return Err(ValidationError::new(
                "slug",
                "must be at most 27 characters",
            ));
        }
        Ok(Self(format!("{APPS_ROOT}/{}", slug.as_str())))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A path inside an app root (`.env`, `.ssh/known_hosts`, ...). Always
/// written as the app user, never as root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPath {
    rel: String,
    full: String,
}

impl AppPath {
    pub fn parse(root: &AppRoot, relative: &str) -> Result<Self, ValidationError> {
        validate_relative("appPath", relative)?;
        Ok(Self {
            full: format!("{}/{relative}", root.as_str()),
            rel: relative.to_owned(),
        })
    }

    pub fn as_str(&self) -> &str {
        &self.full
    }

    pub fn relative(&self) -> &str {
        &self.rel
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_path_accepts_three_prefixes_only() {
        for ok in [
            "/etc/nginx/sites-available/ossiframe-blog",
            "/etc/php/8.3/fpm/pool.d/ossiframe-blog.conf",
            "/etc/nginx/sites-enabled/ossiframe-blog",
        ] {
            assert!(ManagedPath::parse(ok).is_ok(), "{ok}");
        }
        for bad in [
            "/etc/passwd",
            "/etc/nginx/sites-available/default",
            "/etc/nginx/sites-available/ossiframe-",
            "/etc/nginx/sites-available/ossiframe-../../passwd",
            "/etc/nginx/sites-available/ossiframe-a/b",
            "/etc/nginx/sites-available/../sites-available/ossiframe-a",
            "/etc/php/8.3/fpm/pool.d/ossiframe-a b",
            "/etc/php/8.3/fpm/pool.d/ossiframe-a;rm",
            "/etc/nginx/sites-available/ossiframe-a.ossiframe-tmp",
            "/etc/nginx/sites-available/ossiframe-a.ossiframe-bak",
            "/srv/ossiframe/apps/blog/.env",
            "etc/nginx/sites-available/ossiframe-a",
            "",
        ] {
            assert!(ManagedPath::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn entry_file_rejects_hostile_input() {
        for ok in ["public/index.php", "index.html", "web/app.v2/index.php"] {
            assert!(EntryFile::parse(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "../index.php",
            "a/../b",
            "/index.php",
            "a//b",
            "./a",
            "a/",
            "a\\b",
            "a\0b",
            "a b",
            "a;b",
            "a$(x)",
            "a`x`",
            "a|b",
            "a&b",
            "a*",
            "-rf",
            "a/-b",
            "é.php",
        ] {
            assert!(EntryFile::parse(bad).is_err(), "{bad:?}");
        }
        assert_eq!(
            EntryFile::parse("public/index.php").unwrap().dir(),
            Some("public")
        );
        assert_eq!(EntryFile::parse("index.html").unwrap().dir(), None);
    }

    #[test]
    fn app_path_is_confined_to_the_root() {
        let root = AppRoot::new(&AppSlug::parse("blog").unwrap()).unwrap();
        assert_eq!(
            AppPath::parse(&root, ".ssh/known_hosts").unwrap().as_str(),
            "/srv/ossiframe/apps/blog/.ssh/known_hosts"
        );
        assert!(AppPath::parse(&root, "../other/.env").is_err());
        assert!(AppPath::parse(&root, "/etc/passwd").is_err());
        assert!(AppRoot::new(&AppSlug::parse(&"a".repeat(28)).unwrap()).is_err());
    }
}
