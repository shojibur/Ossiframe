//! Structured remote commands and POSIX shell quoting.

use crate::ident::{LinuxUserName, ValidationError};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use zeroize::Zeroizing;

/// Fixed sudo prefix for `Privilege::Root`. `-n` makes sudo fail instead of
/// prompting for a password.
const SUDO_PREFIX: [&str; 3] = ["/usr/bin/sudo", "-n", "--"];

/// Who a command runs as.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Privilege {
    /// The SSH login user.
    User,
    /// Root through non-interactive `sudo -n`.
    Root,
    /// Another unprivileged user through `sudo -n -u <user>`. The name can
    /// never be `root`.
    RunAs(LinuxUserName),
}

/// A remote command built by trusted Rust code: a program, its arguments, a
/// timeout and a privilege. There is no shell syntax in this model; every
/// word is quoted.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "CommandSpecWire")]
pub struct CommandSpec {
    program: String,
    args: Vec<String>,
    timeout: Duration,
    privilege: Privilege,
    /// Bytes written to the remote command's stdin, then EOF. Never
    /// serialized, never part of the command line, never in `Debug`.
    #[serde(skip)]
    stdin: Option<Zeroizing<Vec<u8>>>,
}

/// Largest stdin payload a command may carry.
pub const MAX_STDIN_BYTES: usize = 1024 * 1024;

impl std::fmt::Debug for CommandSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct("CommandSpec");
        d.field("program", &self.program)
            .field("args", &self.args)
            .field("timeout", &self.timeout)
            .field("privilege", &self.privilege);
        match &self.stdin {
            Some(s) => d.field("stdin", &format_args!("<{} bytes>", s.len())),
            None => d.field("stdin", &format_args!("None")),
        };
        d.finish()
    }
}

/// Wire form used to read a stored command back. It is validated through the
/// same constructors as any other command.
#[derive(Deserialize)]
struct CommandSpecWire {
    program: String,
    args: Vec<String>,
    timeout: Duration,
    privilege: Privilege,
}

impl TryFrom<CommandSpecWire> for CommandSpec {
    type Error = ValidationError;
    fn try_from(w: CommandSpecWire) -> Result<Self, Self::Error> {
        let args: Vec<&str> = w.args.iter().map(String::as_str).collect();
        match w.privilege {
            Privilege::User => CommandSpec::new(&w.program, &args, w.timeout),
            Privilege::Root => CommandSpec::privileged(&w.program, &args, w.timeout),
            Privilege::RunAs(u) => CommandSpec::run_as(u, &w.program, &args, w.timeout),
        }
    }
}

impl CommandSpec {
    /// A command that runs as the login user (`Privilege::User`).
    pub fn new(program: &str, args: &[&str], timeout: Duration) -> Result<Self, ValidationError> {
        Self::build(program, args, timeout, Privilege::User)
    }

    /// A command that runs as root through `sudo -n`. `program` must be an
    /// absolute path so that no PATH or `secure_path` lookup is involved.
    pub fn privileged(
        program: &str,
        args: &[&str],
        timeout: Duration,
    ) -> Result<Self, ValidationError> {
        if !program.starts_with('/') {
            return Err(ValidationError::new(
                "program",
                "privileged program must be an absolute path",
            ));
        }
        Self::build(program, args, timeout, Privilege::Root)
    }

    /// A command that runs as `user` through `sudo -n -u <user> --`.
    /// `program` must be an absolute path, as for [`CommandSpec::privileged`].
    pub fn run_as(
        user: LinuxUserName,
        program: &str,
        args: &[&str],
        timeout: Duration,
    ) -> Result<Self, ValidationError> {
        if !program.starts_with('/') {
            return Err(ValidationError::new(
                "program",
                "privileged program must be an absolute path",
            ));
        }
        Self::build(program, args, timeout, Privilege::RunAs(user))
    }

    fn build(
        program: &str,
        args: &[&str],
        timeout: Duration,
        privilege: Privilege,
    ) -> Result<Self, ValidationError> {
        if program.is_empty() {
            return Err(ValidationError::new("program", "must not be empty"));
        }
        if program.starts_with('-') {
            return Err(ValidationError::new("program", "must not start with '-'"));
        }
        if program.contains('\0') || args.iter().any(|a| a.contains('\0')) {
            return Err(ValidationError::new("command", "must not contain NUL"));
        }
        Ok(Self {
            program: program.to_owned(),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
            timeout,
            privilege,
            stdin: None,
        })
    }

    /// Attaches stdin bytes (at most [`MAX_STDIN_BYTES`]). The bytes are
    /// zeroed on drop and are not part of serialization or `Debug`.
    pub fn with_stdin(mut self, stdin: Vec<u8>) -> Result<Self, ValidationError> {
        if stdin.len() > MAX_STDIN_BYTES {
            return Err(ValidationError::new("stdin", "must be at most 1 MiB"));
        }
        self.stdin = Some(Zeroizing::new(stdin));
        Ok(self)
    }

    /// The stdin bytes, if any.
    pub fn stdin(&self) -> Option<&[u8]> {
        self.stdin.as_deref().map(Vec::as_slice)
    }

    pub fn has_stdin(&self) -> bool {
        self.stdin.is_some()
    }

    pub fn privilege(&self) -> Privilege {
        self.privilege.clone()
    }

    pub fn program(&self) -> &str {
        &self.program
    }

    pub fn args(&self) -> &[String] {
        &self.args
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// The single command line sent to the remote shell. Every word,
    /// including the program, is single-quoted. `Root` commands start with
    /// `'/usr/bin/sudo' '-n' '--'`.
    pub fn to_command_line(&self) -> String {
        let prefix: Vec<&str> = match &self.privilege {
            Privilege::User => Vec::new(),
            Privilege::Root => SUDO_PREFIX.to_vec(),
            Privilege::RunAs(u) => vec!["/usr/bin/sudo", "-n", "-u", u.as_str(), "--"],
        };
        prefix
            .into_iter()
            .chain(std::iter::once(self.program.as_str()))
            .chain(self.args.iter().map(String::as_str))
            .map(quote_arg)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// POSIX single-quote escaping: wrap in `'...'` and encode `'` as `'\''`.
pub fn quote_arg(word: &str) -> String {
    let mut out = String::with_capacity(word.len() + 2);
    out.push('\'');
    for c in word.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn stored_command_round_trips_and_is_revalidated() {
        let t = Duration::from_secs(3);
        let spec = CommandSpec::privileged("/usr/bin/apt-get", &["update"], t).unwrap();
        let json = serde_json::to_string(&spec).unwrap();
        let back: CommandSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(back, spec);
        // A tampered record that would skip the constructor checks is refused.
        let relative = json.replace("/usr/bin/apt-get", "apt-get");
        assert!(serde_json::from_str::<CommandSpec>(&relative).is_err());
        let nul = json.replace("update", "up\\u0000date");
        assert!(serde_json::from_str::<CommandSpec>(&nul).is_err());
    }

    use super::*;

    const T: Duration = Duration::from_secs(5);

    #[test]
    fn stdin_limit_rendering_serialization_and_debug() {
        let plain = CommandSpec::new("cat", &[], T).unwrap();
        let at = plain
            .clone()
            .with_stdin(vec![b'x'; MAX_STDIN_BYTES])
            .unwrap();
        assert_eq!(at.to_command_line(), plain.to_command_line());
        assert!(at.has_stdin());
        assert!(
            plain
                .clone()
                .with_stdin(vec![0; MAX_STDIN_BYTES + 1])
                .is_err()
        );
        let s = plain.with_stdin(b"hunter2-secret".to_vec()).unwrap();
        assert_eq!(s.stdin(), Some(&b"hunter2-secret"[..]));
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("stdin") && !json.contains("hunter2"));
        assert!(
            !serde_json::from_str::<CommandSpec>(&json)
                .unwrap()
                .has_stdin()
        );
        let dbg = format!("{s:?}");
        assert!(dbg.contains("stdin: <14 bytes>") && !dbg.contains("hunter2"));
    }
    #[test]
    fn quotes_every_word() {
        let c = CommandSpec::new("cat", &["/etc/os-release"], T).unwrap();
        assert_eq!(c.to_command_line(), "'cat' '/etc/os-release'");
        assert_eq!(quote_arg(""), "''");
        assert_eq!(quote_arg("it's"), "'it'\\''s'");
    }

    #[test]
    fn privileged_line_has_fixed_sudo_prefix() {
        let c = CommandSpec::privileged("/usr/bin/apt-get", &["update"], T).unwrap();
        assert_eq!(c.privilege(), Privilege::Root);
        assert_eq!(
            c.to_command_line(),
            "'/usr/bin/sudo' '-n' '--' '/usr/bin/apt-get' 'update'"
        );
        let u = CommandSpec::new("ls", &[], T).unwrap();
        assert_eq!(u.privilege(), Privilege::User);
        assert!(!u.to_command_line().contains("sudo"));
    }

    /// A hostile argument is exactly one quoted word after the fixed prefix.
    /// The shell round trip above covers the quoting itself.
    #[test]
    fn run_as_line_has_fixed_prefix_and_rejects_root() {
        let u = LinuxUserName::parse("ossi-blog").unwrap();
        let c = CommandSpec::run_as(u.clone(), "/usr/bin/git", &["clone", "a b"], T).unwrap();
        assert_eq!(c.privilege(), Privilege::RunAs(u.clone()));
        assert_eq!(
            c.to_command_line(),
            "'/usr/bin/sudo' '-n' '-u' 'ossi-blog' '--' '/usr/bin/git' 'clone' 'a b'"
        );
        assert!(CommandSpec::run_as(u, "git", &[], T).is_err());
        assert!(LinuxUserName::parse("root").is_err());
        for bad in ["Root", "a b", "-u", "", "a;b", "1x"] {
            assert!(LinuxUserName::parse(bad).is_err(), "{bad:?}");
        }
        // A stored RunAs round-trips; a stored RunAs for root is refused.
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<CommandSpec>(&json).unwrap(), c);
        assert!(serde_json::from_str::<CommandSpec>(&json.replace("ossi-blog", "root")).is_err());
    }

    #[test]
    fn privileged_hostile_argument_is_one_quoted_word() {
        let hostile = "x'; rm -rf / #$(id)";
        let c = CommandSpec::privileged("/usr/bin/true", &[hostile], T).unwrap();
        assert_eq!(
            c.to_command_line(),
            format!(
                "'/usr/bin/sudo' '-n' '--' '/usr/bin/true' {}",
                quote_arg(hostile)
            )
        );
    }

    #[test]
    fn privileged_rejects_relative_program() {
        assert!(CommandSpec::privileged("apt-get", &[], T).is_err());
        assert!(CommandSpec::privileged("", &[], T).is_err());
        assert!(CommandSpec::privileged("/bin/a\0b", &[], T).is_err());
    }

    #[test]
    fn rejects_bad_programs_and_nul() {
        assert!(CommandSpec::new("", &[], T).is_err());
        assert!(CommandSpec::new("-oProxyCommand=x", &[], T).is_err());
        assert!(CommandSpec::new("ls", &["a\0b"], T).is_err());
        assert!(CommandSpec::new("l\0s", &[], T).is_err());
    }

    /// Round trip through a real local POSIX shell: each hostile argument
    /// must arrive as exactly one unchanged word.
    #[test]
    #[cfg(unix)]
    fn hostile_arguments_survive_a_real_shell() {
        let hostile = [
            "it's",
            "a b",
            "; rm -rf /",
            "$(id)",
            "`id`",
            "line1\nline2",
            "-n",
            "--",
            "*",
            "\\",
            "'\"'",
            "$HOME",
            "",
            "&& echo pwned",
            "| cat",
            "> /tmp/x",
        ];
        let mut args = vec!["%s\\0"];
        args.extend(hostile.iter().copied());
        let c = CommandSpec::new("printf", &args, T).unwrap();
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(c.to_command_line())
            .output()
            .unwrap();
        assert!(out.status.success());
        let words: Vec<&[u8]> = out.stdout.split(|b| *b == 0).collect();
        assert_eq!(
            words.len(),
            hostile.len() + 1,
            "one NUL-terminated word per argument"
        );
        for (got, want) in words.iter().zip(hostile.iter()) {
            assert_eq!(*got, want.as_bytes());
        }
    }
}
