//! `.env` read, display redaction and merge-write for `<app_root>/shared/.env`.
//!
//! - Read: `/usr/bin/cat -- <path>` as the app user, bounded to
//!   [`MAX_ENV_BYTES`]. The content lives only in an [`EnvFile`] in backend
//!   memory (its `Debug` prints no content); it is never stored in operation
//!   records or logs.
//! - Display: [`EnvFile::entries`] returns [`EnvEntryDto`]s. Values of
//!   secret-like keys (and URL values carrying a `user:password@`) are never
//!   returned, only whether they are set. There is no reveal.
//! - Edit: [`apply_changes`] merges `set`/`delete` changes into the existing
//!   text. Every unchanged line stays byte-identical, so unchanged secrets
//!   are preserved without the frontend knowing them.
//! - Write: [`env_write_steps`] returns plan steps (design: `app_set_env`
//!   goes through plan and approval) that write the whole file through
//!   [`app_write`] (app user, 0600, content on stdin), plus an optional
//!   `config:cache` follow-up for Laravel.
//!
//! Quoting (compatible with Laravel's vlucas/phpdotenv): plain when the value
//! uses only `A-Za-z0-9_./:@-+,%=`; otherwise single quotes (no
//! interpolation) when it holds neither `'` nor `\`; otherwise double quotes
//! with `\\` and `\"` escaping. phpdotenv interpolates `${VAR}` inside double
//! quotes and offers no escape for it, so a value that needs double quotes
//! and contains `${` is refused.

use crate::paths::{AppPath, AppRoot};
use crate::write::{FileMode, app_write};
use ossiframe_core::{
    CancelToken, CommandSpec, ExecError, Executor, LinuxUserName, PlanStep, StepMode,
    ValidationError,
};
use serde::Serialize;
use std::fmt;
use std::time::Duration;

pub const MAX_ENV_BYTES: usize = 64 * 1024;
pub const MAX_VALUE_BYTES: usize = 4 * 1024;
const CAT: &str = "/usr/bin/cat";
const PHP: &str = "/usr/bin/php";
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const CACHE_TIMEOUT: Duration = Duration::from_secs(120);
const ENV_REL: &str = "shared/.env";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvError {
    Invalid(ValidationError),
    /// The file is larger than [`MAX_ENV_BYTES`].
    TooLarge,
    /// `cat` failed (missing file, permission).
    ReadFailed,
    Exec(ExecError),
    /// The change would break the app; the message says which key.
    Refused(String),
}

impl From<ValidationError> for EnvError {
    fn from(e: ValidationError) -> Self {
        EnvError::Invalid(e)
    }
}

fn invalid(field: &'static str, reason: &str) -> EnvError {
    EnvError::Invalid(ValidationError::new(field, reason.to_owned()))
}

/// A validated variable name: `^[A-Z_][A-Z0-9_]{0,63}$`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvKey(String);

impl EnvKey {
    pub fn parse(input: &str) -> Result<Self, ValidationError> {
        let b = input.as_bytes();
        let ok = !b.is_empty()
            && b.len() <= 64
            && (b[0].is_ascii_uppercase() || b[0] == b'_')
            && b.iter()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == b'_');
        if !ok {
            return Err(ValidationError::new(
                "key",
                "must match [A-Z_][A-Z0-9_]{0,63}",
            ));
        }
        Ok(Self(input.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A validated value: no NUL, CR or LF, at most 4 KiB. `Debug` hides it.
#[derive(Clone, PartialEq, Eq)]
pub struct EnvValue(String);

impl EnvValue {
    pub fn parse(input: &str) -> Result<Self, ValidationError> {
        if input.len() > MAX_VALUE_BYTES {
            return Err(ValidationError::new("value", "must be at most 4 KiB"));
        }
        if input.contains(['\0', '\n', '\r']) {
            return Err(ValidationError::new(
                "value",
                "must not contain NUL or line breaks",
            ));
        }
        Ok(Self(input.to_owned()))
    }
}

impl fmt::Debug for EnvValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EnvValue(<hidden>)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvOp {
    Set(EnvValue),
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvChange {
    pub key: EnvKey,
    pub op: EnvOp,
}

/// What the app is, for refusing deletions that would break it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnvContext {
    pub laravel: bool,
    pub managed_database: bool,
}

/// Display row. A secret entry has `value: None`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvEntryDto {
    pub key: String,
    pub value: Option<String>,
    pub secret: bool,
    pub is_set: bool,
}

/// Whether the key name looks like it holds a secret. Tokens are separated
/// by `_`, so `MY_KEYBOARD_LAYOUT` is not secret and `STRIPE_KEY` is.
pub fn is_secret_key(key: &str) -> bool {
    const TOKENS: [&str; 10] = [
        "PASSWORD",
        "PASS",
        "SECRET",
        "KEY",
        "TOKEN",
        "PRIVATE",
        "CREDENTIAL",
        "CREDENTIALS",
        "AUTH",
        "DSN",
    ];
    const EXPLICIT: [&str; 5] = [
        "APP_KEY",
        "DB_PASSWORD",
        "REDIS_PASSWORD",
        "MAIL_PASSWORD",
        "AWS_SECRET_ACCESS_KEY",
    ];
    let k = key.to_ascii_uppercase();
    EXPLICIT.contains(&k.as_str()) || k.split('_').any(|t| TOKENS.contains(&t))
}

/// `scheme://user:pass@host...`: a password in the URL userinfo.
fn url_has_password(value: &str) -> bool {
    let Some((scheme, rest)) = value.split_once("://") else {
        return false;
    };
    if scheme.is_empty() {
        return false;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    match authority.rsplit_once('@') {
        Some((userinfo, _)) => userinfo.split_once(':').is_some_and(|(_, p)| !p.is_empty()),
        None => false,
    }
}

/// One logical entry of the file: its raw text (one or more physical lines,
/// without the final newline) and, for an assignment, the key and value.
#[derive(Clone)]
struct Line {
    raw: String,
    assign: Option<(String, String)>,
}

fn parse_value(rest: &str) -> (String, usize) {
    // Returns (value, number of extra physical lines consumed is handled by caller)
    let t = rest.trim_start_matches([' ', '\t']);
    match t.chars().next() {
        Some('\'') | Some('"') => (t.to_owned(), 0),
        _ => {
            let cut = match t.find(" #").or_else(|| t.find("\t#")) {
                Some(i) => &t[..i],
                None if t.starts_with('#') => "",
                None => t,
            };
            (cut.trim().to_owned(), 0)
        }
    }
}

/// Reads a quoted value starting at `t` (which begins with the quote).
/// Returns the unquoted value and the byte index just past the closing quote,
/// or `None` when the quote is not closed in `t`.
fn read_quoted(t: &str) -> Option<(String, usize)> {
    let quote = t.chars().next()?;
    let mut out = String::new();
    let mut chars = t.char_indices().skip(1);
    while let Some((i, c)) = chars.next() {
        if c == '\\' {
            match chars.clone().next() {
                Some((_, n)) if n == '\\' || n == quote => {
                    out.push(n);
                    chars.next();
                }
                _ => out.push('\\'),
            }
        } else if c == quote {
            return Some((out, i + 1));
        } else {
            out.push(c);
        }
    }
    None
}

fn split_assignment(line: &str) -> Option<(&str, &str)> {
    let s = line.trim_start();
    let s = s.strip_prefix("export ").map_or(s, str::trim_start);
    let (k, v) = s.split_once('=')?;
    let k = k.trim_end();
    EnvKey::parse(k).ok()?;
    Some((k, v))
}

fn parse_lines(text: &str) -> Vec<Line> {
    let phys: Vec<&str> = text.split('\n').collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < phys.len() {
        let first = phys[i];
        let Some((key, rest)) = split_assignment(first) else {
            out.push(Line {
                raw: first.to_owned(),
                assign: None,
            });
            i += 1;
            continue;
        };
        let (v, _) = parse_value(rest);
        let mut raw = first.to_owned();
        let mut value = v.clone();
        if v.starts_with(['\'', '"']) {
            // Quoted: may span several physical lines.
            let mut acc = v.clone();
            let mut j = i;
            loop {
                if let Some((val, _)) = read_quoted(&acc) {
                    value = val;
                    break;
                }
                if j + 1 >= phys.len() {
                    // Unclosed: treat the first line alone, raw.
                    j = i;
                    value = v.trim_matches(['\'', '"', '\r']).to_owned();
                    break;
                }
                j += 1;
                acc.push('\n');
                acc.push_str(phys[j]);
            }
            raw = phys[i..=j].join("\n");
            i = j;
        } else {
            value = value.trim_end_matches('\r').to_owned();
        }
        out.push(Line {
            raw,
            assign: Some((key.to_owned(), value)),
        });
        i += 1;
    }
    out
}

fn join_lines(lines: &[Line]) -> String {
    lines
        .iter()
        .map(|l| l.raw.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The `.env` text held in backend memory. `Debug` prints only the size.
#[derive(Clone, PartialEq, Eq)]
pub struct EnvFile(String);

impl fmt::Debug for EnvFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EnvFile({} bytes)", self.0.len())
    }
}

impl EnvFile {
    pub fn new(text: String) -> Result<Self, EnvError> {
        if text.len() > MAX_ENV_BYTES {
            return Err(EnvError::TooLarge);
        }
        Ok(Self(text))
    }

    /// Display rows in file order (the last assignment of a key wins when a
    /// key repeats is not modeled: every assignment is listed).
    pub fn entries(&self) -> Vec<EnvEntryDto> {
        parse_lines(&self.0)
            .into_iter()
            .filter_map(|l| l.assign)
            .map(|(key, value)| {
                let secret = is_secret_key(&key) || url_has_password(&value);
                EnvEntryDto {
                    is_set: !value.is_empty(),
                    value: if secret { None } else { Some(value) },
                    secret,
                    key,
                }
            })
            .collect()
    }

    pub(crate) fn text(&self) -> &str {
        &self.0
    }
}

/// The path of the app's `.env`.
pub fn env_path(root: &AppRoot) -> Result<AppPath, ValidationError> {
    AppPath::parse(root, ENV_REL)
}

/// The fixed read command, run as the app user.
pub fn read_command(user: &LinuxUserName, path: &AppPath) -> Result<CommandSpec, ValidationError> {
    CommandSpec::run_as(user.clone(), CAT, &["--", path.as_str()], READ_TIMEOUT)
}

/// Reads the file as the app user. Output stays in memory; oversize is an
/// error.
pub async fn read_env(
    exec: &dyn Executor,
    cancel: &CancelToken,
    user: &LinuxUserName,
    root: &AppRoot,
) -> Result<EnvFile, EnvError> {
    let cmd = read_command(user, &env_path(root)?)?;
    let r = exec.run(&cmd, cancel).await.map_err(EnvError::Exec)?;
    if !r.exit.success() {
        return Err(EnvError::ReadFailed);
    }
    if r.truncated || r.stdout.len() > MAX_ENV_BYTES {
        return Err(EnvError::TooLarge);
    }
    EnvFile::new(r.stdout)
}

fn quote(value: &str) -> Result<String, EnvError> {
    let plain = !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_./:@-+,%=".contains(&b));
    if value.is_empty() || plain {
        return Ok(value.to_owned());
    }
    if !value.contains(['\'', '\\']) {
        return Ok(format!("'{value}'"));
    }
    if value.contains("${") {
        return Err(invalid(
            "value",
            "a value with a quote or backslash cannot also contain '${'",
        ));
    }
    Ok(format!(
        "\"{}\"",
        value.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

fn is_protected(key: &str, ctx: EnvContext) -> bool {
    (ctx.laravel || ctx.managed_database)
        && matches!(
            key,
            "APP_KEY" | "DB_PASSWORD" | "DB_USERNAME" | "DB_DATABASE"
        )
}

/// Merges changes into the existing text. Unchanged lines are untouched. A
/// set replaces the first assignment of the key in place (later duplicates
/// are removed) or appends; a delete removes every assignment of the key.
pub fn apply_changes(
    existing: &EnvFile,
    changes: &[EnvChange],
    ctx: EnvContext,
) -> Result<String, EnvError> {
    for c in changes {
        if c.op == EnvOp::Delete && is_protected(c.key.as_str(), ctx) {
            return Err(EnvError::Refused(format!(
                "{} cannot be deleted: the app needs it",
                c.key.as_str()
            )));
        }
    }
    let mut lines = parse_lines(existing.text());
    for c in changes {
        let key = c.key.as_str();
        let at = |l: &Line| l.assign.as_ref().is_some_and(|(k, _)| k == key);
        match &c.op {
            EnvOp::Delete => lines.retain(|l| !at(l)),
            EnvOp::Set(v) => {
                let raw = format!("{key}={}", quote(&v.0)?);
                let new = Line {
                    raw,
                    assign: Some((key.to_owned(), v.0.clone())),
                };
                match lines.iter().position(at) {
                    Some(first) => {
                        let mut seen = false;
                        lines = lines
                            .into_iter()
                            .enumerate()
                            .filter_map(|(i, l)| {
                                if i == first {
                                    seen = true;
                                    Some(new.clone())
                                } else if seen && at(&l) {
                                    None
                                } else {
                                    Some(l)
                                }
                            })
                            .collect();
                    }
                    None => {
                        // Keep a final trailing newline: the last element is
                        // the empty string after it.
                        let pos = if lines.last().is_some_and(|l| l.raw.is_empty()) {
                            lines.len() - 1
                        } else {
                            lines.len()
                        };
                        lines.insert(pos, new);
                    }
                }
            }
        }
    }
    let out = join_lines(&lines);
    if out.len() > MAX_ENV_BYTES {
        return Err(EnvError::TooLarge);
    }
    Ok(out)
}

fn artisan_cache_step(user: &LinuxUserName, root: &AppRoot) -> Result<PlanStep, EnvError> {
    let artisan = AppPath::parse(root, "current/artisan")?;
    Ok(PlanStep {
        id: "env-config-cache".to_owned(),
        title: "Refresh the Laravel config cache".to_owned(),
        command: CommandSpec::run_as(
            user.clone(),
            PHP,
            &[artisan.as_str(), "config:cache"],
            CACHE_TIMEOUT,
        )?,
        mode: StepMode::Foreground,
        deadline: CACHE_TIMEOUT,
        requires_stdin: false,
    })
}

/// Plan steps for `app_set_env`: write the merged file (app user, 0600, stdin,
/// then atomic move), and for Laravel a `config:cache` follow-up. The content
/// is carried only on the stdin step and must not be persisted by the caller.
pub fn env_write_steps(
    user: &LinuxUserName,
    root: &AppRoot,
    existing: &EnvFile,
    changes: &[EnvChange],
    ctx: EnvContext,
) -> Result<Vec<PlanStep>, EnvError> {
    if changes.is_empty() {
        return Err(invalid("changes", "no changes"));
    }
    let merged = apply_changes(existing, changes, ctx)?;
    let mut steps = app_write(
        "env",
        user,
        &env_path(root)?,
        FileMode::M0600,
        merged.into_bytes(),
    )?;
    if ctx.laravel {
        steps.push(artisan_cache_step(user, root)?);
    }
    Ok(steps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{app_slug, app_user};
    use ossiframe_core::Privilege;

    fn set(k: &str, v: &str) -> EnvChange {
        EnvChange {
            key: EnvKey::parse(k).unwrap(),
            op: EnvOp::Set(EnvValue::parse(v).unwrap()),
        }
    }
    fn del(k: &str) -> EnvChange {
        EnvChange {
            key: EnvKey::parse(k).unwrap(),
            op: EnvOp::Delete,
        }
    }
    const NONE: EnvContext = EnvContext {
        laravel: false,
        managed_database: false,
    };
    const DB: EnvContext = EnvContext {
        laravel: true,
        managed_database: true,
    };
    const SAMPLE: &str = "# app\nAPP_NAME=Blog\nAPP_KEY=base64:abc==\n\nDB_PASSWORD='p w'\nMY_KEYBOARD_LAYOUT=us\nDATABASE_URL=mysql://u:p@h/db\nEMPTY_TOKEN=\n";

    #[test]
    fn redaction_table() {
        for k in [
            "DB_PASSWORD",
            "APP_KEY",
            "STRIPE_SECRET",
            "AWS_SECRET_ACCESS_KEY",
            "STRIPE_KEY",
            "api_token",
            "SENTRY_DSN",
            "GIT_AUTH",
        ] {
            assert!(is_secret_key(k), "{k}");
        }
        for k in ["MY_KEYBOARD_LAYOUT", "APP_NAME", "PASSENGER_MAX"] {
            assert!(!is_secret_key(k), "{k}");
        }
        assert!(url_has_password("mysql://u:p@h/db"));
        assert!(!url_has_password("https://example.com/a@b"));
        assert!(!url_has_password("mysql://u@h/db"));
    }

    #[test]
    fn dto_never_holds_secret_values() {
        let f = EnvFile::new(SAMPLE.to_owned()).unwrap();
        let e = f.entries();
        let get = |k: &str| e.iter().find(|x| x.key == k).unwrap().clone();
        assert_eq!(get("APP_NAME").value.as_deref(), Some("Blog"));
        assert_eq!(get("MY_KEYBOARD_LAYOUT").value.as_deref(), Some("us"));
        for k in ["APP_KEY", "DB_PASSWORD", "DATABASE_URL", "EMPTY_TOKEN"] {
            let x = get(k);
            assert!(x.secret && x.value.is_none(), "{k}");
        }
        assert!(get("DB_PASSWORD").is_set && !get("EMPTY_TOKEN").is_set);
        let shown = format!("{e:?}{}", serde_json::to_string(&e).unwrap());
        for s in ["base64:abc", "p w", "u:p@h"] {
            assert!(!shown.contains(s), "{s}");
        }
        assert!(!format!("{f:?}").contains("base64"));
        assert!(!format!("{:?}", set("A_SECRET", "hunter2")).contains("hunter2"));
    }

    #[test]
    fn unchanged_lines_stay_byte_identical() {
        let f = EnvFile::new(SAMPLE.to_owned()).unwrap();
        let out = apply_changes(&f, &[set("APP_NAME", "New Blog")], NONE).unwrap();
        assert_eq!(out, SAMPLE.replace("APP_NAME=Blog", "APP_NAME='New Blog'"));
        let out =
            apply_changes(&f, &[set("NEW_ONE", "1"), del("MY_KEYBOARD_LAYOUT")], NONE).unwrap();
        assert_eq!(
            out,
            SAMPLE.replace("MY_KEYBOARD_LAYOUT=us\n", "") + "NEW_ONE=1\n"
        );
        // No trailing newline in the source: append after the last line.
        let g = EnvFile::new("A=1".to_owned()).unwrap();
        assert_eq!(
            apply_changes(&g, &[set("B", "2")], NONE).unwrap(),
            "A=1\nB=2"
        );
    }

    #[test]
    fn quoting_round_trips_through_the_parser() {
        for v in [
            "plain",
            "",
            "a b",
            "a#b",
            "$HOME",
            "it's",
            "a\\b",
            "say \"hi\"",
            "x'\"\\y",
            "=",
            "  lead",
            "tail  ",
        ] {
            let f = EnvFile::new(String::new()).unwrap();
            let out = apply_changes(&f, &[set("K", v)], NONE).unwrap();
            let got = EnvFile::new(out.clone()).unwrap().entries();
            assert_eq!(got[0].value.as_deref(), Some(v), "{v:?} -> {out:?}");
        }
        let f = EnvFile::new(String::new()).unwrap();
        assert!(apply_changes(&f, &[set("K", "it's ${X}")], NONE).is_err());
    }

    #[test]
    fn validation_rejects_bad_input() {
        for k in ["", "a", "1A", "A-B", "A B", &"A".repeat(65), "A\n"] {
            assert!(EnvKey::parse(k).is_err(), "{k:?}");
        }
        for v in ["a\nb", "a\rb", "a\0b", &"x".repeat(4097)] {
            assert!(EnvValue::parse(v).is_err());
        }
        assert!(EnvValue::parse(&"x".repeat(4096)).is_ok());
    }

    #[test]
    fn deleting_required_keys_is_refused() {
        let f = EnvFile::new(SAMPLE.to_owned()).unwrap();
        for k in ["DB_PASSWORD", "DB_USERNAME", "DB_DATABASE", "APP_KEY"] {
            assert!(matches!(
                apply_changes(&f, &[del(k)], DB),
                Err(EnvError::Refused(_))
            ));
        }
        assert!(apply_changes(&f, &[del("DB_PASSWORD")], NONE).is_ok());
    }

    #[test]
    fn write_steps_are_app_user_0600_stdin_and_laravel_cache() {
        let slug = app_slug("blog").unwrap();
        let user = app_user(&slug).unwrap();
        let root = AppRoot::new(&slug).unwrap();
        let f = EnvFile::new(SAMPLE.to_owned()).unwrap();
        let steps = env_write_steps(&user, &root, &f, &[set("APP_NAME", "x")], DB).unwrap();
        assert_eq!(steps.len(), 3);
        assert!(steps[0].requires_stdin);
        assert!(steps[0].command.args().contains(&"0600".to_owned()));
        assert!(
            steps[0]
                .command
                .args()
                .last()
                .unwrap()
                .starts_with("/srv/ossiframe/apps/blog/shared/.env")
        );
        for s in &steps {
            assert_eq!(s.command.privilege(), Privilege::RunAs(user.clone()));
        }
        assert!(steps[2].command.args().contains(&"config:cache".to_owned()));
        let plain = env_write_steps(&user, &root, &f, &[set("APP_NAME", "x")], NONE).unwrap();
        assert_eq!(plain.len(), 2);
        assert!(env_write_steps(&user, &root, &f, &[], NONE).is_err());
    }

    #[test]
    fn read_command_is_absolute_cat_as_app_user() {
        let slug = app_slug("blog").unwrap();
        let user = app_user(&slug).unwrap();
        let root = AppRoot::new(&slug).unwrap();
        let c = read_command(&user, &env_path(&root).unwrap()).unwrap();
        assert_eq!(c.program(), CAT);
        assert_eq!(c.args(), ["--", "/srv/ossiframe/apps/blog/shared/.env"]);
        assert!(EnvFile::new("x".repeat(MAX_ENV_BYTES + 1)).is_err());
    }
}
