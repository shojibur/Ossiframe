//! Plain `{{key}}` token renderer for the starter template.
//!
//! Inline tokens are replaced in place. Whole-line tokens (alone on a line)
//! are replaced by value lines indented like the token; an empty value drops
//! the line. Unknown tokens and any `{{` left after substitution are errors.
use crate::plan::{GIT_URL, OssiframeSource, ResolvedModule};
use std::collections::BTreeMap;

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum RenderError {
    #[error("unknown template token '{{{{{0}}}}}'")]
    UnknownToken(String),
    #[error("leftover '{{{{' in rendered output")]
    Leftover,
}

#[derive(Debug, Default)]
pub struct Tokens {
    inline: BTreeMap<&'static str, String>,
    lines: BTreeMap<&'static str, Vec<String>>,
}

impl Tokens {
    pub fn inline(mut self, key: &'static str, value: impl Into<String>) -> Self {
        self.inline.insert(key, value.into());
        self
    }
    pub fn lines(mut self, key: &'static str, value: Vec<String>) -> Self {
        self.lines.insert(key, value);
        self
    }
}

pub fn render(text: &str, tokens: &Tokens) -> Result<String, RenderError> {
    let mut out = String::with_capacity(text.len());
    for raw in text.split_inclusive('\n') {
        let (line, eol) = match raw.strip_suffix("\r\n") {
            Some(l) => (l, "\r\n"),
            None => match raw.strip_suffix('\n') {
                Some(l) => (l, "\n"),
                None => (raw, ""),
            },
        };
        let trimmed = line.trim();
        let whole = trimmed
            .strip_prefix("{{")
            .and_then(|r| r.strip_suffix("}}"))
            .and_then(|k| tokens.lines.get(k));
        if let Some(values) = whole {
            let indent = &line[..line.len() - line.trim_start().len()];
            for v in values {
                out.push_str(indent);
                out.push_str(v);
                out.push_str(if eol.is_empty() { "\n" } else { eol });
            }
            continue;
        }
        out.push_str(&inline(line, tokens)?);
        out.push_str(eol);
    }
    if out.contains("{{") {
        return Err(RenderError::Leftover);
    }
    Ok(out)
}

fn inline(line: &str, tokens: &Tokens) -> Result<String, RenderError> {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            return Err(RenderError::Leftover);
        };
        let key = &after[..end];
        match tokens.inline.get(key) {
            Some(v) => out.push_str(v),
            None => return Err(RenderError::UnknownToken(key.to_owned())),
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Binary (non UTF-8) content is returned unchanged.
pub fn render_bytes(bytes: &[u8], tokens: &Tokens) -> Result<Vec<u8>, RenderError> {
    match std::str::from_utf8(bytes) {
        Ok(text) => render(text, tokens).map(String::into_bytes),
        Err(_) => Ok(bytes.to_vec()),
    }
}

/// TOML basic string. Callers guarantee no control characters.
fn toml_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

pub fn dep_line(name: &str, source: &OssiframeSource, repo_dir: &str, extra: &str) -> String {
    let src = match source {
        OssiframeSource::Git { rev } => {
            format!("git = {}, rev = {}", toml_str(GIT_URL), toml_str(rev))
        }
        OssiframeSource::Local { root } => {
            let p = root.join(repo_dir);
            format!("path = {}", toml_str(&p.to_string_lossy()))
        }
    };
    format!("{name} = {{ {src}{extra} }}")
}

/// Command names the provisioning opt-in adds (kept equal to
/// `ossiframe_desktop::provisioning::PROVISION_COMMAND_NAMES` by a test).
const PROVISION_COMMANDS: [&str; 10] = [
    "provision_components",
    "provision_preflight",
    "provision_create_plan",
    "provision_approve_plan",
    "provision_cancel",
    "provision_reconcile",
    "provision_mark_resolved",
    "provision_release_stale_lock",
    "provision_get_operation",
    "provision_list_operations",
];

/// The template tokens, filled from validated inputs. The provisioning tokens
/// are empty unless a privileged module is selected (the caller has already
/// checked `--enable-provisioning`), so the default output has none of them.
#[allow(clippy::too_many_lines)]
pub fn starter_tokens(
    product_name: &str,
    identifier: &str,
    package_name: &str,
    lib_name: &str,
    source: &OssiframeSource,
    modules: &[ResolvedModule],
) -> Tokens {
    let prov = modules.iter().find(|m| m.entry.privileged);
    let on = prov.is_some();
    let ossiframe_deps = vec![
        dep_line("ossiframe-core", source, "crates/ossiframe-core", ""),
        dep_line(
            "ossiframe-desktop",
            source,
            "crates/ossiframe-desktop",
            if on {
                ", features = [\"tauri\", \"provisioning\"]"
            } else {
                ", features = [\"tauri\"]"
            },
        ),
    ];
    let module_deps = modules
        .iter()
        .map(|m| {
            if m.entry.is_example {
                // Copied into the generated app, in both modes.
                format!(
                    "{} = {{ path = \"../modules/{}\" }}",
                    m.entry.crate_name,
                    m.id()
                )
            } else {
                dep_line(m.entry.crate_name, source, m.entry.crate_dir, "")
            }
        })
        .collect();
    let with_ui = || modules.iter().filter(|m| m.entry.ui_dir.is_some());
    let when = |lines: Vec<String>| if on { lines } else { Vec::new() };
    let names = || PROVISION_COMMANDS.iter();
    let last = PROVISION_COMMANDS.len() - 1;
    Tokens::default()
        .inline("product_name", product_name)
        .inline("identifier", identifier)
        .inline("package_name", package_name)
        .inline("lib_name", lib_name)
        .inline("setup_try", if on { "?;" } else { "" })
        .inline(
            "command_names_iter",
            if on {
                "COMMAND_NAMES.iter().chain(PROVISION_COMMAND_NAMES)"
            } else {
                "COMMAND_NAMES.iter()"
            },
        )
        .inline(
            "command_names_loop",
            if on {
                "COMMAND_NAMES.iter().chain(PROVISION_COMMAND_NAMES)"
            } else {
                "COMMAND_NAMES"
            },
        )
        .inline(
            "command_names_len",
            if on {
                "COMMAND_NAMES.len() + PROVISION_COMMAND_NAMES.len()"
            } else {
                "COMMAND_NAMES.len()"
            },
        )
        .inline("permissions_comma", if on { "," } else { "" })
        .lines(
            "provision_uses",
            when(vec![
                "use ossiframe_desktop::provisioning::{self, commands as provision};".to_owned(),
                "use std::sync::Arc;".to_owned(),
            ]),
        )
        .lines(
            "provision_setup",
            when(vec![format!(
                "provisioning::setup(app, Arc::new({}))",
                prov.map_or("", |m| m.entry.type_path)
            )]),
        )
        .lines(
            "provision_handlers",
            when(names().map(|n| format!("provision::{n},")).collect()),
        )
        .lines(
            "provision_build_names",
            when(names().map(|n| format!("\"{n}\",")).collect()),
        )
        .lines(
            "provision_permissions",
            when(
                names()
                    .enumerate()
                    .map(|(i, n)| {
                        let sep = if i == last { "" } else { "," };
                        format!("\"allow-{}\"{sep}", n.replace('_', "-"))
                    })
                    .collect(),
            ),
        )
        .lines(
            "provision_test_uses",
            when(vec![
                "use ossiframe_desktop::provisioning::PROVISION_COMMAND_NAMES;".to_owned(),
            ]),
        )
        .lines("ossiframe_deps", ossiframe_deps)
        .lines("module_deps", module_deps)
        .lines(
            "module_registrations",
            modules
                .iter()
                .map(|m| format!(".register({})", m.entry.type_path))
                .collect(),
        )
        .lines(
            "module_ui_imports",
            with_ui()
                .map(|m| format!("import * as {} from \"./modules/{}\";", m.ui_alias, m.id()))
                .collect(),
        )
        .lines(
            "module_ui_list",
            with_ui().map(|m| format!("{},", m.ui_alias)).collect(),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{catalog, resolve_modules};
    use ossiframe_core::ModuleId;
    use std::path::PathBuf;

    fn t() -> Tokens {
        Tokens::default()
            .inline("a", "X")
            .lines("many", vec!["l1".into(), "l2".into()])
            .lines("none", vec![])
    }

    #[test]
    fn inline_replaces_all_occurrences() {
        assert_eq!(render("{{a}}-{{a}}\n", &t()).unwrap(), "X-X\n");
    }

    #[test]
    fn whole_line_is_indented_and_empty_drops_line() {
        let src = "k {\n    {{many}}\n  {{none}}\n}\n";
        assert_eq!(render(src, &t()).unwrap(), "k {\n    l1\n    l2\n}\n");
    }

    #[test]
    fn crlf_is_preserved() {
        assert_eq!(render("  {{many}}\r\n", &t()).unwrap(), "  l1\r\n  l2\r\n");
    }

    #[test]
    fn unknown_and_leftover_tokens_are_rejected() {
        assert_eq!(
            render("{{nope}}", &t()).unwrap_err(),
            RenderError::UnknownToken("nope".into())
        );
        assert_eq!(render("a {{ b", &t()).unwrap_err(), RenderError::Leftover);
        // A whole-line key used inline is not an inline token.
        assert!(render("x {{many}}", &t()).is_err());
        // Values are not re-scanned, but a value that brings `{{` is caught.
        let bad = Tokens::default().inline("a", "{{b");
        assert_eq!(render("{{a}}", &bad).unwrap_err(), RenderError::Leftover);
    }

    #[test]
    fn provisioning_tokens_are_empty_without_the_module_and_filled_with_it() {
        let src = OssiframeSource::Git {
            rev: "e".repeat(40),
        };
        let probe = "{{ossiframe_deps}}\n{{provision_uses}}\n{{provision_setup}}\n{{provision_handlers}}\n{{provision_build_names}}\n{{provision_permissions}}\n{{provision_test_uses}}\n[{{setup_try}}{{permissions_comma}}{{command_names_len}}]\n";
        let off = render(probe, &tokens_for(&src, &["linux-inventory"], false)).unwrap();
        assert!(!off.contains("provision"), "{off}");
        assert!(off.contains("features = [\"tauri\"]") && off.ends_with("[COMMAND_NAMES.len()]\n"));
        let on = render(probe, &tokens_for(&src, &["ubuntu-provisioning"], false)).unwrap();
        assert!(
            on.contains("features = [\"tauri\", \"provisioning\"]"),
            "{on}"
        );
        assert!(on.contains("provisioning::setup(app, Arc::new(ossiframe_mod_ubuntu_provisioning::UbuntuProvisioning))"));
        for n in PROVISION_COMMANDS {
            let perm = format!("\"allow-{}\"", n.replace('_', "-"));
            for needle in [format!("provision::{n},"), format!("\"{n}\","), perm] {
                assert!(on.contains(&needle), "missing {needle}");
            }
        }
        assert!(
            on.contains("\"allow-provision-list-operations\"\n"),
            "last has no comma"
        );
    }

    #[test]
    fn binary_is_copied_unchanged() {
        let b = [0xff, 0xfe, b'{', b'{'];
        assert_eq!(render_bytes(&b, &t()).unwrap(), b);
    }

    fn tokens_for(source: &OssiframeSource, req: &[&str], example: bool) -> Tokens {
        let ids: Vec<ModuleId> = req.iter().map(|s| ModuleId::parse(s).unwrap()).collect();
        let m = resolve_modules(&catalog(), &ids, example).unwrap();
        starter_tokens(
            "Acme Ops",
            "dev.example.acme",
            "acme-ops",
            "acme_ops",
            source,
            &m,
        )
    }

    #[test]
    fn git_mode_uses_one_rev_and_no_paths_outside_the_app() {
        let rev = "a".repeat(40);
        let src = OssiframeSource::Git { rev: rev.clone() };
        let tk = tokens_for(&src, &["linux-inventory"], true);
        let out = render(
            "{{ossiframe_deps}}\n{{module_deps}}\n{{module_registrations}}\n",
            &tk,
        )
        .unwrap();
        assert_eq!(out.matches(&format!("rev = \"{rev}\"")).count(), 3);
        assert!(out.contains(
            "ossiframe-desktop = { git = \"https://github.com/shojibur/Ossiframe\", rev"
        ));
        assert!(out.contains("features = [\"tauri\"]"));
        assert!(out.contains("ossiframe-mod-ports-example = { path = \"../modules/ports\" }"));
        assert_eq!(out.matches("path =").count(), 1, "{out}");
    }

    #[test]
    fn linux_audit_deps_in_both_modes_and_no_ui_tokens() {
        let git = OssiframeSource::Git {
            rev: "d".repeat(40),
        };
        let out = render(
            "{{module_deps}}\n{{module_ui_list}}\n",
            &tokens_for(&git, &["linux-audit"], false),
        )
        .unwrap();
        assert!(
            out.contains("ossiframe-mod-linux-inventory = { git = "),
            "{out}"
        );
        assert!(
            out.contains("ossiframe-mod-linux-audit = { git = "),
            "{out}"
        );
        assert!(out.find("linux-inventory").unwrap() < out.find("linux-audit").unwrap());
        assert!(out.contains("ui_linux_inventory,") && !out.contains("ui_linux_audit"));
        let local = OssiframeSource::Local {
            root: PathBuf::from("/work/Ossiframe"),
        };
        let out = render(
            "{{module_deps}}\n",
            &tokens_for(&local, &["linux-audit"], false),
        )
        .unwrap();
        assert!(out.contains("ossiframe-mod-linux-inventory = { path = \"/work/Ossiframe/modules/linux-inventory\" }"), "{out}");
        assert!(
            out.contains(
                "ossiframe-mod-linux-audit = { path = \"/work/Ossiframe/modules/linux-audit\" }"
            ),
            "{out}"
        );
    }

    #[test]
    fn local_mode_uses_paths_and_no_git_or_patch() {
        let src = OssiframeSource::Local {
            root: PathBuf::from("/work/Ossiframe"),
        };
        let tk = tokens_for(&src, &["linux-inventory"], false);
        let out = render("{{ossiframe_deps}}\n{{module_deps}}\n", &tk).unwrap();
        assert!(
            out.contains("ossiframe-core = { path = \"/work/Ossiframe/crates/ossiframe-core\" }")
        );
        assert!(out.contains(
            "ossiframe-mod-linux-inventory = { path = \"/work/Ossiframe/modules/linux-inventory\" }"
        ));
        assert!(!out.contains("git") && !out.contains("patch"), "{out}");
    }

    #[test]
    fn module_ui_tokens_follow_resolved_order_and_zero_modules_render_empty() {
        let src = OssiframeSource::Git {
            rev: "b".repeat(40),
        };
        let tk = tokens_for(&src, &["linux-inventory"], true);
        let out = render("  {{module_ui_imports}}\n[\n  {{module_ui_list}}\n]\n", &tk).unwrap();
        assert_eq!(
            out,
            "  import * as ui_linux_inventory from \"./modules/linux-inventory\";\n  import * as ui_ports from \"./modules/ports\";\n[\n  ui_linux_inventory,\n  ui_ports,\n]\n"
        );
        let tk = tokens_for(&src, &[], false);
        assert_eq!(
            render("a\n  {{module_registrations}}\n  {{module_deps}}\nb\n", &tk).unwrap(),
            "a\nb\n"
        );
    }

    #[test]
    fn real_starter_renders_with_no_leftover_tokens() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../templates/starter");
        let src = OssiframeSource::Git {
            rev: "c".repeat(40),
        };
        let tk = tokens_for(&src, &["linux-inventory"], true);
        for rel in [
            "src-tauri/Cargo.toml.tmpl",
            "src-tauri/src/lib.rs",
            "src/modules.ts",
            "src-tauri/tauri.conf.json",
            "README.md",
            "package.json",
            "index.html",
        ] {
            let text = std::fs::read_to_string(root.join(rel)).unwrap();
            render(&text, &tk).unwrap_or_else(|e| panic!("{rel}: {e}"));
        }
    }
}
