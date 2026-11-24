//! Structured remote commands and POSIX shell quoting.

use crate::ident::ValidationError;
use std::time::Duration;

/// A remote command built by trusted Rust code: a program, its arguments and
/// a timeout. There is no shell syntax in this model; every word is quoted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    program: String,
    args: Vec<String>,
    timeout: Duration,
}

impl CommandSpec {
    pub fn new(program: &str, args: &[&str], timeout: Duration) -> Result<Self, ValidationError> {
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
        })
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
    /// including the program, is single-quoted.
    pub fn to_command_line(&self) -> String {
        std::iter::once(self.program.as_str())
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
