//! The `certificates` step. It finds `ssl_certificate` directives in the
//! readable nginx configuration, validates every path before use and asks
//! `openssl` for the expiry date of each certificate once.
//!
//! Scope: only certificates referenced in nginx.conf, conf.d and
//! sites-enabled. It does not claim a certificate is served, never reads key
//! files, never returns whole configs and never connects to a TLS endpoint.
use crate::model::{Category, CheckResult, Outcome, Severity};
use crate::probe::{ProbeFailure, run_probe};
use crate::updates::format_age;
use ossiframe_core::{CancelToken, ExecError, Executor};
use serde_json::json;

pub const RULE: &str = "tls.cert-expiry";
const TITLE: &str = "Certificate expiry";

/// Most nginx config files read.
pub const MAX_FILES: usize = 32;
/// Largest config file read, in bytes.
pub const MAX_FILE_BYTES: usize = 256 * 1024;
/// Most certificate paths checked.
pub const MAX_CERTS: usize = 20;
/// Longest accepted certificate path.
pub const MAX_PATH_LEN: usize = 255;

/// Expired or expiring in under this many seconds: high.
pub const HIGH_WITHIN_SECS: i64 = 14 * 86_400;
/// Expiring in under this many seconds: medium.
pub const MEDIUM_WITHIN_SECS: i64 = 30 * 86_400;

const MAIN_CONF: &str = "/etc/nginx/nginx.conf";
const CONF_D: &str = "/etc/nginx/conf.d";
const SITES_ENABLED: &str = "/etc/nginx/sites-enabled";
const ALLOWED_PREFIXES: [&str; 4] = [
    "/etc/letsencrypt/live/",
    "/etc/letsencrypt/archive/",
    "/etc/ssl/",
    "/etc/nginx/",
];

const NOT_SERVED: &str = "Referenced in config; not verified to be actively served.";
const SCOPE: &str = "Covers only certificates referenced by ssl_certificate in nginx.conf, \
                     conf.d and sites-enabled. Other includes are not followed and no TLS \
                     endpoint is contacted.";

/// Why a certificate path was not accepted. Static text only, so no raw
/// config content reaches a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathReject {
    Empty,
    TooLong,
    NotAbsolute,
    Characters,
    Traversal,
    NotAFile,
    Prefix,
    PrivateKey,
}

impl PathReject {
    fn describe(self) -> &'static str {
        match self {
            PathReject::Empty => "empty value",
            PathReject::TooLong => "path too long",
            PathReject::NotAbsolute => "not an absolute path",
            PathReject::Characters => "variable or unsupported characters",
            PathReject::Traversal => "parent directory component",
            PathReject::NotAFile => "not a file path",
            PathReject::Prefix => "location not allowed",
            PathReject::PrivateKey => "looks like a private key",
        }
    }
}

/// Accepts a path only when it is safe to pass to `openssl` as one argument:
/// absolute, at most 255 characters, `A-Za-z0-9._/-` only, no `..`
/// component, under one of four allowed directories and not a private key.
pub fn validate_cert_path(p: &str) -> Result<&str, PathReject> {
    if p.is_empty() {
        return Err(PathReject::Empty);
    }
    if p.len() > MAX_PATH_LEN {
        return Err(PathReject::TooLong);
    }
    if !p.starts_with('/') {
        return Err(PathReject::NotAbsolute);
    }
    if !p
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'-'))
    {
        return Err(PathReject::Characters);
    }
    if p.split('/').any(|c| c == "..") {
        return Err(PathReject::Traversal);
    }
    if p.ends_with('/') {
        return Err(PathReject::NotAFile);
    }
    if !ALLOWED_PREFIXES.iter().any(|x| p.starts_with(x)) {
        return Err(PathReject::Prefix);
    }
    let name = p.rsplit('/').next().unwrap_or("");
    if p.split('/').any(|c| c == "private") || name.starts_with("privkey") || name.ends_with(".key")
    {
        return Err(PathReject::PrivateKey);
    }
    Ok(p)
}

/// Values of `ssl_certificate` directives, unvalidated. `ssl_certificate_key`
/// and every other directive are ignored. A `#` starts a comment only at the
/// start of a word, as in nginx. A directive split over several lines is not
/// recognized.
pub fn extract_ssl_certificates(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut end = line.len();
        let mut prev_boundary = true;
        for (i, c) in line.char_indices() {
            if c == '#' && prev_boundary {
                end = i;
                break;
            }
            prev_boundary = c.is_whitespace() || matches!(c, ';' | '{' | '}');
        }
        let parts: Vec<&str> = line[..end].split(';').collect();
        // The last piece has no terminating `;`, so it is not a full directive.
        for piece in &parts[..parts.len() - 1] {
            let stmt = piece.rsplit(['{', '}']).next().unwrap_or("").trim();
            let (kw, rest) = match stmt.split_once(char::is_whitespace) {
                Some((k, r)) => (k, r.trim()),
                None => (stmt, ""),
            };
            if kw == "ssl_certificate" {
                out.push(rest.to_owned());
            }
        }
    }
    out
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Parses `openssl x509 -enddate` output (`notAfter=Jun  1 12:00:00 2026 GMT`)
/// to Unix seconds. Anything else is an error, never a date.
pub fn parse_enddate(output: &str) -> Result<u64, &'static str> {
    const BAD: &str = "the certificate end date was not recognized";
    let line = output
        .lines()
        .find_map(|l| l.trim().strip_prefix("notAfter="))
        .ok_or(BAD)?;
    let w: Vec<&str> = line.split_whitespace().collect();
    let [mon, day, time, year, zone] = w.as_slice() else {
        return Err(BAD);
    };
    if *zone != "GMT" {
        return Err(BAD);
    }
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let m = MONTHS.iter().position(|x| x == mon).ok_or(BAD)? as i64 + 1;
    let y: i64 = year.parse().map_err(|_| BAD)?;
    let d: i64 = day.parse().map_err(|_| BAD)?;
    if !(1970..=9999).contains(&y) || d < 1 || d > days_in_month(y, m) {
        return Err(BAD);
    }
    let t: Vec<&str> = time.split(':').collect();
    let [hh, mm, ss] = t.as_slice() else {
        return Err(BAD);
    };
    let (hh, mm, ss): (i64, i64, i64) = (
        hh.parse().map_err(|_| BAD)?,
        mm.parse().map_err(|_| BAD)?,
        ss.parse().map_err(|_| BAD)?,
    );
    if !(0..24).contains(&hh) || !(0..60).contains(&mm) || !(0..60).contains(&ss) {
        return Err(BAD);
    }
    u64::try_from(days_from_civil(y, m, d) * 86_400 + hh * 3_600 + mm * 60 + ss).map_err(|_| BAD)
}

/// A validated certificate path and the nginx file that references it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertRef {
    pub path: String,
    pub file: String,
}

/// Result of the nginx config discovery.
#[derive(Debug, Default)]
pub struct Discovery {
    /// At least one nginx config file was found or could not be listed.
    pub config_found: bool,
    pub files_read: Vec<String>,
    pub certs: Vec<CertRef>,
    /// One entry per gap: limit hit, unreadable file or skipped path.
    pub limitations: Vec<String>,
    /// First read or listing failure, if any.
    pub failure: Option<ProbeFailure>,
}

/// One certificate and the seconds until it expires (negative when expired).
#[derive(Debug)]
pub struct CertRead {
    pub cert: CertRef,
    pub remaining: Result<i64, ProbeFailure>,
}

pub struct Collected {
    pub discovery: Discovery,
    pub certs: Vec<CertRead>,
}

fn plain_name(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('.')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Config files nginx loads by default from `dir`: `*.conf` in conf.d and
/// any visible file in sites-enabled.
fn dir_candidates(dir: &str, listing: &str, d: &mut Discovery) -> Vec<String> {
    let mut names: Vec<&str> = listing.lines().filter(|l| !l.is_empty()).collect();
    names.sort_unstable();
    let mut out = Vec::new();
    for path in names {
        let Some(name) = path
            .strip_prefix(dir)
            .and_then(|r| r.strip_prefix('/'))
            .filter(|n| plain_name(n))
        else {
            if path.strip_prefix(dir).is_none_or(|r| !r.starts_with("/.")) {
                d.limitations.push(format!(
                    "A file in {dir} had an unsupported name and was not read."
                ));
            }
            continue;
        };
        if dir == CONF_D && !name.ends_with(".conf") {
            continue;
        }
        out.push(path.to_owned());
    }
    out
}

/// Finds and reads the nginx config. `Err` only for cancel and connection
/// loss. Every other problem is recorded in the result.
pub async fn discover(exec: &dyn Executor, cancel: &CancelToken) -> Result<Discovery, ExecError> {
    let mut d = Discovery::default();
    let mut candidates: Vec<String> = Vec::new();
    for root in [MAIN_CONF, CONF_D, SITES_ENABLED] {
        let args = [
            root,
            "-maxdepth",
            "1",
            "(",
            "-type",
            "f",
            "-o",
            "-type",
            "l",
            ")",
        ];
        match run_probe(exec, cancel, "find", &args).await? {
            Ok(out) if root == MAIN_CONF => {
                if out.lines().any(|l| l == MAIN_CONF) {
                    candidates.push(MAIN_CONF.to_owned());
                }
            }
            Ok(out) => candidates.extend(dir_candidates(root, &out, &mut d)),
            // A missing path is normal: nginx may not use it.
            Err(ProbeFailure::Missing) => {}
            Err(f) => {
                d.limitations.push(format!(
                    "{root} could not be listed ({}); its files were not read.",
                    f.reason()
                ));
                d.failure.get_or_insert(f);
                d.config_found = true;
            }
        }
    }
    if !candidates.is_empty() {
        d.config_found = true;
    }
    if candidates.len() > MAX_FILES {
        candidates.truncate(MAX_FILES);
        d.limitations.push(format!(
            "More than {MAX_FILES} nginx config files; the rest were not read."
        ));
    }
    let mut skipped_overflow = false;
    for file in candidates {
        let text = match run_probe(exec, cancel, "cat", &[file.as_str()]).await? {
            Ok(t) if t.len() <= MAX_FILE_BYTES => t,
            Ok(_) | Err(ProbeFailure::Truncated) => {
                d.limitations.push(format!(
                    "{file} is larger than {} KB and was not read.",
                    MAX_FILE_BYTES / 1024
                ));
                continue;
            }
            Err(f) => {
                d.limitations
                    .push(format!("{file} could not be read ({}).", f.reason()));
                d.failure.get_or_insert(f);
                continue;
            }
        };
        d.files_read.push(file.clone());
        let mut skipped: Vec<&'static str> = Vec::new();
        for raw in extract_ssl_certificates(&text) {
            match validate_cert_path(&raw) {
                Ok(path) if d.certs.iter().any(|c| c.path == path) => {}
                Ok(_) if d.certs.len() >= MAX_CERTS => skipped_overflow = true,
                Ok(path) => d.certs.push(CertRef {
                    path: path.to_owned(),
                    file: file.clone(),
                }),
                Err(why) => {
                    if !skipped.contains(&why.describe()) {
                        skipped.push(why.describe());
                    }
                }
            }
        }
        if !skipped.is_empty() {
            d.limitations.push(format!(
                "Some ssl_certificate values in {file} were skipped: {}.",
                skipped.join(", ")
            ));
        }
    }
    if skipped_overflow {
        d.limitations.push(format!(
            "More than {MAX_CERTS} certificate paths; the rest were not checked."
        ));
    }
    Ok(d)
}

fn parse_now(text: &str) -> Result<u64, ProbeFailure> {
    text.trim()
        .parse()
        .map_err(|_| ProbeFailure::Error("the server clock was not a number".to_owned()))
}

/// Discovery, then one `openssl x509 -enddate` per certificate. The server
/// clock is read once. `Err` only for cancel and connection loss.
pub async fn collect(exec: &dyn Executor, cancel: &CancelToken) -> Result<Collected, ExecError> {
    let discovery = discover(exec, cancel).await?;
    let mut certs = Vec::new();
    if !discovery.certs.is_empty() {
        let now = run_probe(exec, cancel, "date", &["-u", "+%s"])
            .await?
            .and_then(|t| parse_now(&t));
        for cert in &discovery.certs {
            let remaining = match &now {
                Err(f) => Err(f.clone()),
                Ok(now) => {
                    // The path was validated: absolute, safe characters only.
                    let args = ["x509", "-enddate", "-noout", "-in", cert.path.as_str()];
                    run_probe(exec, cancel, "openssl", &args)
                        .await?
                        .and_then(|t| {
                            parse_enddate(&t).map_err(|m| ProbeFailure::Error(m.to_owned()))
                        })
                        .map(|end| end as i64 - *now as i64)
                }
            };
            certs.push(CertRead {
                cert: cert.clone(),
                remaining,
            });
        }
    }
    Ok(Collected { discovery, certs })
}

/// `tls.cert-expiry` for one certificate.
pub fn cert_expiry(
    cert: &CertRef,
    remaining: &Result<i64, ProbeFailure>,
    observed_at: &str,
) -> CheckResult {
    let evidence = format!("ssl_certificate {}; ({})", cert.path, cert.file);
    let c = match remaining {
        Err(f) => {
            let mut c = f.into_check(RULE, Category::Security, TITLE, false, observed_at);
            if *f == ProbeFailure::PermissionDenied && cert.path.starts_with("/etc/letsencrypt/") {
                c = c
                    .limitation("Files under /etc/letsencrypt are normally readable by root only.");
            }
            c
        }
        Ok(r) if *r <= 0 => CheckResult::finding(
            RULE,
            Category::Security,
            Severity::High,
            TITLE,
            format!(
                "The certificate expired {} ago.",
                format_age(r.unsigned_abs())
            ),
            observed_at,
        )
        .recommendation("Renew the certificate and reload nginx."),
        Ok(r) if *r < MEDIUM_WITHIN_SECS => {
            let severity = if *r < HIGH_WITHIN_SECS {
                Severity::High
            } else {
                Severity::Medium
            };
            CheckResult::finding(
                RULE,
                Category::Security,
                severity,
                TITLE,
                format!("The certificate expires in {}.", format_age(*r as u64)),
                observed_at,
            )
            .recommendation("Renew the certificate before it expires.")
        }
        Ok(r) => CheckResult::new(
            RULE,
            Category::Security,
            Outcome::Passed,
            TITLE,
            format!("The certificate expires in {}.", format_age(*r as u64)),
            observed_at,
        ),
    };
    c.resource(&cert.path)
        .evidence([evidence])
        .limitation(NOT_SERVED)
}

/// The assessment-wide result: `notApplicable`, `unavailable` or
/// `incomplete`. `None` when the per-certificate checks stand alone.
fn overall(d: &Discovery, has_certs: bool, observed_at: &str) -> Option<CheckResult> {
    let mk = |outcome, text: String| {
        let mut c = CheckResult::new(RULE, Category::Security, outcome, TITLE, text, observed_at)
            .limitation(SCOPE);
        for l in &d.limitations {
            c = c.limitation(l.clone());
        }
        c
    };
    if !d.config_found {
        return Some(mk(
            Outcome::NotApplicable,
            "No nginx configuration was found.".to_owned(),
        ));
    }
    if d.files_read.is_empty() {
        let (outcome, why) = match &d.failure {
            Some(f) => (f.outcome(false), f.reason()),
            None => (Outcome::Incomplete, "see limitations"),
        };
        return Some(mk(
            outcome,
            format!("Not assessed: nginx configuration could not be read ({why})."),
        ));
    }
    let gaps = !d.limitations.is_empty();
    match (has_certs, gaps) {
        (true, false) => None,
        (_, true) => Some(mk(
            Outcome::Incomplete,
            "Not fully assessed: discovery hit a limit, a file could not be read or a path \
             was skipped, so other certificates may exist."
                .to_owned(),
        )),
        (false, false) => Some(mk(
            Outcome::NotApplicable,
            "The inspected nginx configuration has no ssl_certificate directive.".to_owned(),
        )),
    }
}

/// All checks of the step. Findings are always kept. Without a finding,
/// an incomplete discovery gives an `incomplete` result, never a whole-
/// assessment pass.
pub fn assess(c: &Collected, observed_at: &str) -> Vec<CheckResult> {
    let gaps = !c.discovery.limitations.is_empty();
    let mut checks: Vec<CheckResult> = c
        .certs
        .iter()
        .map(|r| {
            let check = cert_expiry(&r.cert, &r.remaining, observed_at);
            if gaps {
                check.limitation(
                    "Certificate discovery was incomplete; other certificates may exist.",
                )
            } else {
                check
            }
        })
        .collect();
    if checks.iter().any(|x| x.outcome == Outcome::Finding) {
        return checks;
    }
    checks.extend(overall(&c.discovery, !c.certs.is_empty(), observed_at));
    checks
}

/// Step evidence: paths, config file names and expiry distance only.
pub fn evidence(c: &Collected) -> serde_json::Value {
    json!({
        "certs": c.certs.iter().map(|r| json!({
            "path": r.cert.path,
            "configFile": r.cert.file,
            "secondsUntilExpiry": r.remaining.as_ref().ok(),
        })).collect::<Vec<_>>(),
        "filesRead": c.discovery.files_read,
        "configFound": c.discovery.config_found,
        "limitations": c.discovery.limitations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use ossiframe_core::{CommandSpec, ExecResult, ExitStatus};
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    const AT: &str = "2025-01-01T00:00:00Z";
    const NOW: i64 = 1_800_000_000;
    const DAY: i64 = 86_400;

    #[test]
    fn validator_accepts_expected_paths() {
        for p in [
            "/etc/letsencrypt/live/example.com/fullchain.pem",
            "/etc/letsencrypt/archive/example.com/cert1.pem",
            "/etc/ssl/certs/site.crt",
            "/etc/nginx/ssl/site.pem",
        ] {
            assert_eq!(validate_cert_path(p), Ok(p));
        }
        assert_eq!(
            validate_cert_path(&format!("/etc/ssl/{}", "a".repeat(246))).map(str::len),
            Ok(255)
        );
    }

    #[test]
    fn validator_rejects_hostile_and_unsupported_paths() {
        let cases = [
            ("", PathReject::Empty),
            ("relative/path.pem", PathReject::NotAbsolute),
            ("-in", PathReject::NotAbsolute),
            ("-----BEGIN", PathReject::NotAbsolute),
            ("/etc/ssl/../shadow", PathReject::Traversal),
            ("/etc/ssl/certs/..", PathReject::Traversal),
            ("/etc/nginx/$ssl_name/cert.pem", PathReject::Characters),
            ("${host}", PathReject::NotAbsolute),
            ("\"/etc/ssl/a.pem\"", PathReject::NotAbsolute),
            ("/etc/ssl/'a'.pem", PathReject::Characters),
            ("/etc/ssl/a.pem\nb", PathReject::Characters),
            ("/etc/ssl/a b.pem", PathReject::Characters),
            ("/etc/ssl/a;rm.pem", PathReject::Characters),
            ("/etc/ssl/dir/", PathReject::NotAFile),
            ("/etc/passwd", PathReject::Prefix),
            ("/etc/sslx/a.pem", PathReject::Prefix),
            ("/etc/ssl", PathReject::Prefix),
            ("/etc/ssl/private/a.pem", PathReject::PrivateKey),
            (
                "/etc/letsencrypt/live/x/privkey.pem",
                PathReject::PrivateKey,
            ),
            ("/etc/nginx/ssl/site.key", PathReject::PrivateKey),
        ];
        for (p, why) in cases {
            assert_eq!(validate_cert_path(p), Err(why), "{p:?}");
        }
        let long = format!("/etc/ssl/{}", "a".repeat(250));
        assert_eq!(validate_cert_path(&long), Err(PathReject::TooLong));
    }

    #[test]
    fn extracts_certificate_directives_only() {
        let conf = "\
server {
    listen 443 ssl; ssl_certificate /etc/ssl/a.pem; ssl_certificate_key /etc/ssl/private/a.key;
    # ssl_certificate /etc/ssl/commented.pem;
    ssl_certificate   /etc/ssl/b.pem;  # trailing
    ssl_certificate /etc/ssl/c#d.pem;
}
server { ssl_certificate $var; }
ssl_certificate /etc/ssl/unterminated.pem
ssl_certificate_key /etc/ssl/private/k.key;
";
        assert_eq!(
            extract_ssl_certificates(conf),
            [
                "/etc/ssl/a.pem",
                "/etc/ssl/b.pem",
                "/etc/ssl/c#d.pem",
                "$var"
            ]
        );
    }

    #[test]
    fn parses_enddate() {
        assert_eq!(parse_enddate("notAfter=Jan  1 00:00:00 1970 GMT\n"), Ok(0));
        assert_eq!(
            parse_enddate("notAfter=Jan  1 00:00:00 2000 GMT"),
            Ok(946_684_800)
        );
        assert_eq!(
            parse_enddate("notAfter=Feb 29 00:00:00 2024 GMT"),
            Ok(1_709_164_800)
        );
        assert_eq!(
            parse_enddate("notAfter=Jun 15 12:30:45 2026 GMT"),
            Ok(1_781_526_645)
        );
        for bad in [
            "",
            "garbage",
            "notBefore=Jan  1 00:00:00 2000 GMT",
            "notAfter=Feb 30 00:00:00 2024 GMT",
            "notAfter=Feb 29 00:00:00 2023 GMT",
            "notAfter=Foo  1 00:00:00 2024 GMT",
            "notAfter=Jan  1 24:00:00 2024 GMT",
            "notAfter=Jan  1 00:00:00 2024 UTC",
            "notAfter=Jan  1 00:00:00 1969 GMT",
            "notAfter=Jan  1 00:00 2024 GMT",
        ] {
            assert!(parse_enddate(bad).is_err(), "{bad:?}");
        }
    }

    fn cert() -> CertRef {
        CertRef {
            path: "/etc/ssl/a.pem".to_owned(),
            file: "/etc/nginx/sites-enabled/a".to_owned(),
        }
    }

    fn expiry(remaining: i64) -> CheckResult {
        cert_expiry(&cert(), &Ok(remaining), AT)
    }

    #[test]
    fn expiry_thresholds() {
        let sev = |r| (expiry(r).outcome, expiry(r).severity);
        assert_eq!(sev(-DAY), (Outcome::Finding, Some(Severity::High)));
        assert_eq!(sev(0), (Outcome::Finding, Some(Severity::High)));
        assert_eq!(sev(1), (Outcome::Finding, Some(Severity::High)));
        assert_eq!(sev(14 * DAY - 1), (Outcome::Finding, Some(Severity::High)));
        assert_eq!(sev(14 * DAY), (Outcome::Finding, Some(Severity::Medium)));
        assert_eq!(
            sev(30 * DAY - 1),
            (Outcome::Finding, Some(Severity::Medium))
        );
        assert_eq!(sev(30 * DAY), (Outcome::Passed, None));
        let p = expiry(90 * DAY);
        assert!(
            p.limitations
                .iter()
                .any(|l| l.contains("not verified to be actively served"))
        );
        assert!(p.explanation.contains("90 days"));
    }

    #[test]
    fn unreadable_certificate_is_unavailable() {
        let c = cert_expiry(&cert(), &Err(ProbeFailure::PermissionDenied), AT);
        assert_eq!(c.outcome, Outcome::Unavailable);
        let c = cert_expiry(&cert(), &Err(ProbeFailure::Error("x".to_owned())), AT);
        assert_eq!(c.outcome, Outcome::Error);
    }

    enum Cert {
        Valid(i64),
        Denied,
        NoOpenssl,
    }

    #[derive(Default)]
    struct Fake {
        files: BTreeMap<String, String>,
        certs: BTreeMap<String, Cert>,
        denied_dirs: Vec<&'static str>,
        commands: Mutex<Vec<String>>,
    }

    fn done(code: i32, out: &str, err: &str) -> Result<ExecResult, ExecError> {
        Ok(ExecResult {
            exit: ExitStatus::Code(code),
            stdout: out.to_owned(),
            stderr: err.to_owned(),
            truncated: false,
        })
    }

    #[async_trait]
    impl Executor for Fake {
        async fn run(&self, spec: &CommandSpec, _: &CancelToken) -> Result<ExecResult, ExecError> {
            self.commands.lock().unwrap().push(spec.to_command_line());
            let a = spec.args();
            match spec.program() {
                "find" => {
                    assert_eq!(
                        a[1..],
                        ["-maxdepth", "1", "(", "-type", "f", "-o", "-type", "l", ")"]
                    );
                    if self.denied_dirs.contains(&a[0].as_str()) {
                        return done(1, "", "find: Permission denied");
                    }
                    let dir = format!("{}/", a[0]);
                    let hits: Vec<&str> = self
                        .files
                        .keys()
                        .filter(|k| **k == a[0] || k.starts_with(&dir))
                        .map(String::as_str)
                        .collect();
                    if hits.is_empty() {
                        return done(1, "", "find: No such file or directory");
                    }
                    done(0, &hits.join("\n"), "")
                }
                "cat" => match self.files.get(&a[0]) {
                    Some(t) => done(0, t, ""),
                    None => done(1, "", "cat: Permission denied"),
                },
                "date" => done(0, &format!("{NOW}\n"), ""),
                "openssl" => {
                    assert_eq!(a[..4], ["x509", "-enddate", "-noout", "-in"]);
                    match self.certs.get(&a[4]) {
                        Some(Cert::Valid(r)) => {
                            let end = (NOW + r) as u64;
                            let (days, secs) = (end / 86_400, end % 86_400);
                            // Reuse the parser's inverse: build text from a known date table.
                            done(0, &format!("notAfter={}\n", gmt(days, secs)), "")
                        }
                        Some(Cert::Denied) => done(1, "", "Could not open file: Permission denied"),
                        Some(Cert::NoOpenssl) => done(127, "", "openssl: command not found"),
                        None => done(1, "", "Could not open file: No such file or directory"),
                    }
                }
                other => panic!("unexpected program {other}"),
            }
        }
    }

    /// Formats days since the epoch and seconds of day as an openssl date.
    fn gmt(days: u64, secs: u64) -> String {
        const MONTHS: [&str; 12] = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        let (mut y, mut left) = (1970i64, days as i64);
        loop {
            let len = if days_in_month(y, 2) == 29 { 366 } else { 365 };
            if left < len {
                break;
            }
            left -= len;
            y += 1;
        }
        let mut m = 1;
        while left >= days_in_month(y, m) {
            left -= days_in_month(y, m);
            m += 1;
        }
        format!(
            "{} {:>2} {:02}:{:02}:{:02} {} GMT",
            MONTHS[m as usize - 1],
            left + 1,
            secs / 3600,
            secs % 3600 / 60,
            secs % 60,
            y
        )
    }

    fn fake(files: &[(&str, &str)], certs: Vec<(&str, Cert)>) -> Fake {
        Fake {
            files: files
                .iter()
                .map(|(p, t)| ((*p).to_owned(), (*t).to_owned()))
                .collect(),
            certs: certs.into_iter().map(|(p, c)| (p.to_owned(), c)).collect(),
            ..Fake::default()
        }
    }

    async fn run(f: &Fake) -> Vec<CheckResult> {
        let c = collect(f, &CancelToken::new()).await.unwrap();
        assess(&c, AT)
    }

    const SITE: &str = "/etc/nginx/sites-enabled/site";

    #[test]
    fn gmt_helper_round_trips() {
        for r in [0, 5 * DAY + 77, 400 * DAY] {
            let end = (NOW + r) as u64;
            assert_eq!(
                parse_enddate(&format!("notAfter={}", gmt(end / 86_400, end % 86_400))),
                Ok(end)
            );
        }
    }

    #[tokio::test]
    async fn no_nginx_config_is_not_applicable() {
        let r = run(&fake(&[], vec![])).await;
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].outcome, Outcome::NotApplicable);
    }

    #[tokio::test]
    async fn healthy_certificate_passes_with_limitation_and_runs_one_openssl() {
        let f = fake(
            &[
                (
                    SITE,
                    "ssl_certificate /etc/ssl/a.pem;\nssl_certificate_key /etc/ssl/private/a.key;",
                ),
                (
                    "/etc/nginx/conf.d/x.conf",
                    "ssl_certificate /etc/ssl/a.pem;",
                ),
            ],
            vec![("/etc/ssl/a.pem", Cert::Valid(90 * DAY))],
        );
        let r = run(&f).await;
        assert_eq!(r.len(), 1, "{r:?}");
        assert_eq!(r[0].outcome, Outcome::Passed);
        assert!(
            r[0].limitations
                .iter()
                .any(|l| l.contains("not verified to be actively served"))
        );
        let cmds = f.commands.lock().unwrap();
        assert_eq!(cmds.iter().filter(|c| c.contains("openssl")).count(), 1);
        assert!(
            cmds.iter()
                .all(|c| !c.contains(".key") && !c.contains("sudo"))
        );
    }

    #[tokio::test]
    async fn expiring_certificate_is_a_finding_even_with_skipped_paths() {
        let f = fake(
            &[(
                SITE,
                "ssl_certificate /etc/ssl/a.pem;\nssl_certificate $dyn;",
            )],
            vec![("/etc/ssl/a.pem", Cert::Valid(10 * DAY))],
        );
        let r = run(&f).await;
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].outcome, Outcome::Finding);
        assert_eq!(r[0].severity, Some(Severity::High));
        assert_eq!(
            r[0].evidence,
            ["ssl_certificate /etc/ssl/a.pem; (/etc/nginx/sites-enabled/site)"]
        );
    }

    #[tokio::test]
    async fn skipped_path_without_finding_is_incomplete() {
        let f = fake(
            &[(
                SITE,
                "ssl_certificate /etc/ssl/a.pem;\nssl_certificate /root/x.pem;",
            )],
            vec![("/etc/ssl/a.pem", Cert::Valid(90 * DAY))],
        );
        let r = run(&f).await;
        assert_eq!(r.last().unwrap().outcome, Outcome::Incomplete);
        assert!(
            r.iter()
                .any(|c| c.rule_id == RULE && c.limitations.iter().any(|l| l.contains("skipped")))
        );
        // The skipped raw value is never echoed.
        assert!(!format!("{r:?}").contains("/root/x.pem"));
        let f = fake(&[(SITE, "ssl_certificate $dyn;")], vec![]);
        let r = run(&f).await;
        assert_eq!((r.len(), r[0].outcome), (1, Outcome::Incomplete));
        assert!(
            f.commands
                .lock()
                .unwrap()
                .iter()
                .all(|c| !c.contains("openssl"))
        );
    }

    #[tokio::test]
    async fn unreadable_config_file_is_incomplete_or_unavailable() {
        // One readable file with a good cert, one unreadable (listed but cat denied).
        let mut f = fake(
            &[(SITE, "ssl_certificate /etc/ssl/a.pem;")],
            vec![("/etc/ssl/a.pem", Cert::Valid(90 * DAY))],
        );
        f.files
            .insert("/etc/nginx/conf.d/x.conf".to_owned(), String::new());
        f.denied_dirs.push("/etc/nginx/sites-enabled");
        let r = run(&f).await;
        assert!(r.iter().any(|c| c.outcome == Outcome::Incomplete));
        // Nothing readable at all.
        let mut f = fake(&[], vec![]);
        f.denied_dirs = vec![MAIN_CONF, CONF_D, SITES_ENABLED];
        let r = run(&f).await;
        assert_eq!((r.len(), r[0].outcome), (1, Outcome::Unavailable));
    }

    #[tokio::test]
    async fn config_without_directives_is_not_applicable() {
        let r = run(&fake(&[(MAIN_CONF, "events {}\nhttp { }")], vec![])).await;
        assert_eq!((r.len(), r[0].outcome), (1, Outcome::NotApplicable));
    }

    #[tokio::test]
    async fn unreadable_or_missing_openssl_is_unavailable_never_passed() {
        for c in [Cert::Denied, Cert::NoOpenssl] {
            let f = fake(
                &[(
                    SITE,
                    "ssl_certificate /etc/letsencrypt/live/x/fullchain.pem;",
                )],
                vec![("/etc/letsencrypt/live/x/fullchain.pem", c)],
            );
            let r = run(&f).await;
            assert!(r.iter().all(|c| c.outcome != Outcome::Passed), "{r:?}");
            assert_eq!(r[0].outcome, Outcome::Unavailable);
        }
    }

    #[tokio::test]
    async fn file_and_certificate_limits_are_incomplete() {
        let mut f = fake(&[], vec![]);
        for i in 0..33 {
            f.files
                .insert(format!("/etc/nginx/conf.d/{i:02}.conf"), String::new());
        }
        let r = run(&f).await;
        assert_eq!((r.len(), r[0].outcome), (1, Outcome::Incomplete));
        let text: String = (0..21)
            .map(|i| format!("ssl_certificate /etc/ssl/c{i}.pem;\n"))
            .collect();
        let certs = (0..21)
            .map(|i| (format!("/etc/ssl/c{i}.pem"), Cert::Valid(90 * DAY)))
            .collect::<Vec<_>>();
        let mut f = fake(&[(SITE, &text)], vec![]);
        f.certs = certs.into_iter().collect();
        let r = run(&f).await;
        assert_eq!(
            r.iter().filter(|c| c.outcome == Outcome::Passed).count(),
            MAX_CERTS
        );
        assert_eq!(r.last().unwrap().outcome, Outcome::Incomplete);
    }

    #[tokio::test]
    async fn oversize_config_is_not_read_and_not_passed() {
        let big = format!(
            "ssl_certificate /etc/ssl/a.pem;\n{}",
            "#".repeat(MAX_FILE_BYTES)
        );
        let r = run(&fake(&[(SITE, &big)], vec![])).await;
        assert_eq!((r.len(), r[0].outcome), (1, Outcome::Incomplete));
    }
}
