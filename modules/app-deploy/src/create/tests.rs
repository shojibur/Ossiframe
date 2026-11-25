use super::*;
use async_trait::async_trait;
use ossiframe_core::{PlanTarget, Preconditions, Privilege, Resources, SudoMode};
use std::collections::BTreeMap;
use std::sync::Mutex;

const PW: &str = "Abcdefghijklmnopqrstuvwxyz0123456789ABCD";

fn pw() -> DbPassword {
    DbPassword::parse(PW).unwrap()
}

fn input(t: AppType, domain: &str, db: bool, default_off: bool) -> CreateInput {
    CreateInput {
        app_type: t,
        slug: app_slug("blog").unwrap(),
        domain: DomainName::parse(domain).unwrap(),
        entry_file: EntryFile::parse(t.info().entry_file).unwrap(),
        with_database: db,
        disable_default_site: default_off,
    }
}

fn steps_of(i: &CreateInput) -> Vec<PlanStep> {
    create_steps(i, &pw()).unwrap().steps
}

fn ids(i: &CreateInput) -> Vec<String> {
    steps_of(i).into_iter().map(|s| s.id).collect()
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

// ---- rendering and plan shape -------------------------------------------------

#[test]
fn marker_is_the_first_line_of_every_managed_file() {
    for t in AppType::ALL {
        let i = input(t, "blog.example.com", true, false);
        let root = AppRoot::new(&i.slug).unwrap();
        let site = render_site(&i, &root);
        assert_eq!(site.lines().next().unwrap(), nginx_marker(&i.slug));
        if t.info().uses_php {
            let user = app_user(&i.slug).unwrap();
            let pool = render_pool(&i.slug, &user, &root);
            assert_eq!(pool.lines().next().unwrap(), fpm_marker(&i.slug));
        }
    }
}

#[test]
fn hostile_values_are_rejected_and_the_site_has_only_known_directives() {
    for d in [
        "a.com;\nroot /;",
        "a.com\n}\nserver {",
        "a.com{",
        "*.a.com",
        "a.com $uri",
    ] {
        assert!(DomainName::parse(d).is_err(), "{d:?}");
    }
    for s in ["x;rm", "a\nb", "a b", "../x"] {
        assert!(app_slug(s).is_err(), "{s:?}");
    }
    for e in ["a;b.php", "a\nb", "a b", "../x.php", "a{b}"] {
        assert!(EntryFile::parse(e).is_err(), "{e:?}");
    }
    let allowed = [
        "# Managed",
        "server {",
        "listen ",
        "server_name ",
        "root ",
        "index ",
        "disable_symlinks ",
        "add_header X-Ossiframe-App ",
        "location ",
        "try_files ",
        "include fastcgi.conf;",
        "fastcgi_pass unix:/run/php/ossiframe-blog.sock;",
        "}",
    ];
    for t in AppType::ALL {
        let mut i = input(t, "my-blog.example.com", false, false);
        i.entry_file = EntryFile::parse(if t == AppType::Php {
            "web/app.v2/main.php"
        } else {
            t.info().entry_file
        })
        .unwrap();
        let site = render_site(&i, &AppRoot::new(&i.slug).unwrap());
        for line in site.lines() {
            let l = line.trim();
            assert!(allowed.iter().any(|a| l.starts_with(a)), "{l:?}");
        }
        assert_eq!(site.matches("server_name").count(), 1);
        assert_eq!(site.matches("root ").count(), 1);
    }
}

#[test]
fn site_denies_dotfiles_sets_the_app_header_and_serves_the_right_root() {
    let l = input(AppType::Laravel, "blog.example.com", true, false);
    let site = render_site(&l, &AppRoot::new(&l.slug).unwrap());
    assert!(site.contains("location ~ /\\.(?!well-known) { deny all; }"));
    assert!(site.contains("add_header X-Ossiframe-App blog always;"));
    assert!(site.contains("root /srv/ossiframe/apps/blog/current/public;"));
    assert!(site.contains("try_files $uri $uri/ /index.php?$query_string;"));
    assert!(site.contains("fastcgi_pass unix:/run/php/ossiframe-blog.sock;"));
    let mut s = input(AppType::Static, "blog.example.com", false, false);
    s.entry_file = EntryFile::parse("index.html").unwrap();
    let site = render_site(&s, &AppRoot::new(&s.slug).unwrap());
    assert!(site.contains("root /srv/ossiframe/apps/blog/current;"));
    assert!(!site.contains("fastcgi"));
    assert!(site.contains("deny all"));
}

#[test]
fn pool_runs_as_the_app_user_and_confines_it() {
    let i = input(AppType::Laravel, "blog.example.com", true, false);
    let pool = render_pool(
        &i.slug,
        &app_user(&i.slug).unwrap(),
        &AppRoot::new(&i.slug).unwrap(),
    );
    for want in [
        "[ossi-blog]",
        "user = ossi-blog",
        "group = ossi-blog",
        "listen = /run/php/ossiframe-blog.sock",
        "listen.owner = www-data",
        "listen.mode = 0660",
        "pm.max_children = 5",
        "clear_env = yes",
        "php_admin_value[open_basedir] = /srv/ossiframe/apps/blog:/tmp",
    ] {
        assert!(pool.lines().any(|l| l == want), "{want}");
    }
}

#[test]
fn step_lists_per_type() {
    let base = [
        "base-dirs",
        "user-add",
        "app-user-verify",
        "app-root-check",
        "app-root-owner",
        "app-root-mode",
        "releases-dir",
        "shared-dir",
        "private-dirs",
    ];
    let with = |extra: &[&str]| -> Vec<String> {
        base.iter().chain(extra).map(|s| (*s).to_owned()).collect()
    };
    let l = input(AppType::Laravel, "blog.example.com", false, false);
    assert_eq!(
        ids(&l),
        with(&["db-apply", "env-write", "pool-apply", "site-apply"])
    );
    let php = input(AppType::Php, "blog.example.com", false, false);
    assert_eq!(ids(&php), with(&["pool-apply", "site-apply"]));
    let php_db = input(AppType::Php, "blog.example.com", true, true);
    assert_eq!(
        ids(&php_db),
        with(&[
            "db-apply",
            "env-write",
            "pool-apply",
            "site-apply-disable-default"
        ])
    );
    let st = input(AppType::Static, "blog.example.com", false, false);
    assert_eq!(ids(&st), with(&["site-apply"]));
    let mut bad = st.clone();
    bad.with_database = true;
    assert!(create_steps(&bad, &pw()).is_err());
    let mut bad = l.clone();
    bad.entry_file = EntryFile::parse("index.php").unwrap();
    assert!(create_steps(&bad, &pw()).is_err());
}

#[test]
fn shared_is_group_readable_by_www_data_but_env_and_ssh_stay_private() {
    let steps = steps_of(&input(AppType::Laravel, "blog.example.com", false, false));
    let line = |id: &str| {
        steps
            .iter()
            .find(|s| s.id == id)
            .unwrap()
            .command
            .to_command_line()
    };
    assert!(line(STEP_SHARED_DIR).contains("'-m' '2750' '--' '/srv/ossiframe/apps/blog/shared'"));
    assert!(line(STEP_PRIVATE_DIRS).contains("'-m' '0700' '--' '/srv/ossiframe/apps/blog/.ssh'"));
    // The env file keeps mode 0600.
    assert!(line(STEP_ENV_WRITE).contains("'-m' '0600'"));
}

#[test]
fn run_as_steps_follow_the_identity_gate_and_root_never_writes_inside_the_app() {
    for t in AppType::ALL {
        let i = input(t, "blog.example.com", t != AppType::Static, false);
        let steps = steps_of(&i);
        let verify = steps.iter().position(|s| s.id == STEP_USER_VERIFY).unwrap();
        let add = steps.iter().position(|s| s.id == STEP_USER_ADD).unwrap();
        assert!(add < verify);
        assert_eq!(steps[verify].mode, StepMode::Gate);
        let user = app_user(&i.slug).unwrap();
        for (n, s) in steps.iter().enumerate() {
            match s.command.privilege() {
                Privilege::RunAs(u) => {
                    assert!(n > verify, "{}", s.id);
                    assert_eq!(u, user);
                }
                Privilege::Root => {
                    // Only the creation steps name the app root itself.
                    let inside = s
                        .command
                        .args()
                        .iter()
                        .any(|a| a.starts_with("/srv/ossiframe/apps/blog/"));
                    assert!(!inside, "{}", s.id);
                }
                Privilege::User => {}
            }
        }
        let useradd = &steps[add].command;
        assert_eq!(useradd.program(), "/usr/sbin/useradd");
        assert!(!useradd.args().iter().any(|a| a == "--system"));
        assert_eq!(useradd.args().last().unwrap(), "ossi-blog");
    }
}

// ---- password and SQL -----------------------------------------------------------

#[test]
fn generated_password_is_long_alphanumeric_and_unique() {
    let a = DbPassword::generate().unwrap();
    let b = DbPassword::generate().unwrap();
    assert_eq!(a.expose().len(), 40);
    assert!(a.expose().bytes().all(|c| c.is_ascii_alphanumeric()));
    assert_ne!(a.expose(), b.expose());
    assert!(!format!("{a:?}").contains(a.expose()));
    assert!(DbPassword::parse("short").is_err());
    assert!(DbPassword::parse(&format!("{PW}'")).is_err());
}

#[test]
fn sql_names_only_the_app_database_and_never_alters_a_user() {
    for s in ["blog", "my-app-2"] {
        let slug = app_slug(s).unwrap();
        let sql = render_sql(&slug, &pw());
        let n = format!("ossi_{s}");
        assert!(!sql.to_uppercase().contains("ALTER"));
        assert!(!sql.contains("*.*"));
        let lines: Vec<&str> = sql.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], format!("CREATE DATABASE IF NOT EXISTS `{n}`;"));
        assert_eq!(
            lines[1],
            format!("CREATE USER IF NOT EXISTS '{n}'@'localhost' IDENTIFIED BY '{PW}';")
        );
        assert_eq!(
            lines[2],
            format!("GRANT ALL PRIVILEGES ON `{n}`.* TO '{n}'@'localhost';")
        );
    }
}

#[test]
fn the_password_is_only_in_stdin_never_in_program_args_debug_or_json() {
    for t in [AppType::Laravel, AppType::Php] {
        let i = input(t, "blog.example.com", true, false);
        let steps = steps_of(&i);
        let mut with_pw = Vec::new();
        for s in &steps {
            assert!(!s.command.program().contains(PW));
            assert!(s.command.args().iter().all(|a| !a.contains(PW)));
            assert!(!s.command.to_command_line().contains(PW));
            assert!(!format!("{s:?}").contains(PW));
            assert!(!s.title.contains(PW));
            if s.command
                .stdin()
                .is_some_and(|b| String::from_utf8_lossy(b).contains(PW))
            {
                with_pw.push(s.id.clone());
            }
        }
        assert_eq!(with_pw, ["db-apply", "env-write"]);
        assert!(!serde_json::to_string(&steps).unwrap().contains(PW));
        let all = create_steps(&i, &pw()).unwrap();
        assert!(!format!("{:?}", all.files).contains(PW));
    }
}

#[test]
fn env_file_has_the_database_settings() {
    let l = input(AppType::Laravel, "blog.example.com", true, false);
    let env = render_env(&l, &pw());
    for want in [
        "APP_ENV=production",
        "APP_DEBUG=false",
        "APP_URL=http://blog.example.com",
        "APP_KEY=",
        "DB_CONNECTION=mysql",
        "DB_HOST=127.0.0.1",
        "DB_PORT=3306",
        "DB_DATABASE=ossi_blog",
        "DB_USERNAME=ossi_blog",
        &format!("DB_PASSWORD={PW}"),
    ] {
        assert!(env.lines().any(|x| x == want), "{want}");
    }
    let php = render_env(&input(AppType::Php, "blog.example.com", true, false), &pw());
    assert!(!php.contains("APP_"));
    let s = steps_of(&l);
    let e = s.iter().find(|s| s.id == STEP_ENV_WRITE).unwrap();
    assert_eq!(e.command.args()[..2], ["-m", "0600"]);
    assert!(matches!(e.command.privilege(), Privilege::RunAs(_)));
}

#[test]
fn db_decision_table() {
    use DbDecision::*;
    use EnvState::*;
    assert_eq!(decide_db(false, NoFile), Create);
    assert_eq!(decide_db(true, HasPassword), Keep);
    for (u, e) in [
        (true, NoFile),
        (true, NoPassword),
        (false, HasPassword),
        (false, NoPassword),
    ] {
        assert!(matches!(decide_db(u, e), Block(_)), "{u} {e:?}");
    }
}

// ---- scripted executor ------------------------------------------------------------

#[derive(Default)]
struct State {
    files: BTreeMap<String, Vec<u8>>,
    links: BTreeMap<String, String>,
    log: Vec<String>,
    sql: Vec<String>,
    db_user: bool,
}

struct Fake(Mutex<State>);

fn res(code: i32, out: &str) -> Result<ExecResult, ExecError> {
    Ok(ExecResult {
        exit: ExitStatus::Code(code),
        stdout: out.to_owned(),
        stderr: if code == 0 {
            String::new()
        } else {
            "test failed".to_owned()
        },
        truncated: false,
    })
}

#[async_trait]
impl Executor for Fake {
    async fn run(&self, s: &CommandSpec, _: &CancelToken) -> Result<ExecResult, ExecError> {
        let mut st = self.0.lock().unwrap();
        st.log.push(s.to_command_line());
        let a: Vec<String> = s.args().to_vec();
        let last = a.last().cloned().unwrap_or_default();
        match s.program() {
            "/usr/bin/test" => {
                let p = &a[1];
                let yes = match a[0].as_str() {
                    "-L" => st.links.contains_key(p),
                    "-e" => st.files.contains_key(p) || st.links.contains_key(p),
                    _ => st.files.contains_key(p),
                };
                res(i32::from(!yes), "")
            }
            "/usr/bin/head" => match st.files.get(&last) {
                Some(b) => res(0, String::from_utf8_lossy(b).lines().next().unwrap_or("")),
                None => res(1, ""),
            },
            "/usr/bin/readlink" => match st.links.get(&last) {
                Some(t) => res(0, t),
                None => res(1, ""),
            },
            "/usr/bin/cp" => {
                let data = st.files.get(&a[a.len() - 2]).cloned().unwrap();
                st.files.insert(last, data);
                res(0, "")
            }
            "/usr/bin/install" => {
                st.files.insert(last, s.stdin().unwrap().to_vec());
                res(0, "")
            }
            "/usr/bin/mv" => {
                let data = st.files.remove(&a[a.len() - 2]).unwrap();
                st.files.insert(last, data);
                res(0, "")
            }
            "/usr/bin/rm" => {
                let force = a[0] == "-f";
                let hit = st.files.remove(&last).is_some() | st.links.remove(&last).is_some();
                res(i32::from(!hit && !force), "")
            }
            "/usr/bin/ln" => {
                if st.links.contains_key(&last) || st.files.contains_key(&last) {
                    return res(1, "");
                }
                let target = a[a.len() - 2].clone();
                st.links.insert(last, target);
                res(0, "")
            }
            "/usr/sbin/nginx" | "/usr/sbin/php-fpm8.3" => {
                let bad = st
                    .files
                    .values()
                    .any(|b| String::from_utf8_lossy(b).contains("fail-me"));
                res(i32::from(bad), "")
            }
            "/usr/bin/grep" => {
                let has = st
                    .files
                    .get(&last)
                    .is_some_and(|b| String::from_utf8_lossy(b).contains("DB_PASSWORD=A"));
                res(i32::from(!has), "")
            }
            "/usr/bin/mysql" => match s.stdin() {
                Some(b) => {
                    st.sql.push(String::from_utf8_lossy(b).into_owned());
                    st.db_user = true;
                    res(0, "")
                }
                None => res(0, if st.db_user { "1\n" } else { "0\n" }),
            },
            _ => res(0, ""),
        }
    }
}

const SITE: &str = "/etc/nginx/sites-available/ossiframe-blog";
const LINK: &str = "/etc/nginx/sites-enabled/ossiframe-blog";
const OLD: &str = "# Managed by Ossiframe. Do not edit manually. app=blog\nold\n";

fn site_plan(domain: &str, default_off: bool) -> (Plan, String) {
    let i = input(AppType::Static, domain, false, default_off);
    let steps = steps_of(&i);
    let id = steps.last().unwrap().id.clone();
    (plan(steps), id)
}

fn fake(files: &[(&str, &str)], links: &[(&str, &str)]) -> Fake {
    let mut st = State::default();
    for (p, c) in files {
        st.files.insert((*p).to_owned(), c.as_bytes().to_vec());
    }
    for (l, t) in links {
        st.links.insert((*l).to_owned(), (*t).to_owned());
    }
    Fake(Mutex::new(st))
}

fn count(f: &Fake, needle: &str) -> usize {
    f.0.lock()
        .unwrap()
        .log
        .iter()
        .filter(|l| l.contains(needle))
        .count()
}

async fn gate(f: &Fake, p: &Plan, id: &str) -> Result<String, String> {
    run_gate(f, &CancelToken::new(), p, id).await
}

#[tokio::test]
async fn a_failed_test_restores_the_old_site_and_does_not_reload() {
    let (p, id) = site_plan("fail-me.example.com", false);
    let f = fake(&[(SITE, OLD)], &[(LINK, SITE)]);
    let err = gate(&f, &p, &id).await.unwrap_err();
    assert!(err.contains("restored"), "{err}");
    assert!(err.contains("Nothing was reloaded"));
    let st = f.0.lock().unwrap();
    assert_eq!(st.files[SITE], OLD.as_bytes());
    assert!(
        !st.files
            .keys()
            .any(|k| k.ends_with(".ossiframe-bak") || k.ends_with(".ossiframe-tmp"))
    );
    drop(st);
    assert_eq!(count(&f, "reload"), 0);
    // The test ran again after the restore.
    assert_eq!(count(&f, "/usr/sbin/nginx"), 2);
}

#[tokio::test]
async fn a_failed_test_with_no_old_file_removes_the_new_file_and_link() {
    let (p, id) = site_plan("fail-me.example.com", false);
    let f = fake(&[], &[]);
    assert!(gate(&f, &p, &id).await.is_err());
    let st = f.0.lock().unwrap();
    assert!(st.files.is_empty() && st.links.is_empty());
    drop(st);
    assert_eq!(count(&f, "reload"), 0);
}

#[tokio::test]
async fn a_passing_test_reloads_and_leaves_the_new_site_enabled() {
    let (p, id) = site_plan("blog.example.com", false);
    let f = fake(&[(SITE, OLD)], &[(LINK, SITE)]);
    gate(&f, &p, &id).await.unwrap();
    {
        let st = f.0.lock().unwrap();
        assert!(String::from_utf8_lossy(&st.files[SITE]).contains("server_name blog.example.com;"));
        assert_eq!(st.links[LINK], SITE);
        assert!(!st.files.keys().any(|k| k.ends_with(".ossiframe-bak")));
    }
    assert_eq!(count(&f, "'reload' 'nginx'"), 1);
    // A fresh create links the site.
    let f = fake(&[], &[]);
    gate(&f, &p, &id).await.unwrap();
    assert_eq!(f.0.lock().unwrap().links[LINK], SITE);
}

#[tokio::test]
async fn an_unmarked_or_foreign_file_is_never_overwritten() {
    let (p, id) = site_plan("blog.example.com", false);
    for existing in [
        "server { }\n",
        "# Managed by Ossiframe. Do not edit manually. app=other\n",
    ] {
        let f = fake(&[(SITE, existing)], &[]);
        assert!(gate(&f, &p, &id).await.is_err());
        assert_eq!(f.0.lock().unwrap().files[SITE], existing.as_bytes());
        assert_eq!(count(&f, "install"), 0);
    }
    // A link pointing elsewhere is not replaced either.
    let f = fake(&[], &[(LINK, "/etc/nginx/sites-available/other")]);
    assert!(gate(&f, &p, &id).await.is_err());
    assert_eq!(count(&f, "install"), 0);
}

#[tokio::test]
async fn the_default_site_link_is_removed_only_when_asked_and_restored_on_failure() {
    const DEF: &str = "/etc/nginx/sites-enabled/default";
    const DEF_AV: &str = "/etc/nginx/sites-available/default";
    let (p, id) = site_plan("blog.example.com", true);
    assert_eq!(id, STEP_SITE_APPLY_DISABLE_DEFAULT);
    let f = fake(&[(DEF_AV, "default")], &[(DEF, DEF_AV)]);
    gate(&f, &p, &id).await.unwrap();
    {
        let st = f.0.lock().unwrap();
        assert!(!st.links.contains_key(DEF));
        assert!(st.files.contains_key(DEF_AV), "the available file is kept");
    }
    let (p, id) = site_plan("fail-me.example.com", true);
    let f = fake(&[(DEF_AV, "default")], &[(DEF, DEF_AV)]);
    assert!(gate(&f, &p, &id).await.is_err());
    assert_eq!(f.0.lock().unwrap().links[DEF], DEF_AV);
    // A default link to something else is left alone and nothing is written.
    let f = fake(&[], &[(DEF, "/etc/nginx/sites-available/custom")]);
    assert!(gate(&f, &p, &id).await.is_err());
    assert_eq!(count(&f, "install"), 0);
    // Without the option the default link is never looked at.
    let (p, id) = site_plan("blog.example.com", false);
    let f = fake(&[(DEF_AV, "default")], &[(DEF, DEF_AV)]);
    gate(&f, &p, &id).await.unwrap();
    assert!(f.0.lock().unwrap().links.contains_key(DEF));
}

#[tokio::test]
async fn a_gate_without_its_stdin_runs_nothing() {
    let (p, id) = site_plan("blog.example.com", false);
    // As after a reload from storage: the command is rebuilt without stdin.
    let json = serde_json::to_string(&p).unwrap();
    let p2: Plan = serde_json::from_str(&json).unwrap();
    let f = fake(&[], &[]);
    let e = gate(&f, &p2, &id).await.unwrap_err();
    assert!(e.contains("re-plan required"));
    assert!(f.0.lock().unwrap().log.is_empty());
}

#[tokio::test]
async fn database_gate_creates_keeps_or_blocks_without_leaking_the_password() {
    let i = input(AppType::Laravel, "blog.example.com", true, false);
    let p = plan(steps_of(&i));
    let env = "/srv/ossiframe/apps/blog/shared/.env";

    // Neither exists: create, then the .env is written.
    let f = fake(&[], &[]);
    gate(&f, &p, STEP_DB_APPLY).await.unwrap();
    gate(&f, &p, STEP_ENV_WRITE).await.unwrap();
    {
        let st = f.0.lock().unwrap();
        assert_eq!(st.sql.len(), 1);
        assert!(String::from_utf8_lossy(&st.files[env]).contains(&format!("DB_PASSWORD={PW}")));
        assert!(st.log.iter().all(|l| !l.contains(PW)));
    }
    // Both exist: nothing runs again and the .env is kept.
    gate(&f, &p, STEP_DB_APPLY).await.unwrap();
    gate(&f, &p, STEP_ENV_WRITE).await.unwrap();
    assert_eq!(f.0.lock().unwrap().sql.len(), 1);

    // User exists, no .env: block, no SQL.
    let f = fake(&[], &[]);
    f.0.lock().unwrap().db_user = true;
    let e = gate(&f, &p, STEP_DB_APPLY).await.unwrap_err();
    assert!(e.contains("inspect"));
    assert!(f.0.lock().unwrap().sql.is_empty());
    // .env with a password but no user: block.
    let f = fake(&[(env, "DB_PASSWORD=Aold\n")], &[]);
    assert!(gate(&f, &p, STEP_DB_APPLY).await.is_err());
    assert!(f.0.lock().unwrap().sql.is_empty());
    // The .env step never overwrites an existing file.
    let f = fake(&[(env, "APP_ENV=x\n")], &[]);
    assert!(gate(&f, &p, STEP_ENV_WRITE).await.is_err());
    assert_eq!(f.0.lock().unwrap().files[env], b"APP_ENV=x\n");
}

#[test]
fn final_path_of_refuses_anything_but_validated_install_steps() {
    let t = Duration::from_secs(1);
    let tmp = |p: &str| format!("{p}{}", crate::paths::TMP_SUFFIX);
    let root = |p: &str| {
        CommandSpec::privileged("/usr/bin/install", &["-T", "--", "/dev/stdin", &tmp(p)], t)
            .unwrap()
    };
    assert!(write::final_path_of(&root(SITE)).is_ok());
    for bad in ["/etc/passwd", LINK, "/srv/ossiframe/apps/blog/.env"] {
        assert!(write::final_path_of(&root(bad)).is_err(), "{bad}");
    }
    let other = CommandSpec::privileged("/usr/bin/tee", &[&tmp(SITE)], t).unwrap();
    assert!(write::final_path_of(&other).is_err());
    let user = CommandSpec::new("/usr/bin/install", &[&tmp(SITE)], t).unwrap();
    assert!(write::final_path_of(&user).is_err());
}
