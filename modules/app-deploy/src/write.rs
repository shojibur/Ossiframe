//! Content-write helper: fixed `install` (content on stdin) then `mv -T`.
//!
//! Two modes, kept apart by the types they accept:
//! - [`root_write`]: root writes to an Ossiframe-managed path under `/etc`.
//!   The parent directory is root-owned, so app code cannot plant a symlink
//!   at the temp name or destination.
//! - [`app_write`]: writes inside an app directory run as the app user, never
//!   root, with no `-o`/`-g`, so a symlink planted by app code cannot redirect
//!   a privileged write.
//!
//! GNU `install` without `-D` removes an existing destination before
//! creating it and never writes through a symlink at the destination; `-T`
//! treats the destination as a file. `mv -T` renames atomically within one
//! directory (the temp name sits beside the final name).

use crate::identity::app_slug;
use crate::paths::{APPS_ROOT, AppPath, AppRoot, BAK_SUFFIX, ManagedKind, ManagedPath, TMP_SUFFIX};
use ossiframe_core::{
    CancelToken, CommandSpec, ExecError, Executor, ExitStatus, LinuxUserName, PlanStep, Privilege,
    StepMode, ValidationError,
};
use std::time::Duration;

pub(crate) const INSTALL: &str = "/usr/bin/install";
pub(crate) const MV: &str = "/usr/bin/mv";
const CP: &str = "/usr/bin/cp";
const STEP_TIMEOUT: Duration = Duration::from_secs(60);

/// File modes the module writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileMode {
    /// Configuration readable by all (NGINX, FPM).
    M0644,
    /// Owner-only secrets (`.env`, `known_hosts`).
    M0600,
}

impl FileMode {
    fn as_str(self) -> &'static str {
        match self {
            FileMode::M0644 => "0644",
            FileMode::M0600 => "0600",
        }
    }
}

fn step(id: &str, title: String, command: CommandSpec, requires_stdin: bool) -> PlanStep {
    PlanStep {
        id: id.to_owned(),
        title,
        command,
        mode: StepMode::Foreground,
        deadline: STEP_TIMEOUT,
        requires_stdin,
    }
}

fn install_then_move(
    id: &str,
    final_path: &str,
    mode: FileMode,
    content: Vec<u8>,
    root_owned: bool,
    user: Option<&LinuxUserName>,
) -> Result<Vec<PlanStep>, ValidationError> {
    let tmp = format!("{final_path}{TMP_SUFFIX}");
    let mut install_args = vec!["-m", mode.as_str()];
    if root_owned {
        install_args.extend(["-o", "root", "-g", "root"]);
    }
    install_args.extend(["-T", "--", "/dev/stdin", &tmp]);
    let mv_args = ["-T", "--", &tmp, final_path];
    let (install, mv) = match user {
        Some(u) => (
            CommandSpec::run_as(u.clone(), INSTALL, &install_args, STEP_TIMEOUT)?,
            CommandSpec::run_as(u.clone(), MV, &mv_args, STEP_TIMEOUT)?,
        ),
        None => (
            CommandSpec::privileged(INSTALL, &install_args, STEP_TIMEOUT)?,
            CommandSpec::privileged(MV, &mv_args, STEP_TIMEOUT)?,
        ),
    };
    Ok(vec![
        step(
            &format!("{id}-install"),
            format!("Write {final_path} (temporary file)"),
            install.with_stdin(content)?,
            true,
        ),
        step(
            &format!("{id}-move"),
            format!("Replace {final_path}"),
            mv,
            false,
        ),
    ])
}

/// Root write to a managed system path (`Privilege::Root`, owner root:root).
/// A `sites-enabled` link is never a content target.
pub fn root_write(
    id: &str,
    path: &ManagedPath,
    mode: FileMode,
    content: Vec<u8>,
) -> Result<Vec<PlanStep>, ValidationError> {
    if path.kind() == ManagedKind::NginxEnabledLink {
        return Err(ValidationError::new(
            "path",
            "a sites-enabled link cannot receive content",
        ));
    }
    install_then_move(id, path.as_str(), mode, content, true, None)
}

/// Write inside the app's own directory, run as the app user (never root,
/// no `-o`/`-g`: the app user owns what it creates). The app root must be
/// the one built for the same slug as `user`; the caller derives both from
/// one validated slug.
pub fn app_write(
    id: &str,
    user: &LinuxUserName,
    path: &AppPath,
    mode: FileMode,
    content: Vec<u8>,
) -> Result<Vec<PlanStep>, ValidationError> {
    install_then_move(id, path.as_str(), mode, content, false, Some(user))
}

/// Keeps `AppRoot` in the public signature of path building without a second
/// way to make an `AppPath`.
pub fn app_path(root: &AppRoot, relative: &str) -> Result<AppPath, ValidationError> {
    AppPath::parse(root, relative)
}

/// The final path an `install` step built by this module writes to: its
/// temp-file argument without [`TMP_SUFFIX`], re-validated for the privilege
/// of the command (a managed path for root, a path inside an app root for the
/// app user). Anything else is refused, so a module-run step can derive its
/// follow-up commands from the plan step alone.
pub fn final_path_of(install: &CommandSpec) -> Result<String, ValidationError> {
    const F: &str = "path";
    let bad = |m: &str| Err(ValidationError::new(F, m.to_owned()));
    let Some(tmp) = install.args().last() else {
        return bad("not an install step");
    };
    let Some(final_path) = tmp.strip_suffix(TMP_SUFFIX) else {
        return bad("not an install step");
    };
    if install.program() != INSTALL {
        return bad("not an install step");
    }
    match install.privilege() {
        Privilege::Root => {
            let p = ManagedPath::parse(final_path)?;
            if p.kind() == ManagedKind::NginxEnabledLink {
                return bad("a sites-enabled link cannot receive content");
            }
        }
        Privilege::RunAs(_) => {
            let rest = final_path
                .strip_prefix(APPS_ROOT)
                .and_then(|r| r.strip_prefix('/'))
                .ok_or_else(|| ValidationError::new(F, "is not inside an app root"))?;
            let (slug, rel) = rest
                .split_once('/')
                .ok_or_else(|| ValidationError::new(F, "is not inside an app root"))?;
            AppPath::parse(&AppRoot::new(&app_slug(slug)?)?, rel)?;
        }
        Privilege::User => return bad("an unprivileged install is not accepted"),
    }
    Ok(final_path.to_owned())
}
/// The `mv -T` step that completes an `install` step, with the same
/// privilege, for the path validated by [`final_path_of`].
pub fn move_for(install: &CommandSpec) -> Result<CommandSpec, ValidationError> {
    let final_path = final_path_of(install)?;
    let tmp = format!("{final_path}{TMP_SUFFIX}");
    let args = ["-T", "--", &tmp, &final_path];
    match install.privilege() {
        Privilege::RunAs(u) => CommandSpec::run_as(u, MV, &args, STEP_TIMEOUT),
        _ => CommandSpec::privileged(MV, &args, STEP_TIMEOUT),
    }
}
/// Copies the current managed file to `<file>.ossiframe-bak`, preserving
/// attributes and never following a symlink.
pub fn backup_step(id: &str, path: &ManagedPath) -> Result<PlanStep, ValidationError> {
    let bak = format!("{}{BAK_SUFFIX}", path.as_str());
    let cmd = CommandSpec::privileged(
        CP,
        &["-a", "--no-dereference", "-T", "--", path.as_str(), &bak],
        STEP_TIMEOUT,
    )?;
    Ok(step(id, format!("Back up {}", path.as_str()), cmd, false))
}

/// Restores the backup over the managed file.
pub fn restore_step(id: &str, path: &ManagedPath) -> Result<PlanStep, ValidationError> {
    let bak = format!("{}{BAK_SUFFIX}", path.as_str());
    let cmd = CommandSpec::privileged(MV, &["-T", "--", &bak, path.as_str()], STEP_TIMEOUT)?;
    Ok(step(id, format!("Restore {}", path.as_str()), cmd, false))
}

/// Why a write sequence stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteError {
    /// The step ran and exited non-zero.
    Failed { step_id: String, exit: ExitStatus },
    /// The step could not complete (timeout, cancel, lost connection).
    Exec { step_id: String, error: ExecError },
}

/// Runs steps in order and stops at the first failure, so `mv` never runs
/// after a failed `install`.
pub async fn run_steps(
    exec: &dyn Executor,
    cancel: &CancelToken,
    steps: &[PlanStep],
) -> Result<(), WriteError> {
    for s in steps {
        match exec.run(&s.command, cancel).await {
            Ok(r) if r.exit.success() => {}
            Ok(r) => {
                return Err(WriteError::Failed {
                    step_id: s.id.clone(),
                    exit: r.exit,
                });
            }
            Err(error) => {
                return Err(WriteError::Exec {
                    step_id: s.id.clone(),
                    error,
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{app_slug, app_user};
    use async_trait::async_trait;
    use ossiframe_core::{ExecResult, Privilege};
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    const FINAL: &str = "/etc/nginx/sites-available/ossiframe-blog";

    struct Fake {
        install: Result<i32, ExecError>,
        mv: i32,
        files: Mutex<BTreeMap<String, Vec<u8>>>,
        calls: Mutex<Vec<String>>,
    }

    impl Fake {
        fn new(install: Result<i32, ExecError>, mv: i32) -> Self {
            let mut files = BTreeMap::new();
            files.insert(FINAL.to_owned(), b"old".to_vec());
            Self {
                install,
                mv,
                files: Mutex::new(files),
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    fn done(code: i32) -> Result<ExecResult, ExecError> {
        Ok(ExecResult {
            exit: ExitStatus::Code(code),
            stdout: String::new(),
            stderr: String::new(),
            truncated: false,
        })
    }

    #[async_trait]
    impl Executor for Fake {
        async fn run(&self, s: &CommandSpec, _: &CancelToken) -> Result<ExecResult, ExecError> {
            self.calls.lock().unwrap().push(s.program().to_owned());
            let a = s.args();
            let mut files = self.files.lock().unwrap();
            if s.program() == INSTALL {
                let code = self.install.clone()?;
                if code == 0 {
                    files.insert(a.last().unwrap().clone(), s.stdin().unwrap().to_vec());
                }
                done(code)
            } else {
                if self.mv == 0 {
                    let data = files.remove(&a[a.len() - 2]).expect("tmp exists");
                    files.insert(a[a.len() - 1].clone(), data);
                }
                done(self.mv)
            }
        }
    }

    fn managed_steps() -> Vec<PlanStep> {
        let p = ManagedPath::parse(FINAL).unwrap();
        root_write("site", &p, FileMode::M0644, b"new".to_vec()).unwrap()
    }

    #[test]
    fn root_write_is_fixed_argv_with_stdin_only_on_install() {
        let s = managed_steps();
        assert_eq!(s.len(), 2);
        assert!(s[0].requires_stdin && s[0].command.has_stdin());
        assert!(!s[1].requires_stdin && !s[1].command.has_stdin());
        assert_eq!(s[0].command.privilege(), Privilege::Root);
        assert_eq!(
            s[0].command.args(),
            [
                "-m",
                "0644",
                "-o",
                "root",
                "-g",
                "root",
                "-T",
                "--",
                "/dev/stdin",
                &format!("{FINAL}.ossiframe-tmp")
            ]
        );
        assert_eq!(
            s[1].command.to_command_line(),
            format!(
                "'/usr/bin/sudo' '-n' '--' '/usr/bin/mv' '-T' '--' '{FINAL}.ossiframe-tmp' '{FINAL}'"
            )
        );
        // A sites-enabled link and non-managed paths cannot be written by root.
        let link = ManagedPath::parse("/etc/nginx/sites-enabled/ossiframe-blog").unwrap();
        assert!(root_write("x", &link, FileMode::M0644, vec![]).is_err());
        assert!(ManagedPath::parse("/etc/passwd").is_err());
    }

    #[test]
    fn app_dir_writes_run_as_the_app_user_never_root() {
        let slug = app_slug("blog").unwrap();
        let user = app_user(&slug).unwrap();
        let root = AppRoot::new(&slug).unwrap();
        for rel in [".env", ".ssh/known_hosts"] {
            let p = app_path(&root, rel).unwrap();
            let steps = app_write("w", &user, &p, FileMode::M0600, b"s".to_vec()).unwrap();
            for s in &steps {
                assert_eq!(s.command.privilege(), Privilege::RunAs(user.clone()));
                assert!(!s.command.args().iter().any(|a| a == "-o" || a == "-g"));
            }
            assert!(steps[0].requires_stdin);
        }
    }

    #[test]
    fn backup_and_restore_do_not_follow_symlinks() {
        let p = ManagedPath::parse(FINAL).unwrap();
        let b = backup_step("bak", &p).unwrap();
        assert_eq!(b.command.program(), CP);
        assert!(b.command.args().iter().any(|a| a == "--no-dereference"));
        let r = restore_step("res", &p).unwrap();
        assert_eq!(r.command.args()[0], "-T");
        assert_eq!(r.command.args().last().unwrap(), FINAL);
    }

    #[tokio::test]
    async fn mv_never_runs_after_failed_install_and_state_is_unchanged() {
        let cases: [(Result<i32, ExecError>, i32, bool, usize); 4] = [
            (Ok(0), 0, true, 2),
            (Ok(1), 0, false, 1),
            (Err(ExecError::ConnectionLost), 0, false, 1),
            (Ok(0), 1, false, 2),
        ];
        for (install, mv, ok, calls) in cases {
            let fake = Fake::new(install, mv);
            let r = run_steps(&fake, &CancelToken::new(), &managed_steps()).await;
            assert_eq!(r.is_ok(), ok);
            assert_eq!(fake.calls.lock().unwrap().len(), calls);
            let files = fake.files.lock().unwrap();
            let expected: &[u8] = if ok { b"new" } else { b"old" };
            assert_eq!(files[FINAL], expected);
            if !ok && calls == 1 {
                assert_eq!(fake.calls.lock().unwrap().as_slice(), [INSTALL]);
            }
        }
    }
}
