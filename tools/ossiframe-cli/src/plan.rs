//! Module catalog, module resolution and Ossiframe source (rev) selection.
//! Pure functions over manifests and an injected git probe; no file writes.
use ossiframe_core::{Module, ModuleId, ModuleManifest, RegistryBuilder, RegistryError};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const GIT_URL: &str = "https://github.com/shojibur/Ossiframe";

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error("modules: {0}")]
    Modules(String),
    #[error("{0}")]
    Source(String),
}

/// A module the generator knows. The manifest is read from the module crate
/// itself, so ids, names and dependencies have one source of truth.
#[derive(Debug, Clone)]
pub struct CatalogEntry {
    pub manifest: ModuleManifest,
    pub crate_name: &'static str,
    /// Rust path of the unit struct that implements `Module`.
    pub type_path: &'static str,
    /// Crate directory relative to the Ossiframe repository root.
    pub crate_dir: &'static str,
    /// Optional `ui/` directory relative to the repository root.
    pub ui_dir: Option<&'static str>,
    /// Example modules are copied into the generated app instead of referenced.
    pub is_example: bool,
    /// Needs root on the server. Only generated with `--enable-provisioning`.
    pub privileged: bool,
}

pub fn catalog() -> Vec<CatalogEntry> {
    vec![
        CatalogEntry {
            manifest: ossiframe_mod_linux_inventory::LinuxInventory.manifest(),
            crate_name: "ossiframe-mod-linux-inventory",
            type_path: "ossiframe_mod_linux_inventory::LinuxInventory",
            crate_dir: "modules/linux-inventory",
            ui_dir: Some("modules/linux-inventory/ui"),
            is_example: false,
            privileged: false,
        },
        CatalogEntry {
            manifest: ossiframe_mod_linux_audit::LinuxAudit.manifest(),
            crate_name: "ossiframe-mod-linux-audit",
            type_path: "ossiframe_mod_linux_audit::LinuxAudit",
            crate_dir: "modules/linux-audit",
            ui_dir: None,
            is_example: false,
            privileged: false,
        },
        CatalogEntry {
            manifest: ossiframe_mod_ports_example::Ports.manifest(),
            crate_name: "ossiframe-mod-ports-example",
            type_path: "ossiframe_mod_ports_example::Ports",
            crate_dir: "examples/custom-module",
            ui_dir: Some("examples/custom-module/ui"),
            is_example: true,
            privileged: false,
        },
        CatalogEntry {
            manifest: ossiframe_mod_ubuntu_provisioning::UbuntuProvisioning.manifest(),
            crate_name: "ossiframe-mod-ubuntu-provisioning",
            type_path: "ossiframe_mod_ubuntu_provisioning::UbuntuProvisioning",
            crate_dir: "modules/ubuntu-provisioning",
            ui_dir: Some("modules/ubuntu-provisioning/ui"),
            is_example: false,
            privileged: true,
        },
    ]
}

/// A selected module in resolved (dependencies first) order.
#[derive(Debug, Clone)]
pub struct ResolvedModule {
    pub entry: CatalogEntry,
    /// Valid TypeScript identifier for the UI import.
    pub ui_alias: String,
}

impl ResolvedModule {
    pub fn id(&self) -> &ModuleId {
        &self.entry.manifest.id
    }
}

/// Selects the requested modules (plus `ports` for the example flag) and their
/// required dependencies. No selection means no modules. Unknown ids,
/// unknown dependencies and cycles fail here, before anything is written.
pub fn resolve_modules(
    catalog: &[CatalogEntry],
    requested: &[ModuleId],
    with_example: bool,
) -> Result<Vec<ResolvedModule>, PlanError> {
    let mut wanted: Vec<ModuleId> = Vec::new();
    for id in requested {
        match catalog.iter().find(|e| &e.manifest.id == id) {
            Some(e) if e.is_example => {
                return Err(PlanError::Modules(format!(
                    "'{id}' is an example module; use --with-example-module"
                )));
            }
            _ => wanted.push(id.clone()),
        }
    }
    if with_example {
        wanted.extend(
            catalog
                .iter()
                .filter(|e| e.is_example)
                .map(|e| e.manifest.id.clone()),
        );
    }
    let manifests: Vec<ModuleManifest> = catalog.iter().map(|e| e.manifest.clone()).collect();
    let order = RegistryBuilder::resolve_selection(&manifests, &wanted)
        .map_err(|e: RegistryError| PlanError::Modules(e.to_string()))?;
    Ok(order
        .iter()
        .filter_map(|id| catalog.iter().find(|e| &e.manifest.id == id))
        .map(|e| ResolvedModule {
            ui_alias: format!("ui_{}", e.manifest.id.as_str().replace('-', "_")),
            entry: e.clone(),
        })
        .collect())
}

/// The generated app may run root commands only when the user opted in.
/// Checked on the resolved list, so a privileged module pulled in as a
/// dependency is covered too. The flag without such a module is also refused,
/// so it never silently does nothing.
pub fn check_privileged(
    modules: &[ResolvedModule],
    enable_provisioning: bool,
) -> Result<(), PlanError> {
    let privileged: Vec<&str> = modules
        .iter()
        .filter(|m| m.entry.privileged)
        .map(|m| m.id().as_str())
        .collect();
    if !privileged.is_empty() && !enable_provisioning {
        return Err(PlanError::Modules(format!(
            "'{}' runs commands as root on servers and is off by default; pass --enable-provisioning to generate an app that includes it",
            privileged.join("', '")
        )));
    }
    if privileged.is_empty() && enable_provisioning {
        return Err(PlanError::Modules(
            "--enable-provisioning needs --modules ubuntu-provisioning".to_owned(),
        ));
    }
    Ok(())
}

/// Where the generated app gets Ossiframe crates from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OssiframeSource {
    /// Exact public git commit.
    Git { rev: String },
    /// Plain path dependencies on a local checkout (testing only).
    Local { root: PathBuf },
}

/// Read-only git queries about the Ossiframe checkout.
pub trait GitProbe {
    fn head(&self) -> Result<String, String>;
    /// Output of `git status --porcelain`; empty means a clean tree.
    fn status(&self) -> Result<String, String>;
}

/// Runs `git` (fixed argv, no shell, no network) in the repository root.
pub struct RepoGit(pub PathBuf);

impl RepoGit {
    fn run(&self, args: &[&str]) -> Result<String, String> {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.0)
            .args(args)
            .output()
            .map_err(|e| format!("could not run git: {e}"))?;
        if !out.status.success() {
            return Err(format!("git {} failed", args.join(" ")));
        }
        String::from_utf8(out.stdout).map_err(|_| "git printed non-UTF-8 output".to_owned())
    }
}

impl GitProbe for RepoGit {
    fn head(&self) -> Result<String, String> {
        Ok(self.run(&["rev-parse", "HEAD"])?.trim().to_owned())
    }
    fn status(&self) -> Result<String, String> {
        self.run(&["status", "--porcelain"])
    }
}

/// `--ossiframe-local` needs `<path>/crates/ossiframe-core/Cargo.toml`. The
/// path is canonicalized and must be valid UTF-8 without control characters.
pub fn local_source(path: &Path) -> Result<OssiframeSource, PlanError> {
    let bad = |why: &str| PlanError::Source(format!("--ossiframe-local: {why}"));
    let root = path
        .canonicalize()
        .map_err(|_| bad("path does not exist"))?;
    if !root.join("crates/ossiframe-core/Cargo.toml").is_file() {
        return Err(bad("crates/ossiframe-core/Cargo.toml not found there"));
    }
    match root.to_str() {
        Some(s) if !s.chars().any(char::is_control) => Ok(OssiframeSource::Local { root }),
        _ => Err(bad("path must be UTF-8 without control characters")),
    }
}

/// Picks the source. `--ossiframe-local` wins over git; `--rev` skips git
/// entirely; otherwise the pin is HEAD of a clean tree.
pub fn pick_source(
    rev: Option<&str>,
    local: Option<&Path>,
    git: &dyn GitProbe,
) -> Result<OssiframeSource, PlanError> {
    if let Some(p) = local {
        return local_source(p);
    }
    if let Some(r) = rev {
        return Ok(OssiframeSource::Git { rev: r.to_owned() });
    }
    let head = git.head().map_err(|e| {
        PlanError::Source(format!(
            "cannot determine the Ossiframe revision ({e}); pass --rev <40-hex commit>"
        ))
    })?;
    let status = git.status().map_err(|e| {
        PlanError::Source(format!(
            "cannot check the working tree ({e}); pass --rev <40-hex commit>"
        ))
    })?;
    if !status.trim().is_empty() {
        return Err(PlanError::Source(
            "the Ossiframe working tree has uncommitted changes; commit them, or pass --rev or --ossiframe-local"
                .to_owned(),
        ));
    }
    crate::validate::parse_rev(head.trim())
        .map(|rev| OssiframeSource::Git { rev })
        .map_err(|e| PlanError::Source(format!("git printed an unexpected HEAD: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> ModuleId {
        ModuleId::parse(s).unwrap()
    }
    fn entry(name: &str, deps: &[&str], example: bool) -> CatalogEntry {
        CatalogEntry {
            manifest: ModuleManifest {
                id: id(name),
                name: name.to_owned(),
                version: "0.1.0".to_owned(),
                description: String::new(),
                dependencies: deps.iter().map(|d| id(d)).collect(),
            },
            crate_name: "x",
            type_path: "x::X",
            crate_dir: "x",
            ui_dir: None,
            is_example: example,
            privileged: false,
        }
    }
    fn ids(v: &[ResolvedModule]) -> Vec<&str> {
        v.iter().map(|m| m.id().as_str()).collect()
    }

    #[test]
    fn zero_modules_is_valid() {
        assert!(resolve_modules(&catalog(), &[], false).unwrap().is_empty());
    }

    #[test]
    fn real_catalog_selection() {
        let m = resolve_modules(&catalog(), &[id("linux-inventory")], true).unwrap();
        assert_eq!(ids(&m), ["linux-inventory", "ports"]);
        assert_eq!(m[0].ui_alias, "ui_linux_inventory");
        let m = resolve_modules(&catalog(), &[], true).unwrap();
        assert_eq!(ids(&m), ["ports"]);
    }

    #[test]
    fn linux_audit_resolves_with_its_dependency_in_order() {
        let m = resolve_modules(&catalog(), &[id("linux-audit")], false).unwrap();
        assert_eq!(ids(&m), ["linux-inventory", "linux-audit"]);
        assert!(m[1].entry.ui_dir.is_none());
    }

    #[test]
    fn privileged_module_needs_the_flag_and_the_flag_needs_the_module() {
        let m = resolve_modules(&catalog(), &[id("ubuntu-provisioning")], false).unwrap();
        assert_eq!(ids(&m), ["linux-inventory", "ubuntu-provisioning"]);
        let e = check_privileged(&m, false).unwrap_err().to_string();
        assert!(e.contains("--enable-provisioning"), "{e}");
        assert!(check_privileged(&m, true).is_ok());
        let plain = resolve_modules(&catalog(), &[id("linux-inventory")], false).unwrap();
        assert!(check_privileged(&plain, false).is_ok());
        assert!(check_privileged(&plain, true).is_err());
    }

    #[test]
    fn unknown_module_and_example_without_flag_are_rejected() {
        assert!(resolve_modules(&catalog(), &[id("nope")], false).is_err());
        let e = resolve_modules(&catalog(), &[id("ports")], false).unwrap_err();
        assert!(e.to_string().contains("--with-example-module"));
    }

    #[test]
    fn required_dependencies_are_included_and_unrelated_are_not() {
        let c = [
            entry("a", &["b"], false),
            entry("b", &[], false),
            entry("c", &[], false),
        ];
        let m = resolve_modules(&c, &[id("a")], false).unwrap();
        assert_eq!(ids(&m), ["b", "a"]);
    }

    #[test]
    fn unknown_dependency_and_cycle_are_rejected() {
        let c = [entry("a", &["ghost"], false)];
        assert!(resolve_modules(&c, &[id("a")], false).is_err());
        let c = [entry("a", &["b"], false), entry("b", &["a"], false)];
        let e = resolve_modules(&c, &[id("a")], false).unwrap_err();
        assert!(e.to_string().contains("cycle"), "{e}");
    }

    struct Fake(Result<String, String>, Result<String, String>);
    impl GitProbe for Fake {
        fn head(&self) -> Result<String, String> {
            self.0.clone()
        }
        fn status(&self) -> Result<String, String> {
            self.1.clone()
        }
    }
    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn clean_tree_pins_head() {
        let g = Fake(Ok(format!("{SHA}\n")), Ok(String::new()));
        assert_eq!(
            pick_source(None, None, &g).unwrap(),
            OssiframeSource::Git { rev: SHA.into() }
        );
    }

    #[test]
    fn dirty_tree_is_refused_unless_rev_or_local_given() {
        let g = Fake(Ok(SHA.into()), Ok(" M crates/x.rs\n".into()));
        assert!(pick_source(None, None, &g).is_err());
        let other = "f".repeat(40);
        assert_eq!(
            pick_source(Some(&other), None, &g).unwrap(),
            OssiframeSource::Git { rev: other }
        );
    }

    #[test]
    fn git_failure_asks_for_rev() {
        let g = Fake(Err("no git".into()), Ok(String::new()));
        let e = pick_source(None, None, &g).unwrap_err();
        assert!(e.to_string().contains("--rev"));
    }

    #[test]
    fn local_path_must_be_a_checkout() {
        let g = Fake(Err("unused".into()), Err("unused".into()));
        assert!(pick_source(None, Some(Path::new("/definitely/not/here")), &g).is_err());
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        assert!(matches!(
            pick_source(None, Some(&root), &g).unwrap(),
            OssiframeSource::Local { .. }
        ));
    }
}
