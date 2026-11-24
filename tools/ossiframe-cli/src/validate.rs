//! Validation of generator inputs. Every value that ends up in a generated
//! file or path is parsed into one of these types first.

use ossiframe_core::ModuleId;
use std::fs;
use std::path::{Path, PathBuf};

pub const OSSIFRAME_NAMESPACE: &str = "dev.ossiframe.";

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("{field}: {reason}")]
pub struct InputError {
    pub field: &'static str,
    pub reason: String,
}

fn err<T>(field: &'static str, reason: impl Into<String>) -> Result<T, InputError> {
    Err(InputError {
        field,
        reason: reason.into(),
    })
}

/// Product name: 1 to 64 chars, starts with an ASCII letter, then
/// `[A-Za-z0-9 _-]`. Safe inside JSON, TSX, Rust strings and HTML.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductName(String);

impl ProductName {
    pub fn parse(input: &str) -> Result<Self, InputError> {
        let n = input.chars().count();
        if !(1..=64).contains(&n) {
            return err("name", "must be 1 to 64 characters");
        }
        let mut chars = input.chars();
        if !chars.next().is_some_and(|c| c.is_ascii_alphabetic()) {
            return err("name", "must start with a letter");
        }
        if !input
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '-'))
        {
            return err(
                "name",
                "may only contain letters, digits, space, '_' and '-'",
            );
        }
        Ok(Self(input.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Kebab-case Cargo package and npm name, for example `Acme Ops` to `acme-ops`.
    pub fn package_name(&self) -> Result<String, InputError> {
        let mut out = String::new();
        for c in self.0.chars() {
            if c.is_ascii_alphanumeric() {
                out.push(c.to_ascii_lowercase());
            } else if !out.ends_with('-') {
                out.push('-');
            }
        }
        let out = out.trim_end_matches('-').to_owned();
        let lib = out.replace('-', "_");
        if out.is_empty() || !out.starts_with(|c: char| c.is_ascii_lowercase()) {
            return err("name", "does not produce a valid package name");
        }
        if out.starts_with("ossiframe") {
            return err("name", "package name must not start with 'ossiframe'");
        }
        if RESERVED_CRATE_NAMES.contains(&lib.as_str()) {
            return err("name", format!("package name '{out}' is reserved"));
        }
        Ok(out)
    }

    /// Snake-case Rust library name derived from the package name.
    pub fn lib_name(&self) -> Result<String, InputError> {
        Ok(format!("{}_lib", self.package_name()?.replace('-', "_")))
    }
}

/// Names that would clash with std, the sysroot or crates the app depends on.
const RESERVED_CRATE_NAMES: &[&str] = &[
    "std",
    "core",
    "alloc",
    "proc_macro",
    "test",
    "tauri",
    "tokio",
    "serde",
    "serde_json",
    "build",
    "main",
    "app",
    "crate",
    "self",
    "super",
];

/// Reverse-DNS app identifier: two or more segments of `[a-z][a-z0-9-]*`,
/// at most 128 chars, not ending in `.app`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppIdentifier(String);

impl AppIdentifier {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn parse(input: &str, allow_ossiframe_namespace: bool) -> Result<Self, InputError> {
        if input.is_empty() || input.len() > 128 {
            return err("identifier", "must be 1 to 128 characters");
        }
        let segments: Vec<&str> = input.split('.').collect();
        if segments.len() < 2 {
            return err("identifier", "needs at least two dot-separated segments");
        }
        for s in &segments {
            let mut cs = s.chars();
            let ok = cs.next().is_some_and(|c| c.is_ascii_lowercase())
                && cs.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
            if !ok {
                return err(
                    "identifier",
                    "each segment must match [a-z][a-z0-9-]* (lowercase)",
                );
            }
        }
        if input.ends_with(".app") {
            return err("identifier", "must not end with '.app'");
        }
        let in_namespace = input.starts_with(OSSIFRAME_NAMESPACE) || input == "dev.ossiframe";
        if in_namespace && !allow_ossiframe_namespace {
            return err(
                "identifier",
                "'dev.ossiframe.*' is reserved; pass --allow-ossiframe-namespace to override",
            );
        }
        Ok(Self(input.to_owned()))
    }
}

/// Parses a 40-hex commit id and normalizes it to lowercase.
pub fn parse_rev(input: &str) -> Result<String, InputError> {
    if input.len() == 40 && input.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(input.to_ascii_lowercase())
    } else {
        err("rev", "must be a full 40-character hex commit id")
    }
}

pub fn parse_modules(inputs: &[String]) -> Result<Vec<ModuleId>, InputError> {
    let mut out: Vec<ModuleId> = Vec::new();
    for s in inputs {
        let id = ModuleId::parse(s).map_err(|e| InputError {
            field: "modules",
            reason: format!("'{s}': {}", e.reason),
        })?;
        if !out.contains(&id) {
            out.push(id);
        }
    }
    Ok(out)
}

/// The target must be absent or an empty directory. Nothing is created here.
pub fn check_target_dir(dir: &Path) -> Result<PathBuf, InputError> {
    let raw = dir.to_string_lossy();
    if raw.is_empty() || raw.chars().any(char::is_control) {
        return err("dir", "must be a non-empty path without control characters");
    }
    match fs::symlink_metadata(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(dir.to_path_buf()),
        Err(e) => err("dir", format!("cannot inspect: {e}")),
        Ok(m) if m.is_dir() => match fs::read_dir(dir) {
            Ok(mut it) => {
                if it.next().is_none() {
                    Ok(dir.to_path_buf())
                } else {
                    err("dir", "exists and is not empty; refusing to write into it")
                }
            }
            Err(e) => err("dir", format!("cannot read: {e}")),
        },
        Ok(_) => err("dir", "exists and is not a directory"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_accepts_plain_and_rejects_hostile() {
        assert_eq!(ProductName::parse("Acme Ops").unwrap().as_str(), "Acme Ops");
        for bad in [
            "",
            "../x",
            "a\"b",
            "a\\b",
            "a\"; rm -rf /",
            "1abc",
            " a",
            "a\u{7}b",
            "a\nb",
            "a<b>",
            "a'b",
            "ünï",
            &"a".repeat(65),
        ] {
            assert!(ProductName::parse(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn derived_names() {
        let n = ProductName::parse("Acme  Ops_2").unwrap();
        assert_eq!(n.package_name().unwrap(), "acme-ops-2");
        assert_eq!(n.lib_name().unwrap(), "acme_ops_2_lib");
        assert!(ProductName::parse("Test").unwrap().package_name().is_err());
        assert!(
            ProductName::parse("Ossiframe Thing")
                .unwrap()
                .package_name()
                .is_err()
        );
    }

    #[test]
    fn identifier_rules() {
        assert!(AppIdentifier::parse("dev.example.min", false).is_ok());
        for bad in [
            "",
            "single",
            "a..b",
            "Dev.example",
            "dev.example.app",
            "dev.1x.y",
            "dev.ex ample",
            "../x.y",
            "a.b\"c",
            "dev.example.x;y",
            &format!("a.{}", "b".repeat(130)),
        ] {
            assert!(
                AppIdentifier::parse(bad, false).is_err(),
                "accepted {bad:?}"
            );
        }
    }

    #[test]
    fn ossiframe_namespace_needs_flag() {
        assert!(AppIdentifier::parse("dev.ossiframe.mine", false).is_err());
        assert!(AppIdentifier::parse("dev.ossiframe.mine", true).is_ok());
    }

    #[test]
    fn rev_must_be_full_hex() {
        assert_eq!(parse_rev(&"AB".repeat(20)).unwrap(), "ab".repeat(20));
        assert!(parse_rev("abc123").is_err());
        assert!(parse_rev(&"g".repeat(40)).is_err());
    }

    #[test]
    fn module_ids_are_validated_and_deduplicated() {
        let ok = parse_modules(&["a".into(), "b".into(), "a".into()]).unwrap();
        assert_eq!(ok.len(), 2);
        assert!(parse_modules(&["../x".into()]).is_err());
        assert!(parse_modules(&["".into()]).is_err());
    }

    #[test]
    fn target_dir_rules() {
        let base = std::env::temp_dir().join(format!("ossiframe-cli-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();

        assert!(check_target_dir(&base.join("absent")).is_ok());
        let empty = base.join("empty");
        fs::create_dir(&empty).unwrap();
        assert!(check_target_dir(&empty).is_ok());
        let full = base.join("full");
        fs::create_dir(&full).unwrap();
        fs::write(full.join("f"), "x").unwrap();
        assert!(
            check_target_dir(&full)
                .unwrap_err()
                .reason
                .contains("not empty")
        );
        let file = base.join("file");
        fs::write(&file, "x").unwrap();
        assert!(check_target_dir(&file).is_err());
        assert!(check_target_dir(Path::new("")).is_err());
        assert!(check_target_dir(Path::new("a\nb")).is_err());

        fs::remove_dir_all(&base).unwrap();
    }
}
