//! Tests for the `app_*` commands. No server is contacted: the server side is
//! a scripted fake and the store is in memory.
use super::*;
use crate::apps::dto::CreateParams;
use crate::apps::plans::{Kind, assemble, build_create, plan_kind};
use crate::apps::provisioner::AppProvisioner;
use async_trait::async_trait;
use ossiframe_core::executor::{ExecError, ExecResult, Executor, ExitStatus};
use ossiframe_core::{CommandSpec, Hostname, Port, ServerName, SshUser};
use ossiframe_desktop::provisioning::engine::{NoOperationEvents, Timing};
use ossiframe_desktop::store::{AuthConfig, ServerRecord};
use ossiframe_mod_app_deploy::env::EnvFile;
use ossiframe_mod_app_deploy::git_access::HostKeys;

const SERVER: &str = "6f1d3f0e-0000-4000-8000-000000000001";
const KEY: &str = "SHA256:abc";
const PASSWORD: &str = "Zq8Xk2Lm9Pv4Rt7Yb1Nc6Hd3Jf5Wg0Sa";

fn identity() -> ServerIdentity {
    ServerIdentity {
        host: "example.com".into(),
        port: 22,
        user: "deploy".into(),
        host_key_sha256: KEY.into(),
    }
}

/// Answers every probe from a fixed table. `reply` gets the program and the
/// rendered command line and returns (exit code, stdout).
struct Fake {
    reply: fn(&str, &str) -> (i32, String),
}

#[async_trait]
impl Executor for Fake {
    async fn run(&self, spec: &CommandSpec, _: &CancelToken) -> Result<ExecResult, ExecError> {
        let (code, stdout) = (self.reply)(spec.program(), &spec.to_command_line());
        Ok(ExecResult {
            exit: ExitStatus::Code(code),
            stdout,
            stderr: String::new(),
            truncated: false,
        })
    }
}

impl ServerLink for Fake {
    fn server_id(&self) -> &str {
        SERVER
    }
    fn identity(&self) -> Option<ServerIdentity> {
        Some(identity())
    }
    fn take_session_cancel(&self) -> bool {
        false
    }
}

struct Access(Arc<Fake>);

impl ServerAccess for Access {
    fn profile(&self, _: &str) -> Result<Profile, IpcError> {
        Ok(Profile {
            identity: identity(),
            name: "web".into(),
            connected: true,
        })
    }
    fn link(&self, _: &str) -> Arc<dyn ServerLink> {
        self.0.clone()
    }
}

fn server_record() -> ServerRecord {
    ServerRecord {
        id: SERVER.into(),
        name: ServerName::parse("web").unwrap(),
        host: Hostname::parse("example.com").unwrap(),
        port: Port::new(22).unwrap(),
        username: SshUser::parse("deploy").unwrap(),
        auth: AuthConfig::Password,
        secret_ref: None,
        created_at: timestamp(),
        updated_at: timestamp(),
    }
}

fn apps(reply: fn(&str, &str) -> (i32, String)) -> (Apps, Arc<Store>) {
    let store = Arc::new(Store::open_in_memory().unwrap());
    store.insert(&server_record()).unwrap();
    let engine = Engine::new(
        store.clone(),
        Arc::new(AppProvisioner::new(store.clone())),
        Arc::new(NoOperationEvents),
        Timing::default(),
    );
    (
        Apps::new(
            Arc::new(Access(Arc::new(Fake { reply }))),
            store.clone(),
            engine,
        ),
        store,
    )
}

fn params() -> CreateParams {
    CreateParams {
        server_id: SERVER.into(),
        app_type: "laravel".into(),
        slug: "blog".into(),
        domain: "blog.example.com".into(),
        repo_url: "https://github.com/o/r".into(),
        repo_branch: "main".into(),
        entry_file: None,
        with_database: false,
        disable_default_site: false,
        trusted_git_host_fingerprints: None,
    }
}

fn nothing(_: &str, _: &str) -> (i32, String) {
    (0, String::new())
}

/// A healthy server where `blog.example.com` is already served by another
/// Ossiframe site.
fn domain_taken(program: &str, line: &str) -> (i32, String) {
    match program {
        "/usr/bin/dpkg-query" => (
            0,
            "git\tii \nnginx\tii \nphp8.3-fpm\tii \ncomposer\tii \nmysql-server\tii \n".into(),
        ),
        "/usr/bin/df" => (
            0,
            "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/x 100000000 1 50000000 1% /srv\n".into(),
        ),
        "/usr/bin/ls" => (0, "default\nossiframe-other\n".into()),
        "/usr/bin/grep" => (0, "/etc/nginx/sites-enabled/ossiframe-other\n".into()),
        "/usr/bin/getent" => (2, String::new()),
        _ if line.contains("curl") => (0, String::new()),
        _ => (0, String::new()),
    }
}

fn plan_reason(e: &IpcError) -> Option<PlanReason> {
    match e {
        IpcError::Plan { reason, .. } => Some(*reason),
        _ => None,
    }
}

fn no_plan_stored(a: &Apps, store: &Store) -> bool {
    lock(&a.intents).is_empty()
        && store.list_apps(SERVER).unwrap().is_empty()
        && a.plans
            .take_approved(
                SERVER,
                &uuid::Uuid::new_v4().to_string(),
                &"0".repeat(64),
                &ApprovalContext {
                    identity: identity(),
                    connected: true,
                    busy: false,
                },
            )
            .unwrap_err()
            == ApprovalError::UnknownPlan
}

// ---- tests ---------------------------------------------------------------------------

#[test]
fn command_parameters_are_typed_ids_names_and_flags_only() {
    let src = include_str!("../commands.rs");
    let allowed = [
        "state",
        "server_id",
        "app_id",
        "app_type",
        "slug",
        "domain",
        "repo_url",
        "repo_branch",
        "entry_file",
        "with_database",
        "disable_default_site",
        "trusted_git_host_fingerprints",
        "run_migrations",
        "release_id",
        "drop_database",
        "delete_files",
        "plan_id",
        "digest",
        "changes",
    ];
    let forbidden = [
        "program",
        "argv",
        "args",
        "command",
        "cmd",
        "path",
        "privilege",
        "plan",
        "steps",
        "stdin",
        "content",
        "script",
        "sql",
        "password",
    ];
    let mut seen = 0;
    for name in APP_COMMAND_NAMES {
        let at = src
            .find(&format!("pub async fn {name}("))
            .unwrap_or_else(|| panic!("missing fn {name}"));
        let sig = &src[at..];
        let sig = &sig[sig.find('(').unwrap() + 1..sig.find(") ->").unwrap()];
        for param in sig.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let pname = param.split(':').next().unwrap().trim();
            assert!(
                allowed.contains(&pname),
                "{name}: unexpected parameter {pname}"
            );
            assert!(!forbidden.contains(&pname), "{name}: forbidden {pname}");
            seen += 1;
        }
    }
    assert!(seen > 20);
    assert_eq!(APP_COMMAND_NAMES.len(), 13);
    // An environment change carries a key, a value and a delete flag only.
    let bad: Result<EnvChangeDto, _> =
        serde_json::from_str(r#"{"key":"A","value":"b","command":"rm -rf /"}"#);
    assert!(bad.is_err());
}

#[test]
fn invalid_input_is_refused_before_anything_else() {
    let mut p = params();
    p.slug = "a".repeat(28);
    assert!(matches!(p.parse(), Err(IpcError::Validation { field, .. }) if field == "slug"));
    let mut p = params();
    p.domain = "*.example.com".into();
    assert!(p.parse().is_err());
    let mut p = params();
    p.repo_url = "file:///etc".into();
    assert!(p.parse().is_err());
    let mut p = params();
    p.repo_branch = "-x".into();
    assert!(p.parse().is_err());
    let mut p = params();
    p.entry_file = Some("../../etc/passwd".into());
    assert!(p.parse().is_err());
    let mut p = params();
    p.app_type = "node".into();
    assert!(p.parse().is_err());
    let mut p = params();
    p.trusted_git_host_fingerprints = Some(vec!["not a fingerprint".into()]);
    assert!(p.parse().is_err());
    assert!(params().parse().is_ok());
    let env = |key: &str, value: Option<&str>, delete: bool| EnvChangeDto {
        key: key.into(),
        value: value.map(Into::into),
        delete,
    };
    assert!(parse_env_changes(&[env("lower", Some("x"), false)]).is_err());
    assert!(parse_env_changes(&[env("A", Some("a\nb"), false)]).is_err());
    assert!(parse_env_changes(&[env("A", Some("x"), true)]).is_err());
    assert!(parse_env_changes(&[env("A", None, false)]).is_err());
    assert!(parse_env_changes(&[env("A", Some("1"), false), env("A", None, true)]).is_err());
    assert!(parse_env_changes(&[env("A", Some("1"), false), env("B", None, true)]).is_ok());
}

#[test]
fn no_response_carries_a_database_password_or_a_secret_env_value() {
    // The environment view: secrets have no value.
    let text = "DB_PASSWORD=hunter2hunter2\nAPP_KEY=base64:SECRETKEYVALUE\nSTRIPE_SECRET=sk_live_abc\nDATABASE_URL=mysql://u:urlpass@h/db\nAPP_NAME=Blog\n";
    let entries = EnvFile::new(text.to_owned()).unwrap().entries();
    let json = serde_json::to_string(&entries).unwrap();
    for secret in ["hunter2hunter2", "SECRETKEYVALUE", "sk_live_abc", "urlpass"] {
        assert!(!json.contains(secret), "{secret} leaked");
    }
    assert!(json.contains("Blog"));

    // A create plan built with a known database password: the password is in
    // memory as stdin and in no serialized response.
    let mut p = params();
    p.repo_url = "https://github.com/o/r".into();
    let i = p.parse().unwrap();
    let password = DbPassword::parse(PASSWORD).unwrap();
    let built = build_create(
        &i,
        SERVER,
        "11111111-1111-4111-8111-111111111111",
        &HostKeys::None,
        &password,
        ReleaseId::now(),
    )
    .unwrap();
    assert!(built.pieces.steps.iter().any(|s| {
        s.command
            .stdin()
            .is_some_and(|b| String::from_utf8_lossy(b).contains(PASSWORD))
    }));
    let plan = assemble(&built.app.id, built.pieces, built.watched, true);
    let dto = PlanDto {
        plan_id: "p".into(),
        digest: "d".into(),
        expires_in_secs: 1,
        plan,
        warnings: Vec::new(),
    };
    let app_json = serde_json::to_string(&AppDto::new(&built.app, None)).unwrap();
    let list_json = serde_json::to_string(&AppSummaryDto::new(&built.app)).unwrap();
    for json in [
        serde_json::to_string(&dto).unwrap(),
        app_json.clone(),
        list_json,
    ] {
        assert!(!json.contains(PASSWORD), "password leaked");
    }
    assert!(!app_json.to_lowercase().contains("password"));
    assert!(app_json.contains("\"hasDatabase\":true"));
}

#[tokio::test]
async fn a_failed_preflight_stores_no_plan() {
    // Nothing is installed and nothing can be read: blockers, no plan.
    let (a, store) = apps(nothing);
    let err = a.create_plan(&params()).await.unwrap_err();
    assert_eq!(plan_reason(&err), Some(PlanReason::Blocked), "{err:?}");
    assert!(no_plan_stored(&a, &store));

    // A domain used by another Ossiframe site has its own reason.
    let (a, store) = apps(domain_taken);
    let err = a.create_plan(&params()).await.unwrap_err();
    assert_eq!(plan_reason(&err), Some(PlanReason::DomainInUse), "{err:?}");
    assert!(no_plan_stored(&a, &store));
}

#[tokio::test]
async fn slug_and_domain_collisions_are_refused_from_the_local_records() {
    let (a, store) = apps(nothing);
    let i = params().parse().unwrap();
    let built = build_create(
        &i,
        SERVER,
        "11111111-1111-4111-8111-111111111111",
        &HostKeys::None,
        &DbPassword::parse(PASSWORD).unwrap(),
        ReleaseId::now(),
    )
    .unwrap();
    store.insert_app(&built.app).unwrap();
    let err = a.create_plan(&params()).await.unwrap_err();
    assert_eq!(plan_reason(&err), Some(PlanReason::SlugInUse));
    let mut other = params();
    other.slug = "shop".into();
    let err = a.create_plan(&other).await.unwrap_err();
    assert_eq!(plan_reason(&err), Some(PlanReason::DomainInUse));
    // Unknown and malformed app ids.
    assert_eq!(
        plan_reason(&a.get("nope").unwrap_err()),
        Some(PlanReason::AppNotFound)
    );
    assert_eq!(
        plan_reason(&a.get(&uuid::Uuid::new_v4().to_string()).unwrap_err()),
        Some(PlanReason::AppNotFound)
    );
    assert_eq!(a.get(&built.app.id).unwrap().slug, "blog");
}

#[tokio::test]
async fn an_unknown_plan_cannot_be_approved() {
    let (a, store) = apps(nothing);
    let err = a
        .approve_plan(SERVER, &uuid::Uuid::new_v4().to_string(), &"a".repeat(64))
        .await
        .unwrap_err();
    assert_eq!(plan_reason(&err), Some(PlanReason::UnknownPlan));
    assert!(store.list_apps(SERVER).unwrap().is_empty());
}

#[test]
fn built_plans_are_classified_by_their_steps() {
    let i = params().parse().unwrap();
    let built = build_create(
        &i,
        SERVER,
        "11111111-1111-4111-8111-111111111111",
        &HostKeys::None,
        &DbPassword::parse(PASSWORD).unwrap(),
        ReleaseId::now(),
    )
    .unwrap();
    assert!(built.release.is_some(), "an HTTPS create deploys at once");
    let plan = assemble("a", built.pieces, built.watched, true);
    assert_eq!(plan_kind(&plan), Some(Kind::Create { deploy: true }));
    assert!(
        plan.steps
            .iter()
            .any(|s| s.requires_stdin && s.command.has_stdin()),
        "stdin steps keep their stdin in memory"
    );
    let removal = remove_steps(&RemoveInput {
        app_type: AppType::Laravel,
        slug: ossiframe_mod_app_deploy::identity::app_slug("blog").unwrap(),
        drop_database: false,
        delete_files: false,
    })
    .unwrap();
    let plan = assemble("a", removal.into(), BTreeMap::new(), false);
    assert_eq!(plan_kind(&plan), Some(Kind::Remove));
    // The default removal keeps the user, the files and the database.
    assert!(
        plan.steps
            .iter()
            .all(|s| !s.id.contains("delete") && !s.id.contains("drop"))
    );
}

#[test]
fn an_ssh_create_waits_for_the_deploy_key_and_pins_the_host_keys() {
    let mut p = params();
    p.repo_url = "git@github.com:o/r.git".into();
    let i = p.parse().unwrap();
    let keys = plan_host_keys(&i.url, &[], &[]).unwrap();
    let built = build_create(
        &i,
        SERVER,
        "11111111-1111-4111-8111-111111111111",
        &keys,
        &DbPassword::parse(PASSWORD).unwrap(),
        ReleaseId::now(),
    )
    .unwrap();
    assert!(built.release.is_none());
    let plan = assemble("a", built.pieces, built.watched, true);
    assert_eq!(plan_kind(&plan), Some(Kind::Create { deploy: false }));
    let ids: Vec<&str> = plan.steps.iter().map(|s| s.id.as_str()).collect();
    assert!(ids.contains(&"git-known-hosts-install"));
    assert!(ids.contains(&"deploy-key-generate"));
    assert!(
        !ids.contains(&"release-clone"),
        "no clone before the key is added"
    );
}
