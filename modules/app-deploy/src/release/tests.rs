use super::*;
use async_trait::async_trait;
use ossiframe_core::{
    ExecError, ExecResult, ExitStatus, PlanTarget, Preconditions, Resources, SudoMode,
};
use std::collections::BTreeMap;
use std::sync::Mutex;

type Resp = Box<dyn Fn(&CommandSpec) -> (i32, String) + Send + Sync>;

struct Fake {
    log: Mutex<Vec<String>>,
    resp: Resp,
}

impl Fake {
    fn new(resp: impl Fn(&CommandSpec) -> (i32, String) + Send + Sync + 'static) -> Self {
        Self {
            log: Mutex::default(),
            resp: Box::new(resp),
        }
    }
    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
    fn saw(&self, needle: &str) -> bool {
        self.log().iter().any(|l| l.contains(needle))
    }
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

const RID: &str = "20250101T000000Z";

fn input(t: AppType) -> DeployInput {
    DeployInput {
        app_type: t,
        slug: app_slug("blog").unwrap(),
        url: GitUrl::parse("https://github.com/o/r.git").unwrap(),
        branch: BranchName::parse("main").unwrap(),
        entry_file: EntryFile::parse(t.info().entry_file).unwrap(),
        release: ReleaseId::parse(RID).unwrap(),
        has_env: t != AppType::Static,
        run_migrations: false,
        access: None,
    }
}

fn ids(steps: &[PlanStep]) -> Vec<&str> {
    steps.iter().map(|s| s.id.as_str()).collect()
}

fn plan(steps: Vec<PlanStep>) -> Plan {
    Plan {
        target: PlanTarget {
            server_id: "s".into(),
            name: "n".into(),
            host: "h".into(),
            port: 22,
            user: "u".into(),
            host_key_sha256: "k".into(),
            app_id: None,
        },
        os: "ubuntu 24.04".into(),
        components: vec![],
        already_present: vec![],
        packages: vec![],
        files: vec![],
        services: vec![],
        resources: Resources {
            disk_mb_needed: 1,
            mem_mb_min: 1,
        },
        privileges: vec![],
        limitations: vec![],
        steps,
        preconditions: Preconditions {
            os_version: "24.04".into(),
            packages: BTreeMap::new(),
            sudo: SudoMode::Passwordless,
            package_sources: vec![],
            watched_listings: BTreeMap::new(),
            min_mem_mb: 1,
            min_disk_mb: 1,
            server_managed: false,
        },
    }
}

fn rid(s: &str) -> ReleaseId {
    ReleaseId::parse(s).unwrap()
}

fn root() -> AppRoot {
    AppRoot::new(&app_slug("blog").unwrap()).unwrap()
}

#[test]
fn release_id_round_trip_and_rejects_bad_input() {
    assert_eq!(ReleaseId::from_unix_secs(0).as_str(), "19700101T000000Z");
    assert_eq!(
        ReleaseId::from_unix_secs(1_700_000_000).as_str(),
        "20231114T221320Z"
    );
    assert_eq!(
        ReleaseId::from_unix_secs(1_709_251_199).as_str(),
        "20240229T235959Z"
    );
    assert!(ReleaseId::parse(ReleaseId::now().as_str()).is_ok());
    for bad in [
        "",
        "../x",
        "20250101T000000",
        "20251301T000000Z",
        "20250101T240000Z",
        "20250101t000000Z",
        "2025010aT000000Z",
        "20250101T000000Z/",
    ] {
        assert!(ReleaseId::parse(bad).is_err(), "{bad}");
    }
}

#[test]
fn every_build_boot_and_switch_step_runs_as_the_app_user() {
    let user = app_user(&app_slug("blog").unwrap()).unwrap();
    for t in AppType::ALL {
        let mut i = input(t);
        i.run_migrations = t == AppType::Laravel;
        let s = redeploy_steps(&i).unwrap().steps;
        for step in &s {
            let p = step.command.privilege();
            match step.id.as_str() {
                STEP_USER_VERIFY => assert_eq!(p, Privilege::User),
                STEP_FPM_RELOAD => assert_eq!(p, Privilege::Root),
                id => assert_eq!(p, Privilege::RunAs(user.clone()), "{t:?} {id}"),
            }
            assert!(!step.command.has_stdin() && !step.requires_stdin);
        }
        let boot = s.iter().find(|x| x.id == STEP_BOOT_CHECK).unwrap();
        let dir = format!("/srv/ossiframe/apps/blog/releases/{RID}");
        let (prog, args): (&str, Vec<String>) = match t {
            AppType::Laravel => (PHP, vec![format!("{dir}/artisan"), "--version".into()]),
            AppType::Php => (PHP, vec!["-l".into(), format!("{dir}/public/index.php")]),
            AppType::Static => (TEST, vec!["-f".into(), format!("{dir}/index.html")]),
        };
        assert_eq!(boot.command.program(), prog);
        assert_eq!(boot.command.args(), args.as_slice());
        // Boot check comes after every build step and before the switch.
        let pos = |id: &str| s.iter().position(|x| x.id == id).unwrap();
        assert!(pos(STEP_BOOT_CHECK) < pos(STEP_SWITCH_LINK));
        assert!(pos(STEP_SWITCH_LINK) < pos(STEP_SWITCH));
        assert!(pos(STEP_SWITCH) < pos(STEP_CLEANUP));
        for build in [STEP_COMPOSER, STEP_MIGRATE, STEP_VIEW_CACHE] {
            if s.iter().any(|x| x.id == build) {
                assert!(pos(build) < pos(STEP_BOOT_CHECK), "{build}");
            }
        }
    }
}

#[test]
fn long_laravel_steps_are_durable_and_the_clone_is_too() {
    let mut i = input(AppType::Laravel);
    i.run_migrations = true;
    let s = deploy_steps(&i).unwrap().steps;
    for id in [STEP_CLONE, STEP_COMPOSER, STEP_MIGRATE] {
        assert_eq!(
            s.iter().find(|x| x.id == id).unwrap().mode,
            StepMode::Durable
        );
    }
}

#[test]
fn migrate_runs_only_when_opted_in_and_only_for_laravel() {
    let mut i = input(AppType::Laravel);
    assert!(!ids(&deploy_steps(&i).unwrap().steps).contains(&STEP_MIGRATE));
    i.run_migrations = true;
    let s = deploy_steps(&i).unwrap().steps;
    let pos = |id: &str| s.iter().position(|x| x.id == id).unwrap();
    assert!(pos(STEP_MIGRATE) < pos(STEP_CONFIG_CACHE));
    for t in [AppType::Php, AppType::Static] {
        let mut i = input(t);
        i.run_migrations = true;
        assert!(deploy_steps(&i).is_err());
    }
}

#[test]
fn ssh_url_needs_a_passed_access_check_and_gets_the_pinned_ssh_command() {
    let mut i = input(AppType::Static);
    i.url = GitUrl::parse("git@github.com:o/r.git").unwrap();
    assert_eq!(deploy_steps(&i).unwrap_err(), DeployError::AccessNotChecked);
    i.access = Some(AccessResult::Ok);
    let s = deploy_steps(&i).unwrap().steps;
    assert_eq!(s[0].id, crate::git_access::STEP_DEPLOY_KEY_CHECK);
    let clone = s.iter().find(|x| x.id == STEP_CLONE).unwrap();
    let line = clone.command.to_command_line();
    assert!(line.contains("GIT_SSH_COMMAND=/usr/bin/ssh -i /srv/ossiframe/apps/blog/"));
    assert!(line.contains("StrictHostKeyChecking=yes") && !line.contains("accept-new"));
    assert!(line.contains("'GIT_TERMINAL_PROMPT=0' '/usr/bin/git' 'clone' '--depth' '1' '--branch' 'main' '--single-branch' '--' 'git@github.com:o/r.git'"));
    // HTTPS: no ssh command, no gate.
    let https = deploy_steps(&input(AppType::Static)).unwrap().steps;
    assert_eq!(https[0].id, STEP_CLONE);
    let line = https[0].command.to_command_line();
    assert!(!line.contains("GIT_SSH_COMMAND") && line.contains("GIT_TERMINAL_PROMPT=0"));
}

#[tokio::test]
async fn php_composer_runs_only_when_composer_json_exists() {
    let i = input(AppType::Php);
    let s = redeploy_steps(&i).unwrap().steps;
    let c = s.iter().find(|x| x.id == STEP_COMPOSER).unwrap();
    assert_eq!(c.mode, StepMode::Gate);
    let p = plan(s);
    for (has, expect_run) in [(true, true), (false, false)] {
        let f = Fake::new(move |c| {
            if c.program() == TEST {
                (i32::from(!has), String::new())
            } else {
                (0, String::new())
            }
        });
        let r = run_gate(&f, &CancelToken::new(), &p, STEP_COMPOSER).await;
        assert!(r.is_ok());
        assert_eq!(f.saw("'/usr/bin/composer' 'install'"), expect_run);
        assert!(f.saw("composer.json"));
    }
    // A failing composer fails the gate.
    let f = Fake::new(|c| (i32::from(c.program() == ENV), String::new()));
    assert!(
        run_gate(&f, &CancelToken::new(), &p, STEP_COMPOSER)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn app_key_is_generated_only_while_empty_and_never_read_back() {
    let p = plan(redeploy_steps(&input(AppType::Laravel)).unwrap().steps);
    for (grep_code, generated) in [(0, false), (1, true)] {
        let f = Fake::new(move |c| {
            if c.program() == GREP {
                (grep_code, String::new())
            } else {
                (0, String::new())
            }
        });
        run_gate(&f, &CancelToken::new(), &p, STEP_APP_KEY)
            .await
            .unwrap();
        assert_eq!(f.saw("key:generate"), generated);
        assert!(!f.saw("'cat'") && f.saw("'-q'"));
    }
    let f = Fake::new(|c| (i32::from(c.program() == GREP) * 2, String::new()));
    assert!(
        run_gate(&f, &CancelToken::new(), &p, STEP_APP_KEY)
            .await
            .is_err()
    );
}

/// Runs the non-gate steps in order and stops at the first failure, like the
/// engine.
async fn run_plan(f: &Fake, steps: &[PlanStep]) -> bool {
    for s in steps.iter().filter(|s| s.mode != StepMode::Gate) {
        let r = f.run(&s.command, &CancelToken::new()).await.unwrap();
        if !r.exit.success() {
            return false;
        }
    }
    true
}

#[tokio::test]
async fn a_failing_boot_check_runs_no_switch_step_and_removes_nothing() {
    let steps = deploy_steps(&input(AppType::Static)).unwrap().steps;
    let f = Fake::new(|c| (i32::from(c.program() == TEST), String::new()));
    assert!(!run_plan(&f, &steps).await);
    assert!(f.saw("'/usr/bin/git' 'clone'"));
    assert!(!f.saw("'/usr/bin/ln'") && !f.saw("'/usr/bin/mv'") && !f.saw("'/usr/bin/rm'"));
    assert!(!f.saw("current"));
    // Passing boot check: the switch runs, still no rm.
    let f = Fake::new(|_| (0, String::new()));
    assert!(run_plan(&f, &steps).await);
    assert!(f.saw("current.ossiframe-tmp") && f.saw("'-Tf'"));
}

#[tokio::test]
async fn a_failing_build_step_stops_before_boot_check_and_switch() {
    let steps = deploy_steps(&input(AppType::Laravel)).unwrap().steps;
    let f = Fake::new(|c| {
        (
            i32::from(c.args().iter().any(|a| a == "install")),
            String::new(),
        )
    });
    assert!(!run_plan(&f, &steps).await);
    assert!(!f.saw("--version") && !f.saw("current"));
}

#[test]
fn retention_keeps_five_plus_current_and_never_removes_current() {
    let all: Vec<ReleaseId> = (1..=9)
        .map(|d| rid(&format!("2025010{d}T000000Z")))
        .collect();
    // Current is the oldest.
    let v = retention_victims(&all, Some(&all[0]), Some(&all[8]));
    assert!(!v.contains(&all[0]) && !v.contains(&all[8]));
    assert_eq!(v, vec![all[3].clone(), all[2].clone(), all[1].clone()]);
    let kept = all.len() - v.len();
    assert_eq!(kept, KEEP_RELEASES + 1);
    // Current among the newest: exactly five kept.
    let v = retention_victims(&all, Some(&all[8]), Some(&all[8]));
    assert_eq!(all.len() - v.len(), KEEP_RELEASES);
    // Rolled back to an old release: current and the newest five survive.
    for cur in &all {
        let v = retention_victims(&all, Some(cur), None);
        assert!(!v.contains(cur));
        assert!(all.len() - v.len() <= KEEP_RELEASES + 1);
    }
    // Five or fewer: nothing to remove.
    assert!(retention_victims(&all[..5], None, None).is_empty());
}

#[tokio::test]
async fn cleanup_gate_deletes_only_valid_old_release_ids_through_rm() {
    let p = plan(redeploy_steps(&input(AppType::Static)).unwrap().steps);
    let listing = "20240101T000000Z\n20240102T000000Z\n20240103T000000Z\n20240104T000000Z\n\
                   20240105T000000Z\n20240106T000000Z\n20240107T000000Z\nnotes\n..\n";
    let cur = format!("{}/releases/20240101T000000Z", root().as_str());
    let f = Fake::new(move |c| match c.program() {
        LS => (0, listing.to_owned()),
        READLINK => (0, format!("{cur}\n")),
        _ => (0, String::new()),
    });
    run_gate(&f, &CancelToken::new(), &p, STEP_CLEANUP)
        .await
        .unwrap();
    let rms: Vec<String> = f
        .log()
        .into_iter()
        .filter(|l| l.contains("'/usr/bin/rm'"))
        .collect();
    assert_eq!(rms.len(), 1, "{rms:?}");
    assert!(rms[0].contains("'-rf' '--' '/srv/ossiframe/apps/blog/releases/20240102T000000Z'"));
    assert!(rms[0].starts_with("'/usr/bin/sudo' '-n' '-u' 'ossi-blog' '--'"));
    // An unrecognised `current` deletes nothing.
    let f = Fake::new(|c| match c.program() {
        LS => (0, "20240101T000000Z\n".to_owned()),
        READLINK => (0, "/etc\n".to_owned()),
        _ => (0, String::new()),
    });
    run_gate(&f, &CancelToken::new(), &p, STEP_CLEANUP)
        .await
        .unwrap();
    assert!(!f.saw("'/usr/bin/rm'"));
}

fn obs(current: CurrentLink, releases: &[&str]) -> AppObservation {
    AppObservation {
        current,
        releases: releases.iter().map(|r| rid(r)).collect(),
        site: FileState::Managed,
        pool: Some(FileState::Managed),
        nginx_active: Some(true),
        fpm_active: Some(true),
    }
}

#[test]
fn rollback_is_refused_for_unknown_failed_missing_or_current_releases() {
    let (a, b, c) = ("20250101T000000Z", "20250102T000000Z", "20250103T000000Z");
    let rec = |id: &str, s| ReleaseRecord {
        id: rid(id),
        status: s,
    };
    let records = vec![
        rec(a, ReleaseStatus::Deployed),
        rec(b, ReleaseStatus::Failed),
        rec(c, ReleaseStatus::Deployed),
    ];
    let slug = app_slug("blog").unwrap();
    let o = obs(CurrentLink::Release(rid(c)), &[a, b, c]);
    let plan_for =
        |t: &str, o: &AppObservation| rollback_plan(AppType::Laravel, &slug, &rid(t), &records, o);
    assert_eq!(
        plan_for("20250109T000000Z", &o).unwrap_err(),
        RollbackError::UnknownRelease
    );
    assert_eq!(plan_for(b, &o).unwrap_err(), RollbackError::FailedRelease);
    assert_eq!(plan_for(c, &o).unwrap_err(), RollbackError::AlreadyCurrent);
    let gone = obs(CurrentLink::Release(rid(c)), &[c]);
    assert_eq!(
        plan_for(a, &gone).unwrap_err(),
        RollbackError::MissingOnDisk
    );
    // Valid: only verification and the switch.
    let s = plan_for(a, &o).unwrap().steps;
    assert_eq!(
        ids(&s),
        [
            STEP_USER_VERIFY,
            STEP_RELEASE_EXISTS,
            STEP_SWITCH_LINK,
            STEP_SWITCH,
            STEP_FPM_RELOAD
        ]
    );
    assert!(
        s[2].command
            .to_command_line()
            .contains(&format!("releases/{a}"))
    );
    // Static: no FPM reload.
    let s = rollback_plan(AppType::Static, &slug, &rid(a), &records, &o)
        .unwrap()
        .steps;
    assert!(!ids(&s).contains(&STEP_FPM_RELOAD));
}

#[test]
fn redeploy_has_no_user_pool_site_database_or_env_step() {
    for t in AppType::ALL {
        let mut i = input(t);
        i.run_migrations = t == AppType::Laravel;
        let out = redeploy_steps(&i).unwrap();
        for s in &out.steps {
            for banned in [
                "user-add",
                "db-apply",
                "env-write",
                "pool-apply",
                "site-apply",
                "base-dirs",
                "app-root-owner",
            ] {
                assert!(!s.id.starts_with(banned), "{}", s.id);
            }
            let line = s.command.to_command_line();
            for word in [
                "DB_PASSWORD",
                "useradd",
                "mysql",
                "ALTER USER",
                "/etc/nginx",
                "pool.d",
            ] {
                assert!(!line.contains(word), "{line}");
            }
            // The only writes to .env paths are links to it.
            if line.contains("shared/.env") {
                assert!(s.id == STEP_LINK_ENV || s.id == STEP_APP_KEY, "{}", s.id);
            }
        }
        assert_eq!(out.steps[0].id, STEP_USER_VERIFY);
    }
}

#[test]
fn shared_links_for_laravel_and_php_only() {
    let l = deploy_steps(&input(AppType::Laravel)).unwrap().steps;
    let line = |id: &str| {
        l.iter()
            .find(|x| x.id == id)
            .unwrap()
            .command
            .to_command_line()
    };
    assert!(line(STEP_STORAGE_REMOVE).contains(
        "'/usr/bin/rm' '-rf' '--' '/srv/ossiframe/apps/blog/releases/20250101T000000Z/storage'"
    ));
    assert!(line(STEP_STORAGE_LINK).contains("'/srv/ossiframe/apps/blog/shared/storage' '/srv/ossiframe/apps/blog/releases/20250101T000000Z/storage'"));
    assert!(line(STEP_LINK_ENV).contains("'../../shared/.env'"));
    assert!(line(STEP_STORAGE_DIRS).contains("shared/storage/framework/views"));
    let mut php = input(AppType::Php);
    php.has_env = false;
    assert!(!ids(&deploy_steps(&php).unwrap().steps).contains(&STEP_LINK_ENV));
    assert!(ids(&deploy_steps(&input(AppType::Php)).unwrap().steps).contains(&STEP_LINK_ENV));
    let st = deploy_steps(&input(AppType::Static)).unwrap().steps;
    assert!(!ids(&st).contains(&STEP_LINK_ENV) && !ids(&st).contains(&STEP_COMPOSER));
}

#[test]
fn commit_sha_is_parsed_strictly() {
    let sha = "0123456789abcdef0123456789abcdef01234567";
    assert_eq!(parse_commit_sha(&format!("{sha}\n")).as_deref(), Some(sha));
    for bad in [
        "",
        "abc",
        "ZZ23456789abcdef0123456789abcdef01234567",
        &format!("{sha}x"),
    ] {
        assert!(parse_commit_sha(bad).is_none(), "{bad}");
    }
}

#[tokio::test]
async fn observation_reads_link_releases_markers_and_services() {
    let slug = app_slug("blog").unwrap();
    let r = root();
    let cur = format!("{}/releases/{RID}\n", r.as_str());
    let site_marker = nginx_marker(&slug);
    let f = Fake::new(move |c| match c.program() {
        READLINK => (0, cur.clone()),
        LS => (0, format!("{RID}\njunk\n")),
        HEAD if c.args().last().unwrap().contains("sites-available") => {
            (0, format!("{site_marker}\nserver {{\n"))
        }
        HEAD => (0, "; edited by hand\n".to_owned()),
        SYSTEMCTL => (3, "active\ninactive\n".to_owned()),
        _ => (0, String::new()),
    });
    let o = observe_app_state(&f, &CancelToken::new(), &slug, AppType::Laravel)
        .await
        .unwrap();
    assert_eq!(o.current, CurrentLink::Release(rid(RID)));
    assert_eq!(o.releases, vec![rid(RID)]);
    assert_eq!(
        (o.site, o.pool),
        (FileState::Managed, Some(FileState::Altered))
    );
    assert_eq!((o.fpm_active, o.nginx_active), (Some(true), Some(false)));
    // Reads only: no mutating program ran.
    for l in f.log() {
        assert!(
            !l.contains("'/usr/bin/rm'")
                && !l.contains("'/usr/bin/ln'")
                && !l.contains("'/usr/bin/mv'")
        );
    }
    assert_eq!(parse_current_link(&r, "/etc/passwd"), CurrentLink::Other);
}
