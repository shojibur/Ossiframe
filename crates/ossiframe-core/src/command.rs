//! Structured remote commands and POSIX shell quoting.

use crate::ident::ValidationError;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Fixed sudo prefix for `Privilege::Root`. `-n` makes sudo fail instead of
/// prompting for a password.
const SUDO_PREFIX: [&str; 3] = ["/usr/bin/sudo", "-n", "--"];

/// Who a command runs as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Privilege {
    /// The SSH login user.
    User,
    /// Root through non-interactive `sudo -n`.
    Root,
}

/// A remote command built by trusted Rust code: a program, its arguments, a
/// timeout and a privilege. There is no shell syntax in this model; every
/// word is quoted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "CommandSpecWire")]
pub struct CommandSpec {
    program: String,
    args: Vec<String>,
    timeout: Duration,
    privilege: Privilege,
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
        })
    }

    pub fn privilege(&self) -> Privilege {
        self.privilege
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
        let prefix: &[&str] = match self.privilege {
            Privilege::User => &[],
            Privilege::Root => &SUDO_PREFIX,
        };
        prefix
            .iter()
            .copied()
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
