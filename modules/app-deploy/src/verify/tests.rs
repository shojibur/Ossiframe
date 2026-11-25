use super::*;
use async_trait::async_trait;
use ossiframe_core::{ExecError, ExecResult, ExitStatus};
use std::sync::Mutex;

fn slug() -> AppSlug {
    app_slug("blog").unwrap()
}

fn rid(s: &str) -> ReleaseId {
    ReleaseId::parse(s).unwrap()
}

const NEW: &str = "20250102T000000Z";
const OLD: &str = "20250101T000000Z";

fn resp(status: &str, header: Option<&str>) -> String {
    let mut s = format!("HTTP/1.1 {status}\r\nServer: nginx\r\n");
    if let Some(h) = header {
        s.push_str(&format!("X-Ossiframe-App: {h}\r\n"));
    }
    s.push_str("\r\n");
    s
}

#[test]
fn http_parser_needs_a_good_status_and_the_app_header() {
    let s = slug();
    assert_eq!(
        parse_http(&resp("200 OK", Some("blog")), &s),
        HttpOutcome::Pass { status: 200 }
    );
    assert_eq!(
        parse_http(&resp("302 Found", Some("blog")), &s),
        HttpOutcome::Pass { status: 302 }
    );
    // Header name is case-insensitive.
    assert!(matches!(
        parse_http("HTTP/1.1 200 OK\r\nx-ossiframe-app: blog\r\n\r\n", &s),
        HttpOutcome::Pass { .. }
    ));
    // A 200 from another server block does not pass.
    assert_eq!(
        parse_http(&resp("200 OK", None), &s),
        HttpOutcome::WrongSite { status: 200 }
    );
    assert_eq!(
        parse_http(&resp("200 OK", Some("other")), &s),
        HttpOutcome::WrongSite { status: 200 }
    );
    assert_eq!(
        parse_http(&resp("502 Bad Gateway", Some("blog")), &s),
        HttpOutcome::BadStatus(502)
    );
    assert_eq!(
        parse_http(&resp("404 Not Found", Some("blog")), &s),
        HttpOutcome::BadStatus(404)
    );
    assert_eq!(parse_http("garbage", &s), HttpOutcome::Unparsable);
}

#[test]
fn fpm_and_app_key_parsers_pass_and_fail() {
    assert!(fpm_ok(Some(true), 0).is_ok());
    assert!(fpm_ok(Some(true), 1).is_err());
    assert!(fpm_ok(Some(false), 0).is_err());
    assert!(fpm_ok(None, 0).is_err());
    assert!(app_key_ok(0).is_ok());
    assert!(app_key_ok(1).is_err());
    assert!(app_key_ok(2).is_err());
}

#[test]
fn http_command_is_structured_with_the_host_header() {
    let c = http_command(&DomainName::parse("blog.example.com").unwrap()).unwrap();
    assert_eq!(
        c.to_command_line(),
        "'/usr/bin/curl' '-sS' '-o' '/dev/null' '-D' '-' '--max-time' '5' '-H' 'Host: blog.example.com' 'http://127.0.0.1/'"
    );
}

type Resp = Box<dyn Fn(&CommandSpec) -> (i32, String) + Send + Sync>;
struct Fake {
    log: Mutex<Vec<String>>,
    resp: Resp,
}

#[async_trait]
impl Executor for Fake {
    async fn run(&self, s: &CommandSpec, _: &CancelToken) -> Result<ExecResult, ExecError> {
        self.log.lock().unwrap().push(s.to_command_line());
        let (code, out) = (self.resp)(s);
        Ok(ExecResult {
            exit: ExitStatus::Code(code),
            stdout: out,
            stderr: String::new(),
            truncated: false,
        })
    }
}

fn server(curl: (i32, String), key_exit: i32) -> Fake {
    let site_marker = crate::create::nginx_marker(&slug());
    let pool_marker = crate::create::fpm_marker(&slug());
    Fake {
        log: Mutex::default(),
        resp: Box::new(move |c| match c.program() {
            "/usr/bin/readlink" => (0, format!("/srv/ossiframe/apps/blog/releases/{NEW}\n")),
            "/usr/bin/ls" => (0, format!("{OLD}\n{NEW}\n")),
            "/usr/bin/head" if c.args().last().unwrap().contains("sites-available") => {
                (0, format!("{site_marker}\n"))
            }
            "/usr/bin/head" => (0, format!("{pool_marker}\n")),
            "/usr/bin/systemctl" => (0, "active\nactive\n".to_owned()),
            "/usr/bin/curl" => curl.clone(),
            "/usr/bin/grep" => (key_exit, String::new()),
            _ => (0, String::new()),
        }),
    }
}

fn vin(t: AppType) -> VerifyInput {
    VerifyInput {
        app_type: t,
        slug: slug(),
        domain: DomainName::parse("blog.example.com").unwrap(),
        release: rid(NEW),
    }
}

fn status_of(r: &[CheckResult], id: &str) -> CheckStatus {
    r.iter().find(|c| c.id == id).unwrap().status
}

#[tokio::test]
async fn verify_passes_and_fails_per_check() {
    let c = CancelToken::new();
    let ok = server((0, resp("200 OK", Some("blog"))), 0);
    let r = verify_app(&ok, &c, &vin(AppType::Laravel)).await;
    assert!(
        r.iter()
            .all(|x| x.essential && x.status == CheckStatus::Pass)
    );
    assert_eq!(r.len(), 4);
    // The key value is never read out: grep runs with -q.
    assert!(ok.log.lock().unwrap().iter().any(|l| l.contains("'-q'")));
    // Static has neither the pool check nor the key check.
    let r = verify_app(&ok, &c, &vin(AppType::Static)).await;
    assert_eq!(
        r.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(),
        [CHECK_CURRENT, CHECK_HTTP]
    );

    let bad = server((0, resp("502 Bad Gateway", Some("blog"))), 1);
    let r = verify_app(&bad, &c, &vin(AppType::Laravel)).await;
    assert_eq!(status_of(&r, CHECK_HTTP), CheckStatus::Fail);
    assert_eq!(status_of(&r, CHECK_APP_KEY), CheckStatus::Fail);
    assert_eq!(status_of(&r, CHECK_CURRENT), CheckStatus::Pass);

    // 200 without the header: another site answered.
    let wrong = server((0, resp("200 OK", None)), 0);
    let r = verify_app(&wrong, &c, &vin(AppType::Php)).await;
    assert_eq!(status_of(&r, CHECK_HTTP), CheckStatus::Fail);

    // curl missing: skipped, not passed.
    let none = server((127, String::new()), 0);
    let r = verify_app(&none, &c, &vin(AppType::Php)).await;
    assert_eq!(status_of(&r, CHECK_HTTP), CheckStatus::Skipped);

    // current on another release fails.
    let mut old = vin(AppType::Php);
    old.release = rid(OLD);
    let r = verify_app(&ok, &c, &old).await;
    assert_eq!(status_of(&r, CHECK_CURRENT), CheckStatus::Fail);
}

fn obs(current: CurrentLink) -> AppObservation {
    AppObservation {
        current,
        releases: vec![rid(OLD), rid(NEW)],
        site: FileState::Managed,
        pool: Some(FileState::Managed),
        nginx_active: Some(true),
        fpm_active: Some(true),
    }
}

fn exp(previous: Option<&str>) -> ExpectedApp {
    ExpectedApp {
        app_type: AppType::Laravel,
        release: rid(NEW),
        previous: previous.map(rid),
    }
}

#[test]
fn reconcile_app_decision_table() {
    use AppRecoveryVerdict::*;
    let cur = |s: &str| CurrentLink::Release(rid(s));
    let v = |o: &AppObservation, e: &ExpectedApp, p| reconcile_app(o, e, p).verdict;

    // Deployed: expected release, everything in place, checks pass.
    assert_eq!(v(&obs(cur(NEW)), &exp(Some(OLD)), Some(true)), Deployed);
    // Same, but the checks fail or were not run: not deployed, not a guess.
    assert_eq!(v(&obs(cur(NEW)), &exp(Some(OLD)), Some(false)), Unknown);
    assert_eq!(v(&obs(cur(NEW)), &exp(Some(OLD)), None), Unknown);
    // Expected release but a structural problem.
    let mut o = obs(cur(NEW));
    o.releases = vec![rid(OLD)];
    assert_eq!(v(&o, &exp(Some(OLD)), Some(true)), Unknown);
    let mut o = obs(cur(NEW));
    o.site = FileState::Altered;
    assert_eq!(v(&o, &exp(Some(OLD)), Some(true)), Unknown);
    let mut o = obs(cur(NEW));
    o.pool = Some(FileState::Missing);
    assert_eq!(v(&o, &exp(Some(OLD)), Some(true)), Unknown);
    let mut o = obs(cur(NEW));
    o.fpm_active = Some(false);
    assert_eq!(v(&o, &exp(Some(OLD)), Some(true)), Unknown);
    let mut o = obs(cur(NEW));
    o.nginx_active = None;
    assert_eq!(v(&o, &exp(Some(OLD)), Some(true)), Unknown);

    // Not switched: current is still the previous release, or still absent
    // for a first deploy.
    assert_eq!(v(&obs(cur(OLD)), &exp(Some(OLD)), None), NotSwitched);
    assert_eq!(v(&obs(CurrentLink::Absent), &exp(None), None), NotSwitched);
    // Anything else is unknown.
    assert_eq!(v(&obs(CurrentLink::Absent), &exp(Some(OLD)), None), Unknown);
    assert_eq!(v(&obs(CurrentLink::Other), &exp(Some(OLD)), None), Unknown);
    assert_eq!(
        v(&obs(cur("20240101T000000Z")), &exp(Some(OLD)), None),
        Unknown
    );
    // A static app needs no pool or FPM.
    let mut o = obs(cur(NEW));
    o.pool = None;
    o.fpm_active = None;
    let mut e = exp(Some(OLD));
    e.app_type = AppType::Static;
    assert_eq!(v(&o, &e, Some(true)), Deployed);
}

#[tokio::test]
async fn recover_app_runs_the_checks_only_when_current_is_the_expected_release() {
    let c = CancelToken::new();
    let f = server((0, resp("200 OK", Some("blog"))), 0);
    let r = recover_app(&f, &c, &vin(AppType::Laravel), Some(rid(OLD))).await;
    assert_eq!(r.verdict, AppRecoveryVerdict::Deployed);
    // current is on the old release: not switched, and no curl was sent.
    let mut input = vin(AppType::Laravel);
    input.release = rid("20250103T000000Z");
    let g = server((0, resp("200 OK", Some("blog"))), 0);
    let r = recover_app(&g, &c, &input, Some(rid(NEW))).await;
    assert_eq!(r.verdict, AppRecoveryVerdict::NotSwitched);
    assert!(!g.log.lock().unwrap().iter().any(|l| l.contains("curl")));
}
