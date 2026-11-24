//! Writes the generated app: render into a temporary sibling directory, then
//! move it into place. Any failure removes the temporary directory and leaves
//! the target untouched.
use crate::plan::{GIT_URL, OssiframeSource, ResolvedModule};
use crate::render::{self, Tokens};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    #[error("{0}")]
    Msg(String),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    Render {
        path: String,
        source: render::RenderError,
    },
}

fn msg<T>(s: impl Into<String>) -> Result<T, WriteError> {
    Err(WriteError::Msg(s.into()))
}

trait IoCtx<T> {
    fn at(self, p: &Path) -> Result<T, WriteError>;
}
impl<T> IoCtx<T> for std::io::Result<T> {
    fn at(self, p: &Path) -> Result<T, WriteError> {
        self.map_err(|source| WriteError::Io {
            path: p.to_path_buf(),
            source,
        })
    }
}

/// Everything the writer needs, already validated.
pub struct Job<'a> {
    pub repo_root: &'a Path,
    pub target: &'a Path,
    pub source: &'a OssiframeSource,
    pub modules: &'a [ResolvedModule],
    pub tokens: &'a Tokens,
}

fn skipped(name: &str) -> bool {
    matches!(
        name,
        "node_modules" | "dist" | ".vite" | "target" | ".DS_Store"
    ) || name.ends_with(".tsbuildinfo")
}

/// Copies a tree. With `tokens` text files are rendered; otherwise bytes are
/// copied as they are. Symlinks are refused. `skip_top` names are skipped at
/// the top level only.
fn copy_tree(
    src: &Path,
    dst: &Path,
    tokens: Option<&Tokens>,
    skip_top: &[&str],
) -> Result<(), WriteError> {
    fs::create_dir_all(dst).at(dst)?;
    for entry in fs::read_dir(src).at(src)? {
        let entry = entry.at(src)?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return msg(format!("non UTF-8 file name under {}", src.display()));
        };
        if skipped(name) || skip_top.contains(&name) {
            continue;
        }
        let from = entry.path();
        // Template manifests holding tokens are not valid TOML, so the
        // template stores them as `Cargo.toml.tmpl` (Cargo would scan them in
        // a git checkout). Rendered output gets the real name.
        let out_name = match (tokens, name) {
            (Some(_), "Cargo.toml.tmpl") => "Cargo.toml",
            _ => name,
        };
        let to = dst.join(out_name);
        let ft = entry.file_type().at(&from)?;
        if ft.is_symlink() {
            return msg(format!("refusing to copy symlink {}", from.display()));
        }
        if ft.is_dir() {
            copy_tree(&from, &to, tokens, &[])?;
        } else {
            let bytes = fs::read(&from).at(&from)?;
            let out = match tokens {
                Some(t) => {
                    render::render_bytes(&bytes, t).map_err(|source| WriteError::Render {
                        path: from.display().to_string(),
                        source,
                    })?
                }
                None => bytes,
            };
            fs::write(&to, out).at(&to)?;
        }
    }
    Ok(())
}

/// Reads `key = "value"` pairs from the `[workspace.package]` section.
fn workspace_package(root_manifest: &str) -> Vec<(String, String)> {
    let mut in_sec = false;
    let mut out = Vec::new();
    for line in root_manifest.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_sec = t == "[workspace.package]";
        } else if in_sec && let Some((k, v)) = t.split_once('=') {
            out.push((k.trim().to_owned(), v.trim().trim_matches('"').to_owned()));
        }
    }
    out
}

/// Makes the example crate's manifest standalone: concrete package fields in
/// place of workspace inheritance, `ossiframe-core` from the app's source, and
/// its own workspace root so no parent workspace is searched.
pub fn standalone_example_manifest(
    manifest: &str,
    workspace_pkg: &[(String, String)],
    source: &OssiframeSource,
) -> Result<String, WriteError> {
    let mut out = String::new();
    for line in manifest.lines() {
        let t = line.trim();
        if let Some(key) = t
            .strip_suffix(".workspace = true")
            .or_else(|| t.strip_suffix(".workspace=true"))
        {
            let key = key.trim();
            let Some((_, v)) = workspace_pkg.iter().find(|(k, _)| k == key) else {
                return msg(format!("example manifest inherits unknown field '{key}'"));
            };
            out.push_str(&format!("{key} = \"{v}\"\n"));
        } else if t.starts_with("ossiframe-core") && t.contains('=') {
            out.push_str(&render::dep_line(
                "ossiframe-core",
                source,
                "crates/ossiframe-core",
                "",
            ));
            out.push('\n');
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str("\n[workspace]\n");
    Ok(out)
}

/// Fails if a generated manifest inherits from a workspace, or (git mode) has
/// a `path` dependency other than `../modules/...` inside the app.
pub fn check_manifest(rel: &str, text: &str, source: &OssiframeSource) -> Result<(), WriteError> {
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('#') {
            continue;
        }
        let squeezed: String = t.chars().filter(|c| !c.is_whitespace()).collect();
        if squeezed.contains("workspace=true") {
            return msg(format!(
                "{rel}: workspace inheritance in generated manifest"
            ));
        }
        if matches!(source, OssiframeSource::Git { .. }) {
            let mut rest = t;
            while let Some(i) = rest.find("path") {
                let before_ok = rest[..i]
                    .chars()
                    .next_back()
                    .is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == '-'));
                let after = rest[i + 4..].trim_start();
                if before_ok && let Some(val) = after.strip_prefix('=') {
                    let val = val.trim_start();
                    if !val.starts_with("\"../modules/") {
                        return msg(format!(
                            "{rel}: standalone manifest has a path dependency outside ../modules/"
                        ));
                    }
                }
                rest = &rest[i + 4..];
            }
        }
    }
    Ok(())
}

fn find_manifests(dir: &Path, base: &Path, out: &mut Vec<PathBuf>) -> Result<(), WriteError> {
    for e in fs::read_dir(dir).at(dir)? {
        let e = e.at(dir)?;
        let p = e.path();
        let name = e.file_name();
        if e.file_type().at(&p)?.is_dir() {
            if name != "node_modules" && name != "packages" {
                find_manifests(&p, base, out)?;
            }
        } else if name == "Cargo.toml" {
            out.push(p.strip_prefix(base).unwrap_or(&p).to_path_buf());
        }
    }
    Ok(())
}

fn build(job: &Job, tmp: &Path) -> Result<(), WriteError> {
    let root = job.repo_root;
    // 1. Starter template.
    let starter = root.join("templates/starter");
    if !starter.is_dir() {
        return msg(format!("template not found at {}", starter.display()));
    }
    copy_tree(&starter, tmp, Some(job.tokens), &[])?;
    // 2. Vendored shared packages.
    for pkg in ["ui", "client"] {
        let from = root.join("packages").join(pkg);
        if !from.is_dir() {
            return msg(format!("package not found at {}", from.display()));
        }
        copy_tree(&from, &tmp.join("packages").join(pkg), None, &[])?;
    }
    // 3. Module UI folders and the example crate.
    for m in job.modules {
        let id = m.id().as_str();
        if let Some(ui) = m.entry.ui_dir {
            let from = root.join(ui);
            if !from.is_dir() {
                return msg(format!("module ui not found at {}", from.display()));
            }
            copy_tree(&from, &tmp.join("src/modules").join(id), None, &[])?;
        }
        if m.entry.is_example {
            let from = root.join(m.entry.crate_dir);
            let to = tmp.join("modules").join(id);
            copy_tree(&from, &to, None, &["ui", "Cargo.toml"])?;
            let manifest = fs::read_to_string(from.join("Cargo.toml")).at(&from)?;
            let root_manifest = fs::read_to_string(root.join("Cargo.toml")).at(root)?;
            let text = standalone_example_manifest(
                &manifest,
                &workspace_package(&root_manifest),
                job.source,
            )?;
            fs::write(to.join("Cargo.toml"), text).at(&to)?;
        }
    }
    // 4. Mode notice in the README.
    let readme = tmp.join("README.md");
    if readme.is_file() {
        let notice = match job.source {
            OssiframeSource::Local { root } => format!(
                "\n> **Local testing only; remove before sharing.** This app was generated with `--ossiframe-local` and its Ossiframe dependencies are `path` entries into `{}`.\n",
                root.display()
            ),
            OssiframeSource::Git { rev } => format!(
                "\n> Ossiframe is pinned to commit `{rev}` of {GIT_URL}. Standalone builds work only if that commit is published there.\n"
            ),
        };
        let mut text = fs::read_to_string(&readme).at(&readme)?;
        text.push_str(&notice);
        fs::write(&readme, text).at(&readme)?;
    }
    // 5. Manifest checks.
    let mut manifests = Vec::new();
    find_manifests(tmp, tmp, &mut manifests)?;
    for rel in manifests {
        let text = fs::read_to_string(tmp.join(&rel)).at(&rel)?;
        check_manifest(&rel.display().to_string(), &text, job.source)?;
    }
    Ok(())
}

/// Renders into `<parent>/.<name>.ossiframe-tmp-<pid>` and renames it to the
/// target. The target must be absent or an empty directory.
pub fn generate(job: &Job) -> Result<(), WriteError> {
    let target = job.target;
    let Some(name) = target.file_name().and_then(|n| n.to_str()) else {
        return msg("target directory needs a plain final path component");
    };
    let parent = match target.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    fs::create_dir_all(&parent).at(&parent)?;
    let tmp = parent.join(format!(".{name}.ossiframe-tmp-{}", std::process::id()));
    fs::create_dir(&tmp).at(&tmp)?;
    let result = build(job, &tmp).and_then(|()| {
        // Re-check right before moving: absent or empty only.
        crate::validate::check_target_dir(target).map_err(|e| WriteError::Msg(e.to_string()))?;
        if target.is_dir() {
            fs::remove_dir(target).at(target)?;
        }
        fs::rename(&tmp, target).at(target)
    });
    if result.is_err() {
        let _ = fs::remove_dir_all(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git() -> OssiframeSource {
        OssiframeSource::Git {
            rev: "a".repeat(40),
        }
    }

    #[test]
    fn standalone_check_allows_only_modules_paths() {
        let ok = "x = { path = \"../modules/ports\" }\n[workspace]\n";
        assert!(check_manifest("c", ok, &git()).is_ok());
        for bad in [
            "x = { path = \"/abs/crates/x\" }",
            "x = { path = \"../../crates/x\" }",
            "x = {path=\"../crates/x\"}",
            "version.workspace = true",
            "x = { workspace = true }",
        ] {
            assert!(check_manifest("c", bad, &git()).is_err(), "{bad}");
        }
        // Local mode may use absolute paths but still not inherit.
        let local = OssiframeSource::Local { root: "/w".into() };
        assert!(check_manifest("c", "x = { path = \"/w/crates/x\" }", &local).is_ok());
        assert!(check_manifest("c", "edition.workspace = true", &local).is_err());
    }

    #[test]
    fn example_manifest_becomes_standalone() {
        let m = "[package]\nname = \"p\"\nversion.workspace = true\nedition.workspace = true\n[dependencies]\nossiframe-core = { version = \"0.1.0\", path = \"../../crates/ossiframe-core\" }\n";
        let ws = vec![
            ("version".to_owned(), "0.1.0".to_owned()),
            ("edition".to_owned(), "2024".to_owned()),
        ];
        let out = standalone_example_manifest(m, &ws, &git()).unwrap();
        assert!(out.contains("version = \"0.1.0\"") && out.contains("edition = \"2024\""));
        assert!(out.contains("ossiframe-core = { git = "), "{out}");
        assert!(
            !out.contains("path") && !out.contains(".workspace"),
            "{out}"
        );
        check_manifest("c", &out, &git()).unwrap();
    }

    #[test]
    fn failure_leaves_no_target_and_no_temp_dir() {
        let base = std::env::temp_dir().join(format!("ossiframe-cli-write-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let fake_repo = base.join("repo"); // has no templates/starter
        fs::create_dir_all(&fake_repo).unwrap();
        let target = base.join("out/app");
        let tokens = Tokens::default();
        let src = git();
        let job = Job {
            repo_root: &fake_repo,
            target: &target,
            source: &src,
            modules: &[],
            tokens: &tokens,
        };
        assert!(generate(&job).is_err());
        assert!(!target.exists());
        let left: Vec<_> = fs::read_dir(base.join("out")).unwrap().collect();
        assert!(left.is_empty(), "temp dir left behind: {left:?}");
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn real_generation_in_git_mode_is_complete_and_standalone() {
        use crate::plan::{catalog, resolve_modules};
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let base = std::env::temp_dir().join(format!("ossiframe-cli-gen-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let src = git();
        let ids = [ossiframe_core::ModuleId::parse("linux-inventory").unwrap()];
        let modules = resolve_modules(&catalog(), &ids, true).unwrap();
        let tokens = render::starter_tokens(
            "Acme Ops",
            "dev.example.acme",
            "acme-ops",
            "acme_ops",
            &src,
            &modules,
        );
        let target = base.join("app");
        let job = Job {
            repo_root: &root,
            target: &target,
            source: &src,
            modules: &modules,
            tokens: &tokens,
        };
        generate(&job).unwrap();
        assert!(target.join("packages/ui/src/index.ts").is_file());
        assert!(!target.join("packages/ui/node_modules").exists());
        assert!(
            target
                .join("src/modules/linux-inventory/index.ts")
                .is_file()
        );
        assert!(target.join("src/modules/ports/index.ts").is_file());
        assert!(target.join("src-tauri/Cargo.toml").is_file());
        assert!(!target.join("src-tauri/Cargo.toml.tmpl").exists());
        assert!(target.join("modules/ports/src/lib.rs").is_file());
        assert!(!target.join("modules/ports/ui").exists());
        // Generated CSP lines equal Doctor's.
        let csp = |p: &Path| -> Vec<String> {
            fs::read_to_string(p)
                .unwrap()
                .lines()
                .filter(|l| l.contains("\"csp\"") || l.contains("\"devCsp\""))
                .map(|l| l.trim().to_owned())
                .collect()
        };
        let doctor = csp(&root.join("apps/doctor/src-tauri/tauri.conf.json"));
        assert_eq!(doctor.len(), 2);
        assert_eq!(doctor, csp(&target.join("src-tauri/tauri.conf.json")));
        // Non-empty target is refused and nothing else is written next to it.
        assert!(generate(&job).is_err());
        assert_eq!(fs::read_dir(&base).unwrap().count(), 1);
        fs::remove_dir_all(&base).unwrap();
    }
}
