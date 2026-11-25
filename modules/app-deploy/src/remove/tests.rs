use super::*;
use async_trait::async_trait;
use ossiframe_core::{
    ExecError, ExecResult, ExitStatus, PlanTarget, Preconditions, Resources, SudoMode,
};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

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
    fn count(&self, needle: &str) -> usize {
        self.log
            .lock()
            .unwrap()
            .iter()
            .filter(|l| l.contains(needle))
            .count()
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

fn input(t: AppType, drop_database: bool, delete_files: bool) -> RemoveInput {
    RemoveInput {
        app_type: t,
        slug: app_slug("blog").unwrap(),
        drop_database,
        delete_files,
    }
}

fn ids(i: &RemoveInput) -> Vec<String> {
    remove_steps(i)
        .unwrap()
        .steps
        .into_iter()
        .map(|s| s.id)
        .collect()
}

fn plan_of(i: &RemoveInput) -> Plan {
    Plan {
        target: PlanTarget {
            server_id: "s".into(),
            name: "n".into(),
            host: "h".into(),
            port: 22,
            user: "u".into(),
            host_key_sha256: "k".into(),
            app_id: Some("a".into()),
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
        steps: remove_steps(i).unwrap().steps,
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

#[test]
fn the_four_option_combinations_give_the_expected_steps() {
    let base = ["app-user-verify", "site-remove", "pool-remove"];
    let drop = ["db-drop"];
    let del = [
        "app-processes-check",
        "app-root-real-dir",
        "user-delete",
        "files-delete",
    ];
    let want = |parts: &[&[&str]]| -> Vec<String> {
        parts
            .iter()
            .flat_map(|p| p.iter())
            .map(|s| (*s).to_owned())
            .collect()
    };
    assert_eq!(ids(&input(AppType::Laravel, false, false)), want(&[&base]));
    assert_eq!(
        ids(&input(AppType::Laravel, true, false)),
        want(&[&base, &drop])
    );
    assert_eq!(
        ids(&input(AppType::Laravel, false, true)),
        want(&[&base, &del])
    );
    assert_eq!(
        ids(&input(AppType::Laravel, true, true)),
        want(&[&base, &drop, &del])
    );
    // Default: neither a drop nor a userdel, and the app is marked removed.
    let d = remove_steps(&input(AppType::Laravel, false, false)).unwrap();
    assert!(d.mark_removed);
    assert!(!d.steps.iter().any(|s| s.command.program() == USERDEL
        || s.command.program() == MYSQL
        || s.command.args().iter().any(|a| a == "-rf")));
    // A static site has no pool and no database.
    assert_eq!(
        ids(&input(AppType::Static, false, false)),
        ["app-user-verify", "site-remove"]
    );
    assert!(remove_steps(&input(AppType::Static, true, false)).is_err());
}

#[test]
fn delete_steps_are_ordered_and_use_fixed_arguments() {
    let steps = remove_steps(&input(AppType::Laravel, true, true))
        .unwrap()
        .steps;
    let pos = |id: &str| steps.iter().position(|s| s.id == id).unwrap();
    assert!(pos("pool-remove") < pos("app-processes-check"));
    assert!(pos("app-root-real-dir") < pos("user-delete"));
    assert!(pos("user-delete") < pos("files-delete"));
    let line = |id: &str| steps[pos(id)].command.to_command_line();
    assert_eq!(
        line("user-delete"),
        "'/usr/bin/sudo' '-n' '--' '/usr/sbin/userdel' 'ossi-blog'"
    );
    assert_eq!(
        line("files-delete"),
        "'/usr/bin/sudo' '-n' '--' '/usr/bin/rm' '-rf' '--one-file-system' '--' '/srv/ossiframe/apps/blog'"
    );
    // The SQL travels on stdin only and names only this app's objects.
    let drop = &steps[pos("db-drop")];
    assert!(drop.requires_stdin && drop.command.has_stdin());
    assert!(!drop.command.to_command_line().contains("DROP"));
    assert_eq!(
        render_drop_sql(&app_slug("blog").unwrap()),
        "DROP DATABASE IF EXISTS `ossi_blog`;\nDROP USER IF EXISTS 'ossi_blog'@'localhost';\n"
    );
}

#[test]
fn process_and_root_checks_have_a_pass_and_a_fail_input() {
    assert!(parse_processes(1, "0\n").is_ok());
    assert!(parse_processes(0, "3\n").is_err());
    assert!(parse_processes(2, "").is_err());
    assert!(parse_root_stat("directory:ossi-blog\n", "ossi-blog").is_ok());
    // A symlink, or a directory of another user, is refused.
    assert!(parse_root_stat("symbolic link:ossi-blog\n", "ossi-blog").is_err());
    assert!(parse_root_stat("directory:root\n", "ossi-blog").is_err());
}

const SITE: &str = "/etc/nginx/sites-available/ossiframe-blog";
const LINK: &str = "/etc/nginx/sites-enabled/ossiframe-blog";

fn site_fake(first_line: &'static str, nginx_t_fails_once: bool) -> Fake {
    let tests = AtomicUsize::new(0);
    Fake::new(move |c| {
        let line = c.to_command_line();
        match c.program() {
            TEST if line.contains("'-L'") && line.contains(LINK) => (0, String::new()),
            TEST if line.contains("'-L'") => (1, String::new()),
            TEST => (0, String::new()),
            HEAD => (0, format!("{first_line}\nserver {{\n")),
            READLINK => (0, format!("{SITE}\n")),
            NGINX if nginx_t_fails_once && tests.fetch_add(1, Ordering::SeqCst) == 0 => {
                (1, String::new())
            }
            _ => (0, String::new()),
        }
    })
}

#[tokio::test]
async fn the_site_is_removed_only_with_this_apps_marker() {
    let slug = app_slug("blog").unwrap();
    let p = plan_of(&input(AppType::Laravel, false, false));
    let c = CancelToken::new();

    let f = site_fake(
        "# Managed by Ossiframe. Do not edit manually. app=blog",
        false,
    );
    assert_eq!(
        nginx_marker(&slug),
        "# Managed by Ossiframe. Do not edit manually. app=blog"
    );
    run_gate(&f, &c, &p, STEP_SITE_REMOVE).await.unwrap();
    assert_eq!(
        f.count("'/usr/bin/rm' '--' '/etc/nginx/sites-enabled/ossiframe-blog'"),
        1
    );
    assert_eq!(
        f.count("'/usr/bin/rm' '--' '/etc/nginx/sites-available/ossiframe-blog'"),
        1
    );
    assert_eq!(f.count("'reload' 'nginx'"), 1);
    // The test ran before the reload.
    let log = f.log.lock().unwrap().clone();
    let t = log
        .iter()
        .position(|l| l.contains("/usr/sbin/nginx"))
        .unwrap();
    let r = log
        .iter()
        .position(|l| l.contains("'reload' 'nginx'"))
        .unwrap();
    assert!(t < r);

    // Another app's marker, or a hand-edited file: nothing is removed.
    let f = site_fake(
        "# Managed by Ossiframe. Do not edit manually. app=other",
        false,
    );
    let err = run_gate(&f, &c, &p, STEP_SITE_REMOVE).await.unwrap_err();
    assert!(
        err.contains("without this application's Ossiframe marker"),
        "{err}"
    );
    assert_eq!(f.count("'/usr/bin/rm'"), 0);
    assert_eq!(f.count("reload"), 0);
}

#[tokio::test]
async fn a_failed_nginx_test_restores_the_site_and_the_link_without_reload() {
    let p = plan_of(&input(AppType::Laravel, false, false));
    let f = site_fake(
        "# Managed by Ossiframe. Do not edit manually. app=blog",
        true,
    );
    let err = run_gate(&f, &CancelToken::new(), &p, STEP_SITE_REMOVE)
        .await
        .unwrap_err();
    assert!(
        err.contains("restored") && err.contains("Nothing was reloaded"),
        "{err}"
    );
    assert_eq!(
        f.count(
            "'/usr/bin/mv' '-T' '--' '/etc/nginx/sites-available/ossiframe-blog.ossiframe-bak'"
        ),
        1
    );
    assert_eq!(f.count("'/usr/bin/ln' '-s' '-T' '--' '/etc/nginx/sites-available/ossiframe-blog' '/etc/nginx/sites-enabled/ossiframe-blog'"), 1);
    assert_eq!(f.count("reload"), 0);
    // Tested again after the restore.
    assert_eq!(f.count("/usr/sbin/nginx"), 2);
}
