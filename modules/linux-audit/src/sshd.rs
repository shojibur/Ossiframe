//! Limited `sshd_config` reader. This is not a full OpenSSH parser.
//!
//! It reads `/etc/ssh/sshd_config` with `cat`, follows `Include` lines the way
//! OpenSSH does (relative to `/etc/ssh`, lexical order, at the position of the
//! `Include` line) and records the first value of `PasswordAuthentication` and
//! `PermitRootLogin`. Everything it cannot follow is recorded as a [`Gap`], and
//! reading stops there, so a later value is never trusted past a hole.
//!
//! The result describes global configuration on disk. It says nothing about
//! per-user settings (`Match` blocks) or the running daemon.

use crate::model::MAX_EVIDENCE_CHARS;
use crate::probe::{ProbeFailure, run_probe};
use ossiframe_core::{CancelToken, ExecError, Executor};

/// The configuration file every read starts at.
pub const MAIN_CONFIG: &str = "/etc/ssh/sshd_config";
/// Deepest file nesting that is read. The main file has depth 0.
pub const MAX_INCLUDE_DEPTH: usize = 4;
/// Most files read, including the main file.
pub const MAX_FILES: usize = 32;
/// Longest stored value, in characters.
const MAX_VALUE_CHARS: usize = 64;
const SSH_DIR_PREFIX: &str = "/etc/ssh/";

/// The first value seen for a keyword and the file it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setting {
    /// The value as written, cut to [`MAX_VALUE_CHARS`] characters.
    pub value: String,
    pub file: String,
}

/// Why reading stopped before the end of the configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gap {
    /// A file could not be read (permission denied or gone).
    Unreadable,
    /// Something was not followed: unsupported pattern, limit reached or
    /// output cut at the size limit.
    Skipped,
    /// A command failed in some other way (short reason, no raw output).
    Failed(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SshdConfig {
    /// Files read, in the order they were read.
    pub files: Vec<String>,
    pub password_authentication: Option<Setting>,
    pub permit_root_login: Option<Setting>,
    /// A `Match` line was seen; later directives are not collected.
    pub match_blocks: bool,
    /// Set when reading stopped early. Values found before it still count.
    pub gap: Option<Gap>,
    pub limitations: Vec<String>,
}

#[derive(Clone, Copy)]
enum Key {
    Password,
    Root,
}

enum Line {
    Skip,
    Match,
    Include(Result<Vec<String>, ()>),
    Key(Key, String),
}

/// Splits a value into words. Double quotes group, an unquoted `#` at the
/// start of a word starts a comment. An unterminated quote is an error.
fn tokenize(s: &str) -> Result<Vec<String>, ()> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut quoted = false;
    for c in s.chars() {
        if quoted {
            if c == '"' {
                quoted = false;
            } else {
                cur.push(c);
            }
            continue;
        }
        match c {
            '"' => {
                quoted = true;
                in_word = true;
            }
            '#' if !in_word => return Ok(out),
            c if c.is_whitespace() => {
                if in_word {
                    out.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            c => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if quoted {
        return Err(());
    }
    if in_word {
        out.push(cur);
    }
    Ok(out)
}

fn parse_line(raw: &str) -> Line {
    let line = raw.trim();
    if line.is_empty() || line.starts_with('#') {
        return Line::Skip;
    }
    let end = line
        .find(|c: char| c.is_whitespace() || c == '=')
        .unwrap_or(line.len());
    let keyword = line[..end].to_ascii_lowercase();
    let mut rest = line[end..].trim_start();
    if let Some(r) = rest.strip_prefix('=') {
        rest = r.trim_start();
    }
    match keyword.as_str() {
        "match" => Line::Match,
        "include" => Line::Include(tokenize(rest)),
        "passwordauthentication" => Line::Key(Key::Password, value_text(rest)),
        "permitrootlogin" => Line::Key(Key::Root, value_text(rest)),
        _ => Line::Skip,
    }
}

fn value_text(rest: &str) -> String {
    let text = match tokenize(rest) {
        Ok(words) => words.join(" "),
        Err(()) => "(unparsable)".to_owned(),
    };
    text.chars()
        .filter(|c| !c.is_control())
        .take(MAX_VALUE_CHARS)
        .collect()
}

fn is_path_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '*' | '-')
}

/// Validates one `Include` word. Returns the directory and the last path
/// segment (the `find -name` value). Only `/etc/ssh/...` paths with the
/// characters `A-Za-z0-9._/*-` are accepted, no `..`, and `*` only in the last
/// segment.
pub fn parse_include_pattern(word: &str) -> Result<(String, String), &'static str> {
    if word.is_empty() {
        return Err("empty pattern");
    }
    let abs = if word.starts_with('/') {
        word.to_owned()
    } else {
        format!("{SSH_DIR_PREFIX}{word}")
    };
    if !abs.starts_with(SSH_DIR_PREFIX) || abs.len() == SSH_DIR_PREFIX.len() {
        return Err("outside /etc/ssh");
    }
    if !abs.chars().all(is_path_char) {
        return Err("unsupported characters");
    }
    if abs.contains("..") || abs.contains("//") {
        return Err("unsupported path form");
    }
    let (dir, name) = abs.rsplit_once('/').ok_or("unsupported path form")?;
    if name.is_empty() {
        return Err("no file name");
    }
    if dir.contains('*') {
        return Err("wildcard in directory part");
    }
    Ok((dir.to_owned(), name.to_owned()))
}

/// `*`-only glob match, as the Include pattern charset allows nothing else.
/// Like glob(3), a leading `.` in the name needs a leading `.` in the pattern.
fn glob_match(pattern: &str, name: &str) -> bool {
    if name.starts_with('.') && !pattern.starts_with('.') {
        return false;
    }
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or("");
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    if !pattern.contains('*') {
        return rest.is_empty();
    }
    let mut parts: Vec<&str> = parts.collect();
    let last = parts.pop().unwrap_or("");
    for mid in parts {
        match rest.find(mid) {
            Some(i) => rest = &rest[i + mid.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

/// A path printed by `find` must be exactly `dir/file` with a plain file
/// name. Returns the file name.
fn plain_file<'a>(dir: &str, path: &'a str) -> Option<&'a str> {
    let file = path.strip_prefix(dir)?.strip_prefix('/')?;
    (!file.is_empty() && !file.contains('/') && file.chars().all(is_path_char)).then_some(file)
}

struct Frame {
    file: String,
    lines: Vec<String>,
    next: usize,
    depth: usize,
    /// Set on a marker frame: when it is reached, reading stops with this gap.
    then: Option<Gap>,
}

impl Frame {
    fn marker(depth: usize, gap: Gap) -> Self {
        Frame {
            file: String::new(),
            lines: Vec::new(),
            next: 0,
            depth,
            then: Some(gap),
        }
    }
}

struct Reader<'a> {
    exec: &'a dyn Executor,
    cancel: &'a CancelToken,
    cfg: SshdConfig,
    /// Files tried, including unreadable ones.
    attempts: usize,
}

impl Reader<'_> {
    fn gap_for(&mut self, what: &str, failure: ProbeFailure) -> Gap {
        match failure {
            ProbeFailure::PermissionDenied | ProbeFailure::Missing => {
                self.cfg
                    .limitations
                    .push(format!("{what} could not be read ({}).", failure.reason()));
                Gap::Unreadable
            }
            ProbeFailure::Truncated => {
                self.cfg.limitations.push(format!(
                    "{what} was cut at the output size limit and was not read in full."
                ));
                Gap::Skipped
            }
            ProbeFailure::Error(m) => {
                self.cfg
                    .limitations
                    .push(format!("{what} could not be read ({m})."));
                Gap::Failed(m)
            }
        }
    }

    async fn read(&mut self, path: &str, depth: usize) -> Result<Result<Frame, Gap>, ExecError> {
        if self.attempts >= MAX_FILES {
            self.cfg.limitations.push(format!(
                "More than {MAX_FILES} configuration files; the rest were not read."
            ));
            return Ok(Err(Gap::Skipped));
        }
        self.attempts += 1;
        match run_probe(self.exec, self.cancel, "cat", &[path]).await? {
            Ok(text) => {
                self.cfg.files.push(path.to_owned());
                Ok(Ok(Frame {
                    file: path.to_owned(),
                    lines: text.lines().map(str::to_owned).collect(),
                    next: 0,
                    depth,
                    then: None,
                }))
            }
            Err(failure) => Ok(Err(self.gap_for(path, failure))),
        }
    }

    /// Frames for one `Include` line, in processing order. A marker frame at
    /// the end records a gap that stops reading after the earlier files.
    async fn expand(&mut self, words: &[String], depth: usize) -> Result<Vec<Frame>, ExecError> {
        let mut frames = Vec::new();
        let mut stop = None;
        'words: for word in words {
            let (dir, name) = match parse_include_pattern(word) {
                Ok(p) => p,
                Err(why) => {
                    self.cfg.limitations.push(format!(
                        "An Include pattern is not supported ({why}) and was not followed."
                    ));
                    stop = Some(Gap::Skipped);
                    break;
                }
            };
            let args = [
                dir.as_str(),
                "-maxdepth",
                "1",
                "(",
                "-type",
                "f",
                "-o",
                "-type",
                "l",
                ")",
                "-name",
                name.as_str(),
            ];
            let out = match run_probe(self.exec, self.cancel, "find", &args).await? {
                Ok(o) => o,
                // No such directory: the pattern matches nothing, as in OpenSSH.
                Err(ProbeFailure::Missing) => continue,
                Err(f) => {
                    stop = Some(self.gap_for(&format!("The Include search in {dir}"), f));
                    break;
                }
            };
            let mut paths: Vec<&str> = out.lines().filter(|l| !l.is_empty()).collect();
            paths.sort_unstable();
            for path in paths {
                let Some(file) = plain_file(&dir, path) else {
                    self.cfg.limitations.push(
                        "A file matched by an Include had an unsupported name and was not read."
                            .to_owned(),
                    );
                    stop = Some(Gap::Skipped);
                    break 'words;
                };
                if !glob_match(&name, file) {
                    continue;
                }
                match self.read(path, depth).await? {
                    Ok(frame) => frames.push(frame),
                    Err(gap) => {
                        stop = Some(gap);
                        break 'words;
                    }
                }
            }
        }
        if let Some(gap) = stop {
            frames.push(Frame::marker(depth, gap));
        }
        Ok(frames)
    }

    fn stop(&mut self, gap: Gap, why: &str) {
        self.cfg.limitations.push(why.to_owned());
        self.cfg.gap = Some(gap);
    }

    async fn walk(mut self) -> Result<SshdConfig, ExecError> {
        let mut stack = match self.read(MAIN_CONFIG, 0).await? {
            Ok(frame) => vec![frame],
            Err(gap) => {
                self.cfg.gap = Some(gap);
                return Ok(self.cfg);
            }
        };
        while let Some(top) = stack.last_mut() {
            if top.next >= top.lines.len() {
                let done = stack.pop();
                if let Some(gap) = done.and_then(|f| f.then) {
                    self.cfg.gap = Some(gap);
                    break;
                }
                continue;
            }
            let line = std::mem::take(&mut top.lines[top.next]);
            top.next += 1;
            let (file, depth) = (top.file.clone(), top.depth);
            match parse_line(&line) {
                Line::Skip => {}
                Line::Match => {
                    self.cfg.match_blocks = true;
                    break;
                }
                Line::Include(Err(())) => {
                    self.stop(Gap::Skipped, "An Include line could not be parsed.");
                    break;
                }
                Line::Include(Ok(words)) if words.is_empty() => {
                    self.stop(Gap::Skipped, "An Include line has no pattern.");
                    break;
                }
                Line::Include(Ok(_)) if depth + 1 > MAX_INCLUDE_DEPTH => {
                    self.stop(
                        Gap::Skipped,
                        &format!(
                            "Include nesting deeper than {MAX_INCLUDE_DEPTH} was not followed."
                        ),
                    );
                    break;
                }
                Line::Include(Ok(words)) => {
                    let frames = self.expand(&words, depth + 1).await?;
                    stack.extend(frames.into_iter().rev());
                }
                Line::Key(key, value) => {
                    let slot = match key {
                        Key::Password => &mut self.cfg.password_authentication,
                        Key::Root => &mut self.cfg.permit_root_login,
                    };
                    // OpenSSH: the first value wins.
                    if slot.is_none() {
                        *slot = Some(Setting { value, file });
                    }
                }
            }
        }
        Ok(self.cfg)
    }
}

/// Reads the global SSH daemon configuration. `Err` only for cancel and
/// connection loss. All other problems are recorded in the result.
pub async fn collect(exec: &dyn Executor, cancel: &CancelToken) -> Result<SshdConfig, ExecError> {
    Reader {
        exec,
        cancel,
        cfg: SshdConfig::default(),
        attempts: 0,
    }
    .walk()
    .await
}

/// Evidence line for one keyword: `Keyword value (file)` or the default note.
pub fn evidence_line(keyword: &str, setting: Option<&Setting>) -> String {
    let line = match setting {
        Some(s) => format!("{keyword} {} ({})", s.value, s.file),
        None => format!("{keyword}: default (not set)"),
    };
    line.chars().take(MAX_EVIDENCE_CHARS).collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use async_trait::async_trait;
    use ossiframe_core::{CommandSpec, ExecResult, ExitStatus};
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    pub(crate) enum Entry {
        Text(String),
        Denied,
        Truncated,
    }

    /// Fake remote file system answering `cat` and the fixed `find` form.
    #[derive(Default)]
    pub(crate) struct FakeFs {
        pub files: BTreeMap<String, Entry>,
        pub commands: Mutex<Vec<String>>,
    }

    impl FakeFs {
        pub(crate) fn with(files: &[(&str, &str)]) -> Self {
            let mut fs = FakeFs::default();
            for (p, t) in files {
                fs.files
                    .insert((*p).to_owned(), Entry::Text((*t).to_owned()));
            }
            fs
        }
    }

    fn done(
        code: i32,
        stdout: &str,
        stderr: &str,
        truncated: bool,
    ) -> Result<ExecResult, ExecError> {
        Ok(ExecResult {
            exit: ExitStatus::Code(code),
            stdout: stdout.to_owned(),
            stderr: stderr.to_owned(),
            truncated,
        })
    }

    #[async_trait]
    impl Executor for FakeFs {
        async fn run(&self, spec: &CommandSpec, _: &CancelToken) -> Result<ExecResult, ExecError> {
            self.commands.lock().unwrap().push(spec.to_command_line());
            let args = spec.args();
            match spec.program() {
                "cat" => match self.files.get(&args[0]) {
                    Some(Entry::Text(t)) => done(0, t, "", false),
                    Some(Entry::Denied) => done(1, "", "cat: Permission denied", false),
                    Some(Entry::Truncated) => done(0, "x", "", true),
                    None => done(1, "", "cat: No such file or directory", false),
                },
                "find" => {
                    assert_eq!(
                        args[1..11],
                        [
                            "-maxdepth",
                            "1",
                            "(",
                            "-type",
                            "f",
                            "-o",
                            "-type",
                            "l",
                            ")",
                            "-name"
                        ]
                    );
                    let (dir, pat) = (&args[0], &args[11]);
                    let out: Vec<&str> = self
                        .files
                        .keys()
                        .filter(|k| plain_file(dir, k).is_some_and(|f| glob_match(pat, f)))
                        .map(String::as_str)
                        .collect();
                    // Unsorted on purpose: the reader must sort.
                    let mut out = out;
                    out.reverse();
                    done(0, &out.join("\n"), "", false)
                }
                other => panic!("unexpected program {other}"),
            }
        }
    }

    async fn read(fs: &FakeFs) -> SshdConfig {
        collect(fs, &CancelToken::new()).await.unwrap()
    }

    fn val(s: &Option<Setting>) -> Option<(&str, &str)> {
        s.as_ref().map(|s| (s.value.as_str(), s.file.as_str()))
    }

    const D: &str = "/etc/ssh/sshd_config.d";

    #[tokio::test]
    async fn first_value_wins_in_include_order() {
        let fs = FakeFs::with(&[
            (
                MAIN_CONFIG,
                "Include /etc/ssh/sshd_config.d/*.conf\nPasswordAuthentication yes\nPermitRootLogin no\n",
            ),
            (
                &format!("{D}/50-b.conf"),
                "PasswordAuthentication yes\nPermitRootLogin yes\n",
            ),
            (&format!("{D}/10-a.conf"), "PasswordAuthentication no\n"),
            (
                &format!("{D}/.hidden.conf"),
                "PermitRootLogin forced-commands-only\n",
            ),
        ]);
        let cfg = read(&fs).await;
        // Include comes first, so 10-a wins over the later main-file line.
        assert_eq!(
            val(&cfg.password_authentication),
            Some(("no", "/etc/ssh/sshd_config.d/10-a.conf"))
        );
        assert_eq!(
            val(&cfg.permit_root_login),
            Some(("yes", "/etc/ssh/sshd_config.d/50-b.conf"))
        );
        assert_eq!(cfg.files[1], format!("{D}/10-a.conf"));
        assert_eq!(cfg.files.len(), 3);
        assert!(cfg.gap.is_none() && !cfg.match_blocks);
    }

    #[tokio::test]
    async fn relative_include_and_value_after_include_position() {
        let fs = FakeFs::with(&[
            (
                MAIN_CONFIG,
                "PermitRootLogin no\nInclude sshd_config.d/*.conf\n",
            ),
            (&format!("{D}/a.conf"), "PermitRootLogin yes\n"),
        ]);
        let cfg = read(&fs).await;
        assert_eq!(val(&cfg.permit_root_login), Some(("no", MAIN_CONFIG)));
        assert_eq!(cfg.files.len(), 2);
    }

    #[tokio::test]
    async fn equals_form_case_quotes_and_comments() {
        let fs = FakeFs::with(&[(
            MAIN_CONFIG,
            "# PasswordAuthentication yes\n  passwordauthentication=\"no\"\nPERMITROOTLOGIN = Prohibit-Password # note\n",
        )]);
        let cfg = read(&fs).await;
        assert_eq!(val(&cfg.password_authentication).unwrap().0, "no");
        assert_eq!(val(&cfg.permit_root_login).unwrap().0, "Prohibit-Password");
    }

    #[tokio::test]
    async fn match_stops_collection() {
        let fs = FakeFs::with(&[(
            MAIN_CONFIG,
            "PermitRootLogin no\nMatch User bob\n  PasswordAuthentication yes\n",
        )]);
        let cfg = read(&fs).await;
        assert!(cfg.match_blocks);
        assert!(cfg.password_authentication.is_none());
        assert!(cfg.permit_root_login.is_some());
        assert!(cfg.gap.is_none());
    }

    #[tokio::test]
    async fn unreadable_main_and_include_are_gaps() {
        let mut fs = FakeFs::default();
        fs.files.insert(MAIN_CONFIG.to_owned(), Entry::Denied);
        assert_eq!(read(&fs).await.gap, Some(Gap::Unreadable));

        let mut fs = FakeFs::with(&[(
            MAIN_CONFIG,
            "Include sshd_config.d/*.conf\nPermitRootLogin no\n",
        )]);
        fs.files.insert(
            format!("{D}/10-a.conf"),
            Entry::Text("PasswordAuthentication yes\n".into()),
        );
        fs.files.insert(format!("{D}/20-b.conf"), Entry::Denied);
        let cfg = read(&fs).await;
        assert_eq!(cfg.gap, Some(Gap::Unreadable));
        // Earlier file counts, the later main-file line is not trusted.
        assert_eq!(val(&cfg.password_authentication).unwrap().0, "yes");
        assert!(cfg.permit_root_login.is_none());
        assert!(!cfg.limitations.is_empty());
    }

    #[tokio::test]
    async fn truncated_file_is_skipped_not_passed() {
        let mut fs = FakeFs::default();
        fs.files.insert(MAIN_CONFIG.to_owned(), Entry::Truncated);
        assert_eq!(read(&fs).await.gap, Some(Gap::Skipped));
    }

    #[tokio::test]
    async fn depth_limit_stops_with_a_skip() {
        let mut files: Vec<(String, String)> =
            vec![(MAIN_CONFIG.into(), "Include /etc/ssh/d1\n".into())];
        for i in 1..=5 {
            files.push((
                format!("/etc/ssh/d{i}"),
                format!("Include /etc/ssh/d{}\n", i + 1),
            ));
        }
        files.push(("/etc/ssh/d6".into(), "PermitRootLogin yes\n".into()));
        let refs: Vec<(&str, &str)> = files
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        let cfg = read(&FakeFs::with(&refs)).await;
        assert_eq!(cfg.gap, Some(Gap::Skipped));
        assert_eq!(cfg.files.len(), 5); // main (0) plus depths 1 to 4
        assert!(cfg.permit_root_login.is_none());
    }

    #[tokio::test]
    async fn file_limit_stops_with_a_skip() {
        let mut fs = FakeFs::with(&[(MAIN_CONFIG, "Include sshd_config.d/*.conf\n")]);
        for i in 0..40 {
            fs.files.insert(
                format!("{D}/{i:02}.conf"),
                Entry::Text("# nothing\n".into()),
            );
        }
        let cfg = read(&fs).await;
        assert_eq!(cfg.files.len(), MAX_FILES);
        assert_eq!(cfg.gap, Some(Gap::Skipped));
    }

    #[test]
    fn hostile_include_patterns_are_rejected() {
        for bad in [
            "../shadow",
            "/etc/ssh/../shadow",
            "/etc/passwd",
            "/etc/ssh",
            "/etc/ssh/",
            "/etc/ssh/*/x.conf",
            "/etc/ssh/a;id",
            "/etc/ssh/$(id)",
            "/etc/ssh/`id`",
            "/etc/ssh/a?b",
            "/etc/ssh/[ab]",
            "~/x",
            "/etc/ssh/a\nb",
            "/etc/ssh/a//b",
            "",
        ] {
            assert!(parse_include_pattern(bad).is_err(), "{bad:?}");
        }
        assert_eq!(
            parse_include_pattern("sshd_config.d/*.conf").unwrap(),
            (D.to_owned(), "*.conf".to_owned())
        );
        assert!(parse_include_pattern("/etc/ssh/-oProxy").is_ok()); // only ever a -name value
    }

    #[tokio::test]
    async fn unsupported_include_runs_no_command_and_is_not_a_pass() {
        let fs = FakeFs::with(&[(MAIN_CONFIG, "Include /etc/ssh/$(id)\nPermitRootLogin no\n")]);
        let cfg = read(&fs).await;
        assert_eq!(cfg.gap, Some(Gap::Skipped));
        assert!(cfg.permit_root_login.is_none());
        let cmds = fs.commands.lock().unwrap();
        assert_eq!(cmds.len(), 1, "only the main cat ran: {cmds:?}");
        assert!(cmds.iter().all(|c| !c.contains("find")));
    }

    #[tokio::test]
    async fn find_uses_fixed_argv_and_missing_dir_matches_nothing() {
        let fs = FakeFs::with(&[(
            MAIN_CONFIG,
            "Include nothing.d/*.conf\nPermitRootLogin no\n",
        )]);
        let cfg = read(&fs).await;
        assert!(cfg.gap.is_none());
        assert_eq!(val(&cfg.permit_root_login).unwrap().0, "no");
        let cmds = fs.commands.lock().unwrap();
        assert_eq!(
            cmds[1],
            "'find' '/etc/ssh/nothing.d' '-maxdepth' '1' '(' '-type' 'f' '-o' '-type' 'l' ')' '-name' '*.conf'"
        );
    }

    #[tokio::test]
    async fn connection_loss_aborts() {
        struct Lost;
        #[async_trait]
        impl Executor for Lost {
            async fn run(&self, _: &CommandSpec, _: &CancelToken) -> Result<ExecResult, ExecError> {
                Err(ExecError::ConnectionLost)
            }
        }
        assert_eq!(
            collect(&Lost, &CancelToken::new()).await.unwrap_err(),
            ExecError::ConnectionLost
        );
    }
}
