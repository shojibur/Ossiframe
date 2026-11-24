//! Repository-local project generator for Ossiframe apps.
//!
//! `new` validates inputs, resolves modules and the Ossiframe source, renders
//! the starter template into a temporary sibling directory and moves it into
//! place.

mod plan;
mod render;
mod validate;
mod write;

use clap::{Args, Parser, Subcommand};
use ossiframe_core::ModuleId;
use std::path::PathBuf;
use std::process::ExitCode;
use validate::{AppIdentifier, InputError, ProductName};

#[derive(Parser)]
#[command(name = "ossiframe-cli", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Generate a new Ossiframe app in <DIR> (absent or empty directory).
    New(NewArgs),
}

#[derive(Args)]
struct NewArgs {
    /// Target directory. Must be absent or empty.
    dir: PathBuf,
    /// Product name, for example "Acme Ops".
    #[arg(long)]
    name: String,
    /// Reverse-DNS app identifier, for example dev.example.acme.
    #[arg(long)]
    identifier: String,
    /// Optional modules to include (comma separated). None by default.
    #[arg(long, value_delimiter = ',')]
    modules: Vec<String>,
    /// Include the example `ports` module as a local module crate.
    #[arg(long)]
    with_example_module: bool,
    /// Ossiframe commit to pin (40-hex). Defaults to the local HEAD.
    #[arg(long, conflicts_with = "ossiframe_local")]
    rev: Option<String>,
    /// Use plain `path` dependencies on a local Ossiframe checkout (testing only).
    #[arg(long, value_name = "PATH")]
    ossiframe_local: Option<PathBuf>,
    /// Allow identifiers under dev.ossiframe.*.
    #[arg(long)]
    allow_ossiframe_namespace: bool,
}

/// Validated inputs of `new`. Nothing here has touched the file system yet,
/// except the read-only check of the target directory.
#[derive(Debug)]
struct NewRequest {
    dir: PathBuf,
    name: ProductName,
    package_name: String,
    lib_name: String,
    identifier: AppIdentifier,
    modules: Vec<ModuleId>,
    with_example_module: bool,
    rev: Option<String>,
    ossiframe_local: Option<PathBuf>,
}

fn validate_new(a: NewArgs) -> Result<NewRequest, InputError> {
    let name = ProductName::parse(&a.name)?;
    let package_name = name.package_name()?;
    let lib_name = name.lib_name()?;
    let identifier = AppIdentifier::parse(&a.identifier, a.allow_ossiframe_namespace)?;
    let modules = validate::parse_modules(&a.modules)?;
    let rev = a.rev.as_deref().map(validate::parse_rev).transpose()?;
    let dir = validate::check_target_dir(&a.dir)?;
    Ok(NewRequest {
        dir,
        name,
        package_name,
        lib_name,
        identifier,
        modules,
        with_example_module: a.with_example_module,
        rev,
        ossiframe_local: a.ossiframe_local,
    })
}

/// Everything task 10.3 needs to write the app: resolved modules, the
/// Ossiframe source and the rendering tokens. Reads git, writes nothing.
struct Prepared {
    modules: Vec<plan::ResolvedModule>,
    source: plan::OssiframeSource,
    tokens: render::Tokens,
}

fn repo_root() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn next_steps(dir: &std::path::Path, source: &plan::OssiframeSource) -> String {
    let mut s = format!(
        "Generated {}\n\nNext:\n  cd {}\n  pnpm install\n  pnpm tauri dev\n",
        dir.display(),
        shell_quote(&dir.to_string_lossy())
    );
    match source {
        plan::OssiframeSource::Local { .. } => s.push_str(
            "\nWarning: generated with --ossiframe-local (Local_Mode). Cargo.toml has path\ndependencies into your checkout: local testing only, remove before sharing.\n",
        ),
        plan::OssiframeSource::Git { rev } => s.push_str(&format!(
            "\nOssiframe is pinned to {rev}. Standalone builds need that commit to be\npublished at {}.\n",
            plan::GIT_URL
        )),
    }
    s
}

/// Single-quotes a path for display in a copy-paste command.
fn shell_quote(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-~".contains(c))
    {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

fn prepare(req: &NewRequest) -> Result<Prepared, plan::PlanError> {
    let modules = plan::resolve_modules(&plan::catalog(), &req.modules, req.with_example_module)?;
    let repo = repo_root();
    let source = plan::pick_source(
        req.rev.as_deref(),
        req.ossiframe_local.as_deref(),
        &plan::RepoGit(repo),
    )?;
    let tokens = render::starter_tokens(
        req.name.as_str(),
        req.identifier.as_str(),
        &req.package_name,
        &req.lib_name,
        &source,
        &modules,
    );
    Ok(Prepared {
        modules,
        source,
        tokens,
    })
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::New(args) => match validate_new(args) {
            Ok(req) => match prepare(&req) {
                Ok(p) => {
                    let job = write::Job {
                        repo_root: &repo_root(),
                        target: &req.dir,
                        source: &p.source,
                        modules: &p.modules,
                        tokens: &p.tokens,
                    };
                    match write::generate(&job) {
                        Ok(()) => {
                            println!("{}", next_steps(&req.dir, &p.source));
                            ExitCode::SUCCESS
                        }
                        Err(e) => {
                            eprintln!("error: {e}");
                            ExitCode::FAILURE
                        }
                    }
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            },
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("ossiframe-cli").chain(args.iter().copied()))
    }

    #[test]
    fn cli_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn rev_and_local_are_mutually_exclusive() {
        let rev = "a".repeat(40);
        assert!(
            parse(&[
                "new",
                "d",
                "--name",
                "A",
                "--identifier",
                "a.b",
                "--rev",
                &rev,
                "--ossiframe-local",
                "/x"
            ])
            .is_err()
        );
    }

    #[test]
    fn zero_modules_is_valid_and_modules_split_on_comma() {
        let Command::New(a) = parse(&["new", "d", "--name", "A", "--identifier", "a.b"])
            .unwrap()
            .command;
        assert!(a.modules.is_empty());
        let Command::New(a) = parse(&[
            "new",
            "d",
            "--name",
            "A",
            "--identifier",
            "a.b",
            "--modules",
            "x,y",
        ])
        .unwrap()
        .command;
        assert_eq!(a.modules, ["x", "y"]);
    }

    #[test]
    fn validation_refuses_non_empty_dir() {
        let dir = std::env::temp_dir().join(format!("ossiframe-cli-new-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f"), "x").unwrap();
        let args = |d: &PathBuf| NewArgs {
            dir: d.clone(),
            name: "Acme Ops".into(),
            identifier: "dev.example.acme".into(),
            modules: vec![],
            with_example_module: false,
            rev: None,
            ossiframe_local: None,
            allow_ossiframe_namespace: false,
        };
        assert!(validate_new(args(&dir)).is_err());
        let fresh = dir.join("sub");
        let req = validate_new(args(&fresh)).unwrap();
        assert_eq!(req.package_name, "acme-ops");
        assert!(!fresh.exists(), "validation must not write");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
