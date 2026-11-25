//! Engine tests with a scripted fake connection. No server is contacted.
use super::engine::*;
use super::plans::{ApprovalContext, ApprovedPlan, PlanStore, ServerIdentity};
use super::reconcile::{self, Decision, EvidenceSet, StepFacts, UnitObs, UnitShow, decide};
use crate::remote_lock::{Evidence, StaleRefusal};
use crate::store::tests::record;
use crate::store::{STEP_OUTPUT_CAP, Store};
use async_trait::async_trait;
use ossiframe_core::command::CommandSpec;
use ossiframe_core::executor::{CancelToken, ExecError, ExecResult, Executor, ExitStatus};
use ossiframe_core::module::OperationError;
use ossiframe_core::provision::*;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const SERVER: &str = "6f1d3f0e-0000-4000-8000-000000000001";
const KEY: &str = "k1";

fn lk<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn out(code: i32, stdout: &str) -> Result<ExecResult, ExecError> {
    Ok(ExecResult {
        exit: ExitStatus::Code(code),
        stdout: stdout.into(),
        stderr: String::new(),
        truncated: false,
    })
}

fn show(load: &str, active: &str, sub: &str, result: &str, exit: i32, pid: u32) -> String {
    format!(
        "LoadState={load}\nActiveState={active}\nSubState={sub}\nResult={result}\nExecMainStatus={exit}\nMainPID={pid}\n"
    )
}
fn s_running() -> String {
    show("loaded", "active", "running", "success", 0, 99)
}
fn s_ok() -> String {
    show("loaded", "active", "exited", "success", 0, 0)
}
fn s_failed() -> String {
    show("loaded", "failed", "failed", "exit-code", 100, 0)
}
fn s_missing() -> String {
    show("not-found", "inactive", "dead", "success", 0, 0)
}

#[derive(Clone)]
enum Scripted {
    Out(String),
    Err(ExecError),
    Disconnect,
    Then(Hook, String),
}

#[derive(Default)]
struct Probes {
    pgrep_busy: bool,
    fuser_busy: bool,
    units_out: String,
    pkg_out: String,
}

type Hook = Arc<dyn Fn(&Fake) + Send + Sync>;
type StepSnapshot = Vec<(String, bool)>;

struct Fake {
    ident: Mutex<Option<ServerIdentity>>,
    dead: AtomicBool,
    log: Mutex<Vec<String>>,
    lock: Mutex<Option<String>>,
    lock_unavailable: AtomicBool,
    on_lock: Mutex<Option<Hook>>,
    show: Mutex<HashMap<String, VecDeque<Scripted>>>,
    launches: Mutex<u32>,
    probes: Mutex<Probes>,
    journal: Mutex<String>,
    store: Arc<Store>,
    cleanup_seen: Mutex<Vec<(String, StepSnapshot)>>,
}

fn identity(key: &str) -> ServerIdentity {
    ServerIdentity {
        host: "example.com".into(),
        port: 22,
        user: "root".into(),
        host_key_sha256: key.into(),
    }
}

impl Fake {
    fn new(store: Arc<Store>) -> Arc<Self> {
        Arc::new(Self {
            ident: Mutex::new(Some(identity(KEY))),
            dead: AtomicBool::new(false),
            log: Mutex::default(),
            lock: Mutex::default(),
            lock_unavailable: AtomicBool::new(false),
            on_lock: Mutex::default(),
            show: Mutex::default(),
            launches: Mutex::new(0),
            probes: Mutex::new(Probes {
                units_out: String::new(),
                pkg_out: "nginx\tii \t1.24\n".into(),
                ..Probes::default()
            }),
            journal: Mutex::new("journal text\n".into()),
            store,
            cleanup_seen: Mutex::default(),
        })
    }

    fn script(&self, idx: u32, items: Vec<Scripted>) {
        lk(&self.show).insert(idx.to_string(), items.into());
    }
    fn lines(&self) -> Vec<String> {
        lk(&self.log).clone()
    }
    fn any(&self, needle: &str) -> bool {
        self.lines().iter().any(|l| l.contains(needle))
    }
    fn lock_stopped(&self) -> bool {
        self.any("'stop' 'ossiframe-provision-lock.service'")
    }
    fn reconnect(&self) {
        self.dead.store(false, Ordering::SeqCst);
        *lk(&self.ident) = Some(identity(KEY));
    }
    fn disconnect(&self) {
        self.dead.store(true, Ordering::SeqCst);
        *lk(&self.ident) = None;
    }
    fn op_id(&self) -> String {
        self.store.list_operations(None, 1).unwrap()[0].id.clone()
    }
}

#[async_trait]
impl Executor for Fake {
    async fn run(&self, spec: &CommandSpec, _: &CancelToken) -> Result<ExecResult, ExecError> {
        lk(&self.log).push(spec.to_command_line());
        if self.dead.load(Ordering::SeqCst) {
            return Err(ExecError::ConnectionLost);
        }
        if spec.program().ends_with("drop-link") {
            // The connection drops while this command is in flight.
            self.disconnect();
            return Err(ExecError::ConnectionLost);
        }
        let a = spec.args();
        let last = a.last().map(String::as_str).unwrap_or("");
        let is_lock = a.iter().any(|x| x.contains("ossiframe-provision-lock"));
        let prog = spec.program();
        if prog.ends_with("systemd-run") {
            if is_lock {
                let hook = lk(&self.on_lock).clone();
                if self.lock_unavailable.load(Ordering::SeqCst) || lk(&self.lock).is_some() {
                    return out(1, "");
                }
                let d = a
                    .iter()
                    .find_map(|x| x.strip_prefix("--description="))
                    .unwrap();
                *lk(&self.lock) = Some(d.to_owned());
                if let Some(h) = hook {
                    h(self);
                }
                return out(0, "");
            }
            let mut n = lk(&self.launches);
            *n += 1;
            return out(0, "");
        }
        if prog.ends_with("systemctl") {
            return match a[0].as_str() {
                "show" if is_lock => match &*lk(&self.lock) {
                    Some(d) => out(
                        0,
                        &format!(
                            "Description={d}\nActiveState=active\nSubState=running\nLoadState=loaded\n"
                        ),
                    ),
                    None => out(
                        0,
                        "Description=x\nActiveState=inactive\nSubState=dead\nLoadState=not-found\n",
                    ),
                },
                "show" => {
                    let idx = last
                        .trim_end_matches(".service")
                        .rsplit('-')
                        .next()
                        .unwrap()
                        .to_owned();
                    let next = {
                        let mut m = lk(&self.show);
                        match m.get_mut(&idx) {
                            Some(q) if q.len() > 1 => q.pop_front(),
                            Some(q) => q.front().cloned(),
                            None => None,
                        }
                    };
                    match next {
                        None => out(0, &s_ok()),
                        Some(Scripted::Out(s)) => out(0, &s),
                        Some(Scripted::Err(e)) => Err(e),
                        Some(Scripted::Disconnect) => {
                            self.disconnect();
                            Err(ExecError::ConnectionLost)
                        }
                        Some(Scripted::Then(f, s)) => {
                            f(self);
                            out(0, &s)
                        }
                    }
                }
                "stop" | "reset-failed" if is_lock => {
                    *lk(&self.lock) = None;
                    out(0, "")
                }
                "stop" | "reset-failed" => {
                    let id = self.op_id();
                    let seen = self
                        .store
                        .get_steps(&id)
                        .unwrap()
                        .into_iter()
                        .map(|s| (s.status, s.finished_at.is_some()))
                        .collect();
                    lk(&self.cleanup_seen).push((spec.to_command_line(), seen));
                    out(0, "")
                }
                "list-units" => out(0, &lk(&self.probes).units_out.clone()),
                "is-active" => out(0, "active\n"),
                _ => out(0, ""),
            };
        }
        if prog.ends_with("journalctl") {
            return out(0, &lk(&self.journal).clone());
        }
        if prog.ends_with("pgrep") {
            return if lk(&self.probes).pgrep_busy {
                out(0, "4242 apt-get\n")
            } else {
                out(1, "")
            };
        }
        if prog.ends_with("fuser") {
            return if lk(&self.probes).fuser_busy {
                out(0, "/var/lib/dpkg/lock: 4242")
            } else {
                out(1, "")
            };
        }
        if prog.ends_with("dpkg-query")
            && spec
                .to_command_line()
                .contains("Status-Abbrev}\\t${Package}")
        {
            // Whole-system dpkg health listing: every package settled.
            return out(0, "ii \tbase-files\n");
        }
        if prog.ends_with("dpkg-query") {
            return out(0, &lk(&self.probes).pkg_out.clone());
        }
        if prog.ends_with("tee")
            && let Some(input) = spec.stdin()
        {
            // A client that quotes its input back, as a database error does.
            lk(&self.log).push(format!("stdin-present:{}", input.len()));
            return out(0, &String::from_utf8_lossy(input));
        }
        out(0, "")
    }
}

impl ServerLink for Fake {
    fn server_id(&self) -> &str {
        SERVER
    }
    fn identity(&self) -> Option<ServerIdentity> {
        lk(&self.ident).clone()
    }
    fn take_session_cancel(&self) -> bool {
        false
    }
}

struct FakeProv {
    recheck: Mutex<Result<(), Vec<Drift>>>,
    gate: Mutex<Result<String, String>>,
    /// When set, the gate runs this command through the executor first.
    gate_cmd: Mutex<Option<CommandSpec>>,
    checks: Mutex<Vec<CheckResult>>,
}

fn check(status: CheckStatus, essential: bool) -> CheckResult {
    CheckResult {
        component: "web".into(),
        id: "c".into(),
        title: "serves".into(),
        essential,
        status,
        detail: "d".into(),
    }
}

#[async_trait]
impl Provisioner for FakeProv {
    fn components(&self) -> Vec<ComponentInfo> {
        vec![]
    }
    async fn preflight(
        &self,
        _: &dyn Executor,
        _: &CancelToken,
    ) -> Result<PreflightReport, OperationError> {
        unreachable!("not used by the engine")
    }
    async fn preflight_for(
        &self,
        _: &dyn Executor,
        _: &CancelToken,
        _: &[String],
        _: &[ossiframe_core::PriorOperation],
    ) -> Result<PreflightReport, OperationError> {
        unreachable!("not used by the engine")
    }
    fn plan(&self, _: &PreflightReport, _: &[String]) -> Result<Plan, PlanError> {
        unreachable!("not used by the engine")
    }
    async fn recheck(&self, _: &dyn Executor, _: &CancelToken, _: &Plan) -> Result<(), Vec<Drift>> {
        lk(&self.recheck).clone()
    }
    async fn gate(
        &self,
        exec: &dyn Executor,
        cancel: &CancelToken,
        _: &Plan,
        _: &str,
    ) -> Result<String, String> {
        let cmd = lk(&self.gate_cmd).clone();
        if let Some(cmd) = cmd {
            exec.run(&cmd, cancel).await.map_err(|e| e.to_string())?;
        }
        lk(&self.gate).clone()
    }
    async fn verify(&self, _: &dyn Executor, _: &CancelToken, _: &Plan) -> Vec<CheckResult> {
        lk(&self.checks).clone()
    }
}

struct Rec(Mutex<Vec<OperationState>>);
impl OperationEvents for Rec {
    fn operation_updated(&self, _: &str, _: &str, s: OperationState) {
        lk(&self.0).push(s);
    }
}

fn step(id: &str, mode: StepMode, deadline_ms: u64) -> PlanStep {
    PlanStep {
        id: id.into(),
        title: id.into(),
        command: CommandSpec::privileged(
            "/usr/bin/apt-get",
            &[if id == "apt-update" {
                "update"
            } else {
                "install"
            }],
            Duration::from_secs(5),
        )
        .unwrap(),
        mode,
        deadline: Duration::from_millis(deadline_ms),
        requires_stdin: false,
    }
}

fn durable(id: &str) -> PlanStep {
    step(id, StepMode::Durable, 5_000)
}

fn plan_with(steps: Vec<PlanStep>) -> Plan {
    Plan {
        target: PlanTarget {
            server_id: SERVER.into(),
            name: "web".into(),
            host: "example.com".into(),
            port: 22,
            user: "root".into(),
            host_key_sha256: KEY.into(),
            app_id: None,
        },
        os: "ubuntu 24.04".into(),
        components: vec!["web".into()],
        already_present: vec![],
        packages: vec![PackageChange {
            name: "nginx".into(),
            installed_version: None,
        }],
        files: vec![],
        services: vec![ServiceEffect {
            unit: "nginx".into(),
            effect: "started".into(),
        }],
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

struct H {
    engine: Arc<Engine>,
    store: Arc<Store>,
    fake: Arc<Fake>,
    prov: Arc<FakeProv>,
    events: Arc<Rec>,
    plan: Plan,
}

fn harness(steps: Vec<PlanStep>) -> H {
    let store = Arc::new(Store::open_in_memory().unwrap());
    store.insert(&record(SERVER)).unwrap();
    let prov = Arc::new(FakeProv {
        recheck: Mutex::new(Ok(())),
        gate: Mutex::new(Ok("gate evidence".into())),
        gate_cmd: Mutex::default(),
        checks: Mutex::new(vec![check(CheckStatus::Pass, true)]),
    });
    let events = Arc::new(Rec(Mutex::default()));
    let engine = Engine::new(
        store.clone(),
        prov.clone(),
        events.clone(),
        Timing {
            poll_interval: Duration::from_millis(5),
            journal_interval: Duration::from_secs(3600),
            max_poll_misses: 3,
        },
    );
    H {
        engine,
        fake: Fake::new(store.clone()),
        store,
        prov,
        events,
        plan: plan_with(steps),
    }
}

fn approve(h: &H) -> ApprovedPlan {
    let ps = PlanStore::default();
    let handle = ps.insert(h.plan.clone());
    ps.take_approved(
        SERVER,
        &handle.id,
        &handle.digest,
        &ApprovalContext {
            identity: identity(KEY),
            connected: true,
            busy: false,
        },
    )
    .unwrap()
}

async fn run_plan(h: &H) -> String {
    let started = h.engine.start(approve(h), h.fake.clone()).await.unwrap();
    started.task.await.unwrap();
    started.operation_id
}

fn state(h: &H, id: &str) -> OperationState {
    h.store.get_operation(id).unwrap().state
}

fn statuses(h: &H, id: &str) -> Vec<String> {
    h.store
        .get_steps(id)
        .unwrap()
        .into_iter()
        .map(|s| s.status)
        .collect()
}

fn stdin_step(with_stdin: bool) -> PlanStep {
    let mut command =
        CommandSpec::privileged("/usr/bin/tee", &["/etc/x"], Duration::from_secs(5)).unwrap();
    if with_stdin {
        command = command.with_stdin(b"s3cret-input".to_vec()).unwrap();
    }
    PlanStep {
        id: "write-secret".into(),
        title: "write-secret".into(),
        command,
        mode: StepMode::Durable,
        deadline: Duration::from_secs(5),
        requires_stdin: true,
    }
}
/// No stored text of the operation may contain the stdin bytes.
fn assert_no_stdin_stored(h: &H, id: &str) {
    let op = format!("{:?}", h.store.get_operation(id).unwrap());
    let steps = format!("{:?}", h.store.get_steps(id).unwrap());
    let events = format!("{:?}", h.store.get_events(id).unwrap());
    let plan = serde_json::to_string(&h.plan).unwrap();
    for t in [op, steps, events, plan, h.fake.lines().join("\n")] {
        assert!(!t.contains("s3cret-input"));
    }
}
#[tokio::test]
async fn a_stdin_step_runs_directly_and_never_stores_its_stdin() {
    let h = harness(vec![stdin_step(true)]);
    let id = run_plan(&h).await;
    assert_eq!(state(&h, &id), OperationState::Succeeded);
    let lines = h.fake.lines();
    let tee: Vec<_> = lines
        .iter()
        .filter(|l| l.contains("'/usr/bin/tee'"))
        .collect();
    assert_eq!(tee.len(), 1, "run once, directly");
    assert!(!tee[0].contains("systemd-run"));
    assert_eq!(*lk(&h.fake.launches), 0);
    assert_no_stdin_stored(&h, &id);
}
#[tokio::test]
async fn a_stdin_step_that_echoes_its_input_has_its_output_withheld() {
    let h = harness(vec![stdin_step(true)]);
    let id = run_plan(&h).await;
    assert_eq!(state(&h, &id), OperationState::Succeeded);
    let steps = h.store.get_steps(&id).unwrap();
    assert!(steps[0].output.as_deref().unwrap().contains("withheld"));
    assert_no_stdin_stored(&h, &id);
}
#[tokio::test]
async fn a_stdin_step_without_its_stdin_is_refused_and_nothing_runs() {
    // As after a reload from the stored plan: stdin is not serialized.
    let h = harness(vec![stdin_step(false)]);
    let id = run_plan(&h).await;
    assert_eq!(state(&h, &id), OperationState::Failed);
    assert!(
        h.store
            .get_operation(&id)
            .unwrap()
            .error
            .unwrap()
            .contains("re-plan required")
    );
    assert!(!h.fake.any("'/usr/bin/tee'"));
    assert_no_stdin_stored(&h, &id);
}
#[tokio::test]
async fn stdin_reaches_the_engine_from_memory_and_a_reloaded_plan_refuses_the_step() {
    let h = harness(vec![stdin_step(true)]);
    // The approved plan comes from the in-memory PlanStore with its stdin.
    let approved = approve(&h);
    assert!(approved.plan().steps[0].command.has_stdin());
    let started = h.engine.start(approved, h.fake.clone()).await.unwrap();
    started.task.await.unwrap();
    let id = started.operation_id;
    assert_eq!(state(&h, &id), OperationState::Succeeded);
    assert!(h.fake.any("stdin-present:12"), "the executor got the stdin");
    // What SQLite holds has no stdin, so a plan read back cannot run the step.
    let stored = h.store.get_operation(&id).unwrap();
    assert!(!stored.plan_json.contains("s3cret-input"));
    let reloaded: Plan = serde_json::from_str(&stored.plan_json).unwrap();
    assert!(reloaded.steps[0].requires_stdin && !reloaded.steps[0].command.has_stdin());
    let ps = PlanStore::default();
    let handle = ps.insert(reloaded);
    let approved = ps
        .take_approved(
            SERVER,
            &handle.id,
            &handle.digest,
            &ApprovalContext {
                identity: identity(KEY),
                connected: true,
                busy: false,
            },
        )
        .unwrap();
    let started = h.engine.start(approved, h.fake.clone()).await.unwrap();
    started.task.await.unwrap();
    assert_eq!(state(&h, &started.operation_id), OperationState::Failed);
    let ran = h
        .fake
        .lines()
        .iter()
        .filter(|l| l.contains("'/usr/bin/tee'"))
        .count();
    assert_eq!(ran, 1, "the reloaded plan ran nothing");
}
// ---- start --------------------------------------------------------------------------

#[tokio::test]
async fn nothing_runs_before_start_and_a_busy_lock_creates_no_operation() {
    let h = harness(vec![durable("apt-update")]);
    assert!(h.fake.lines().is_empty());
    *lk(&h.fake.lock) = Some(
        "ossiframe-lock op=11111111-2222-4333-8444-555555555555 app=dev.ossiframe.deploy desktop=0123456789abcdef0123456789abcdef started=2026-01-02T03:04:05Z"
            .into(),
    );
    let err = match h.engine.start(approve(&h), h.fake.clone()).await {
        Err(e) => e,
        Ok(_) => panic!("must not start"),
    };
    assert!(matches!(&err, StartError::Busy { holder: Some(h) } if h.owner.is_some()));
    // The record was written first, then ended Failed with the reason.
    let ops = h.store.list_operations(None, 5).unwrap();
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].state, OperationState::Failed);
    assert!(
        ops[0]
            .error
            .as_deref()
            .unwrap_or("")
            .contains("nothing ran")
    );
    assert!(!h.store.has_unresolved_operation(SERVER).unwrap());
    assert!(!h.fake.any("--unit=ossiframe-op-"));
    assert!(!h.fake.lock_stopped(), "a foreign lock is never released");
}

#[tokio::test]
async fn the_operation_record_exists_before_the_remote_lock_is_taken() {
    let h = harness(vec![durable("apt-update")]);
    let seen: Arc<Mutex<Vec<OperationState>>> = Arc::default();
    let sink = seen.clone();
    *lk(&h.fake.on_lock) = Some(Arc::new(move |f: &Fake| {
        let ops = f.store.list_operations(None, 5).unwrap();
        lk(&sink).extend(ops.iter().map(|o| o.state));
    }));
    let id = run_plan(&h).await;
    assert_eq!(*lk(&seen), vec![OperationState::Pending]);
    assert_ne!(state(&h, &id), OperationState::Pending);
}

#[tokio::test]
async fn unavailable_lock_is_a_blocker_and_drift_requires_a_new_plan_before_any_lock() {
    let h = harness(vec![durable("apt-update")]);
    h.fake.lock_unavailable.store(true, Ordering::SeqCst);
    let r = h.engine.start(approve(&h), h.fake.clone()).await;
    assert!(matches!(r, Err(StartError::Blocked(_))));
    let ops = h.store.list_operations(None, 5).unwrap();
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].state, OperationState::Failed);

    let h = harness(vec![durable("apt-update")]);
    *lk(&h.prov.recheck) = Err(vec![Drift {
        id: "pkg".into(),
        message: "changed".into(),
    }]);
    let r = h.engine.start(approve(&h), h.fake.clone()).await;
    assert!(matches!(r, Err(StartError::RePlanRequired(d)) if d.len() == 1));
    assert!(
        !h.fake.any("systemd-run"),
        "no lock before the recheck passes"
    );
    assert!(h.store.list_operations(None, 5).unwrap().is_empty());
}

// ---- running ------------------------------------------------------------------------

#[tokio::test]
async fn happy_path_persists_before_cleanup_bounds_output_and_releases_the_lock() {
    let h = harness(vec![
        durable("apt-update"),
        step("candidate-check", StepMode::Gate, 1_000),
        durable("apt-install"),
    ]);
    *lk(&h.fake.journal) = "x".repeat(200_000);
    h.fake
        .script(0, vec![Scripted::Out(s_running()), Scripted::Out(s_ok())]);
    h.fake.script(2, vec![Scripted::Out(s_ok())]);
    let id = run_plan(&h).await;
    assert_eq!(state(&h, &id), OperationState::Succeeded);
    assert_eq!(statuses(&h, &id), ["succeeded", "succeeded", "succeeded"]);
    assert!(
        h.store
            .get_operation(&id)
            .unwrap()
            .verification_json
            .is_some()
    );
    assert!(h.fake.lock_stopped());
    // Unit names carry the full operation uuid and the step index.
    assert!(h.fake.any(&format!("--unit=ossiframe-op-{id}-0")));
    assert!(h.fake.any(&format!("--unit=ossiframe-op-{id}-2")));
    // The terminal result was already stored when each cleanup command ran.
    let seen = lk(&h.fake.cleanup_seen).clone();
    assert_eq!(seen.len(), 2);
    assert!(seen[0].1[0] == ("succeeded".to_owned(), true));
    assert!(seen[1].1[2] == ("succeeded".to_owned(), true));
    let out = h.store.get_steps(&id).unwrap()[0].output.clone().unwrap();
    assert!(out.len() <= STEP_OUTPUT_CAP);
    // Every root command is part of approved-plan execution.
    for l in h
        .fake
        .lines()
        .iter()
        .filter(|l| l.contains("'/usr/bin/sudo'"))
    {
        assert!(
            [
                "systemd-run",
                "journalctl",
                "'stop'",
                "'reset-failed'",
                "fuser"
            ]
            .iter()
            .any(|p| l.contains(p)),
            "{l}"
        );
    }
    let ev = lk(&h.events.0).clone();
    assert_eq!(ev.last(), Some(&OperationState::Succeeded));
}

#[tokio::test]
async fn failed_step_fails_the_operation_skips_the_rest_and_cleans_up_after_persisting() {
    let h = harness(vec![durable("apt-update"), durable("apt-install")]);
    h.fake.script(0, vec![Scripted::Out(s_failed())]);
    let id = run_plan(&h).await;
    assert_eq!(state(&h, &id), OperationState::Failed);
    assert_eq!(statuses(&h, &id), ["failed", "skipped"]);
    assert_eq!(
        *lk(&h.fake.launches),
        1,
        "the install step is never launched"
    );
    let seen = lk(&h.fake.cleanup_seen).clone();
    assert!(seen[0].0.contains("'reset-failed'"));
    assert_eq!(seen[0].1[0], ("failed".to_owned(), true));
    assert!(h.fake.lock_stopped());
}

#[tokio::test]
async fn a_unit_that_may_still_have_a_process_is_not_stopped() {
    let h = harness(vec![durable("apt-install")]);
    h.fake.script(
        0,
        vec![Scripted::Out(show(
            "loaded", "active", "exited", "success", 0, 77,
        ))],
    );
    let id = run_plan(&h).await;
    assert_eq!(state(&h, &id), OperationState::Succeeded);
    assert!(!h.fake.any("'stop' 'ossiframe-op-"));
}

#[tokio::test]
async fn disconnect_during_a_step_is_unknown_outcome_without_relaunch_or_lock_release() {
    let h = harness(vec![durable("apt-update"), durable("apt-install")]);
    h.fake
        .script(0, vec![Scripted::Out(s_running()), Scripted::Disconnect]);
    let id = run_plan(&h).await;
    assert_eq!(state(&h, &id), OperationState::UnknownOutcome);
    assert_eq!(statuses(&h, &id), ["unknown", "pending"]);
    assert_eq!(*lk(&h.fake.launches), 1);
    assert!(!h.fake.lock_stopped(), "the lock is kept at UnknownOutcome");
    assert!(!h.fake.any("'stop'") && !h.fake.any("'reset-failed'"));
}

#[tokio::test]
async fn deadline_expiry_stops_waiting_without_any_stop_or_kill() {
    let h = harness(vec![step("apt-install", StepMode::Durable, 40)]);
    h.fake.script(0, vec![Scripted::Out(s_running())]);
    let id = run_plan(&h).await;
    assert_eq!(state(&h, &id), OperationState::UnknownOutcome);
    let err = h.store.get_operation(&id).unwrap().error.unwrap();
    assert!(err.contains("may still be running"));
    assert!(!h.fake.any("'stop'") && !h.fake.any("kill") && !h.fake.any("reset-failed"));
    assert!(!h.fake.lock_stopped());
}

#[tokio::test]
async fn poll_timeouts_are_retried_three_times_then_unknown_outcome() {
    let h = harness(vec![durable("apt-install")]);
    h.fake.script(0, vec![Scripted::Err(ExecError::Timeout)]);
    let id = run_plan(&h).await;
    assert_eq!(state(&h, &id), OperationState::UnknownOutcome);
    let shows = h
        .fake
        .lines()
        .iter()
        .filter(|l| l.contains("'show'") && l.contains("ossiframe-op-"))
        .count();
    assert_eq!(shows, 3);
    assert_eq!(*lk(&h.fake.launches), 1);

    // Two timeouts followed by a good answer do not lose the outcome.
    let h = harness(vec![durable("apt-install")]);
    h.fake.script(
        0,
        vec![
            Scripted::Err(ExecError::Timeout),
            Scripted::Err(ExecError::Timeout),
            Scripted::Out(s_ok()),
        ],
    );
    let id = run_plan(&h).await;
    assert_eq!(state(&h, &id), OperationState::Succeeded);
}

#[tokio::test]
async fn cancel_is_honoured_only_between_steps() {
    let h = harness(vec![durable("apt-update"), durable("apt-install")]);
    let engine = h.engine.clone();
    h.fake.script(
        0,
        vec![Scripted::Then(
            Arc::new(move |f: &Fake| engine.cancel(&f.op_id()).unwrap()),
            s_ok(),
        )],
    );
    let id = run_plan(&h).await;
    assert_eq!(state(&h, &id), OperationState::Cancelled);
    assert_eq!(statuses(&h, &id), ["succeeded", "skipped"]);
    assert_eq!(*lk(&h.fake.launches), 1, "step 2 is never launched");
    assert!(h.fake.lock_stopped());
    let events = h.store.get_events(&id).unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.detail.as_deref() == Some("cancel requested, waiting for current step"))
    );
}

#[tokio::test]
async fn identity_mismatch_stops_all_further_commands() {
    let h = harness(vec![durable("apt-update"), durable("apt-install")]);
    let before = Arc::new(Mutex::new(0usize));
    let b = before.clone();
    h.fake.script(
        0,
        vec![Scripted::Then(
            Arc::new(move |f: &Fake| {
                *lk(&f.ident) = Some(identity("other-key"));
                *lk(&b) = f.lines().len();
            }),
            s_ok(),
        )],
    );
    let id = run_plan(&h).await;
    assert_eq!(state(&h, &id), OperationState::UnknownOutcome);
    assert_eq!(*lk(&h.fake.launches), 1);
    // The poll that saw the mismatch was the last command sent.
    assert_eq!(h.fake.lines().len(), *lk(&before));
    assert!(!h.fake.lock_stopped());
}

#[tokio::test]
async fn failed_gate_fails_before_the_next_step() {
    let h = harness(vec![
        durable("apt-update"),
        step("candidate-check", StepMode::Gate, 1_000),
        durable("apt-install"),
    ]);
    *lk(&h.prov.gate) = Err("origin not allowed".into());
    let id = run_plan(&h).await;
    assert_eq!(state(&h, &id), OperationState::Failed);
    assert_eq!(statuses(&h, &id), ["succeeded", "failed", "skipped"]);
    assert_eq!(*lk(&h.fake.launches), 1);
}

#[tokio::test]
async fn gate_that_loses_the_connection_ends_in_unknown_outcome() {
    let h = harness(vec![
        step("site-apply", StepMode::Gate, 1_000),
        durable("apt-install"),
    ]);
    // A privileged command inside the gate loses the connection.
    *lk(&h.prov.gate_cmd) = Some(
        CommandSpec::privileged("/usr/bin/drop-link", &["x"], Duration::from_secs(5)).unwrap(),
    );
    *lk(&h.prov.gate) = Err("never reached".into());
    let id = run_plan(&h).await;
    assert_eq!(state(&h, &id), OperationState::UnknownOutcome);
    assert_eq!(statuses(&h, &id), ["unknown", "pending"]);
    // Nothing was retried or launched, and the lock is kept.
    assert_eq!(*lk(&h.fake.launches), 0);
    assert!(!h.fake.lock_stopped());
}

#[tokio::test]
async fn verification_decides_success() {
    // A failed essential check fails the operation, installed packages stay.
    let h = harness(vec![durable("apt-install")]);
    *lk(&h.prov.checks) = vec![
        check(CheckStatus::Pass, true),
        check(CheckStatus::Fail, true),
    ];
    let id = run_plan(&h).await;
    assert_eq!(state(&h, &id), OperationState::Failed);
    assert!(h.fake.lock_stopped());

    // A skipped essential check is not verified.
    let h = harness(vec![durable("apt-install")]);
    *lk(&h.prov.checks) = vec![check(CheckStatus::Skipped, true)];
    assert_eq!(state(&h, &run_plan(&h).await), OperationState::Failed);

    // No evidence at all is not success either.
    let h = harness(vec![]);
    *lk(&h.prov.checks) = vec![];
    assert_eq!(state(&h, &run_plan(&h).await), OperationState::Failed);

    // A zero-step plan goes straight to verification.
    let h = harness(vec![]);
    let id = run_plan(&h).await;
    assert_eq!(state(&h, &id), OperationState::Succeeded);
    assert_eq!(*lk(&h.fake.launches), 0);
}

// ---- reconcile ------------------------------------------------------------------------

async fn lost_install(h: &H) -> String {
    h.fake
        .script(0, vec![Scripted::Out(s_running()), Scripted::Disconnect]);
    let id = run_plan(h).await;
    assert_eq!(state(h, &id), OperationState::UnknownOutcome);
    h.fake.reconnect();
    id
}

fn mutating_marks() -> [&'static str; 6] {
    [
        "systemd-run",
        "'stop'",
        "'reset-failed'",
        "kill",
        "--configure",
        "'install'",
    ]
}

#[tokio::test]
async fn reconcile_with_a_running_unit_stays_unknown_and_changes_nothing() {
    let h = harness(vec![durable("apt-install")]);
    let id = lost_install(&h).await;
    h.fake.script(0, vec![Scripted::Out(s_running())]);
    let n = h.fake.lines().len();
    let r = h.engine.reconcile(&id, h.fake.clone()).await.unwrap();
    assert_eq!(r.state, OperationState::UnknownOutcome);
    assert_eq!(state(&h, &id), OperationState::UnknownOutcome);
    for l in &h.fake.lines()[n..] {
        for m in mutating_marks() {
            assert!(!l.contains(m), "reconcile ran: {l}");
        }
    }
    assert!(!h.fake.lock_stopped());
}

#[tokio::test]
async fn reconcile_with_a_finished_unit_verifies_and_releases_the_lock() {
    let h = harness(vec![durable("apt-install")]);
    let id = lost_install(&h).await;
    h.fake.script(0, vec![Scripted::Out(s_ok())]);
    let r = h.engine.reconcile(&id, h.fake.clone()).await.unwrap();
    assert_eq!(r.state, OperationState::Succeeded);
    assert_eq!(statuses(&h, &id), ["succeeded"]);
    assert!(h.fake.lock_stopped());
    // Reconcile does not clean the unit up.
    assert!(!h.fake.any("'stop' 'ossiframe-op-"));
}

#[tokio::test]
async fn reconcile_with_a_failed_unit_fails_and_a_finished_first_step_never_resumes() {
    let h = harness(vec![durable("apt-install")]);
    let id = lost_install(&h).await;
    h.fake.script(0, vec![Scripted::Out(s_failed())]);
    let r = h.engine.reconcile(&id, h.fake.clone()).await.unwrap();
    assert_eq!(r.state, OperationState::Failed);
    assert!(h.fake.lock_stopped());

    // apt-update finished, apt-install was never launched: nothing resumes.
    let h = harness(vec![durable("apt-update"), durable("apt-install")]);
    let id = lost_install(&h).await;
    h.fake.script(0, vec![Scripted::Out(s_ok())]);
    let r = h.engine.reconcile(&id, h.fake.clone()).await.unwrap();
    assert_eq!(r.state, OperationState::UnknownOutcome);
    assert_eq!(*lk(&h.fake.launches), 1);
    assert_eq!(statuses(&h, &id), ["succeeded", "pending"]);
}

#[tokio::test]
async fn missing_unit_with_a_launched_step_is_not_failed() {
    // Clean evidence and every package installed: verification decides.
    let h = harness(vec![durable("apt-install")]);
    let id = lost_install(&h).await;
    h.fake.script(0, vec![Scripted::Out(s_missing())]);
    let r = h.engine.reconcile(&id, h.fake.clone()).await.unwrap();
    assert_eq!(r.state, OperationState::Succeeded);

    // Inconclusive evidence (an apt process exists) stays UnknownOutcome.
    let h = harness(vec![durable("apt-install")]);
    let id = lost_install(&h).await;
    h.fake.script(0, vec![Scripted::Out(s_missing())]);
    lk(&h.fake.probes).pgrep_busy = true;
    let r = h.engine.reconcile(&id, h.fake.clone()).await.unwrap();
    assert_eq!(r.state, OperationState::UnknownOutcome);

    // Packages not installed: still UnknownOutcome, never Failed.
    let h = harness(vec![durable("apt-install")]);
    let id = lost_install(&h).await;
    h.fake.script(0, vec![Scripted::Out(s_missing())]);
    lk(&h.fake.probes).pkg_out = String::new();
    let r = h.engine.reconcile(&id, h.fake.clone()).await.unwrap();
    assert_eq!(r.state, OperationState::UnknownOutcome);
}

#[tokio::test]
async fn missing_unit_with_a_never_launched_step_fails_from_the_step_record() {
    let h = harness(vec![durable("apt-update")]);
    // The connection stops matching right after the lock was taken, so no
    // step is ever launched.
    *lk(&h.fake.on_lock) = Some(Arc::new(|f: &Fake| *lk(&f.ident) = Some(identity("other"))));
    let id = run_plan(&h).await;
    assert_eq!(state(&h, &id), OperationState::UnknownOutcome);
    assert_eq!(statuses(&h, &id), ["pending"]);
    h.fake.reconnect();
    let r = h.engine.reconcile(&id, h.fake.clone()).await.unwrap();
    assert_eq!(r.state, OperationState::Failed);
    assert_eq!(statuses(&h, &id), ["skipped"]);
    assert!(h.fake.lock_stopped());
}

#[tokio::test]
async fn reconcile_refuses_a_connection_that_is_not_the_recorded_server() {
    let h = harness(vec![durable("apt-install")]);
    let id = lost_install(&h).await;
    *lk(&h.fake.ident) = Some(identity("other-key"));
    let n = h.fake.lines().len();
    let r = h.engine.reconcile(&id, h.fake.clone()).await;
    assert_eq!(r.unwrap_err(), EngineError::IdentityMismatch);
    assert_eq!(h.fake.lines().len(), n, "no command was sent");
}

// ---- mark resolved ----------------------------------------------------------------------

#[tokio::test]
async fn mark_resolved_is_gated_on_the_backends_own_evidence() {
    let h = harness(vec![durable("apt-install")]);
    let id = lost_install(&h).await;

    lk(&h.fake.probes).pgrep_busy = true;
    let e = h
        .engine
        .mark_resolved(&id, h.fake.clone())
        .await
        .unwrap_err();
    assert_eq!(e, EngineError::Refused(StaleRefusal::AptProcesses));
    lk(&h.fake.probes).pgrep_busy = false;

    lk(&h.fake.probes).fuser_busy = true;
    let e = h
        .engine
        .mark_resolved(&id, h.fake.clone())
        .await
        .unwrap_err();
    assert_eq!(e, EngineError::Refused(StaleRefusal::LockFiles));
    lk(&h.fake.probes).fuser_busy = false;

    lk(&h.fake.probes).units_out = "ossiframe-op-a-0.service loaded active running x\n".into();
    let e = h
        .engine
        .mark_resolved(&id, h.fake.clone())
        .await
        .unwrap_err();
    assert_eq!(e, EngineError::Refused(StaleRefusal::StepUnits));
    lk(&h.fake.probes).units_out = String::new();
    assert_eq!(state(&h, &id), OperationState::UnknownOutcome);
    assert!(!h.fake.lock_stopped());

    let r = h.engine.mark_resolved(&id, h.fake.clone()).await.unwrap();
    assert!(!r.evidence.items.is_empty());
    let rec = h.store.get_operation(&id).unwrap();
    assert_eq!(rec.state, OperationState::Failed);
    assert!(rec.resolution_json.unwrap().contains("markedResolved"));
    assert!(h.fake.lock_stopped());
    assert!(
        h.store
            .get_events(&id)
            .unwrap()
            .iter()
            .any(|e| e.kind == "mark-resolved")
    );
    // Only UnknownOutcome can be resolved.
    let e = h
        .engine
        .mark_resolved(&id, h.fake.clone())
        .await
        .unwrap_err();
    assert_eq!(e, EngineError::NotUnknownOutcome);
}

#[tokio::test]
async fn unknown_evidence_blocks_mark_resolved() {
    let h = harness(vec![durable("apt-install")]);
    let id = lost_install(&h).await;
    // fuser fails (for example sudo unavailable): unknown is never clear.
    *lk(&h.fake.probes) = Probes {
        pkg_out: String::new(),
        ..Probes::default()
    };
    h.fake.dead.store(false, Ordering::SeqCst);
    struct Failing(Arc<Fake>);
    #[async_trait]
    impl Executor for Failing {
        async fn run(&self, s: &CommandSpec, c: &CancelToken) -> Result<ExecResult, ExecError> {
            if s.program().ends_with("fuser") {
                return out(127, "");
            }
            self.0.run(s, c).await
        }
    }
    impl ServerLink for Failing {
        fn server_id(&self) -> &str {
            SERVER
        }
        fn identity(&self) -> Option<ServerIdentity> {
            self.0.identity()
        }
        fn take_session_cancel(&self) -> bool {
            false
        }
    }
    let e = h
        .engine
        .mark_resolved(&id, Arc::new(Failing(h.fake.clone())))
        .await
        .unwrap_err();
    assert_eq!(e, EngineError::Refused(StaleRefusal::LockFiles));
}

// ---- pure decision table ------------------------------------------------------------------

fn facts(statuses: &[&str]) -> Vec<StepFacts> {
    statuses
        .iter()
        .enumerate()
        .map(|(i, s)| StepFacts {
            idx: i as u32,
            step_id: if i == 0 && statuses.len() > 1 {
                "apt-update".into()
            } else {
                "apt-install".into()
            },
            mode: StepMode::Durable,
            status: (*s).into(),
        })
        .collect()
}

fn unit(obs: UnitObs) -> UnitShow {
    UnitShow {
        obs,
        active: "x".into(),
        sub: "y".into(),
        exit_status: Some(0),
        main_pid: Some(0),
    }
}

fn evidence(unit: Option<Result<UnitShow, String>>, clear: bool, installed: bool) -> EvidenceSet {
    let mut pk = BTreeMap::new();
    pk.insert(
        "nginx".to_owned(),
        if installed {
            PackageState::Installed {
                version: "1".into(),
            }
        } else {
            PackageState::Absent
        },
    );
    let e = if clear {
        Evidence::Clear
    } else {
        Evidence::Present
    };
    EvidenceSet {
        unit,
        dpkg_clean: Ok(true),
        apt_processes: e,
        lock_files: Evidence::Clear,
        step_units: Evidence::Clear,
        packages: Ok(pk),
        services: vec![],
    }
}

#[test]
fn decision_table_covers_every_observation() {
    let plan = plan_with(vec![]);
    let d = |f: &[&str], e: &EvidenceSet| decide(&facts(f), e, &plan);
    let u = |o| Some(Ok(unit(o)));
    let stay = |d: Decision| matches!(d, Decision::Stay(_));

    assert!(stay(d(
        &["running"],
        &evidence(u(UnitObs::Running), true, true)
    )));
    assert_eq!(
        d(&["running"], &evidence(u(UnitObs::Succeeded), true, true)),
        Decision::UnitSucceeded { idx: 0, last: true }
    );
    assert_eq!(
        d(
            &["running", "pending"],
            &evidence(u(UnitObs::Succeeded), true, true)
        ),
        Decision::UnitSucceeded {
            idx: 0,
            last: false
        }
    );
    assert!(matches!(
        d(
            &["succeeded", "running"],
            &evidence(
                u(UnitObs::Failed {
                    result: "exit-code".into()
                }),
                true,
                true
            )
        ),
        Decision::UnitFailed { idx: 1, .. }
    ));
    assert!(stay(d(
        &["running"],
        &evidence(u(UnitObs::Other("inactive".into())), true, true)
    )));
    assert!(stay(d(&["running"], &evidence(None, true, true))));
    assert!(stay(d(
        &["running"],
        &evidence(Some(Err("x".into())), true, true)
    )));

    // Missing unit with a launched step.
    let missing = |c, i| evidence(u(UnitObs::NotFound), c, i);
    assert_eq!(
        d(&["succeeded", "unknown"], &missing(true, true)),
        Decision::InferredComplete { idx: 1 }
    );
    assert!(stay(d(&["unknown"], &missing(false, true))));
    assert!(stay(d(&["unknown"], &missing(true, false))));
    // apt-update never implies apt-install, and is never inferred itself.
    assert!(stay(d(&["unknown", "pending"], &missing(true, true))));
    // A step that ran over the connection cannot be read back.
    let mut f = facts(&["running"]);
    f[0].mode = StepMode::Foreground;
    assert!(stay(decide(&f, &missing(true, true), &plan)));

    // Nothing launched, or everything recorded.
    assert_eq!(
        d(&["pending", "pending"], &evidence(None, true, true)),
        Decision::NeverLaunched { next: 0 }
    );
    assert_eq!(
        d(&["succeeded", "pending"], &evidence(None, true, true)),
        Decision::NeverLaunched { next: 1 }
    );
    assert_eq!(
        d(&["succeeded", "succeeded"], &evidence(None, true, true)),
        Decision::AllStepsRecorded
    );
}

#[test]
fn unit_naming_uses_the_full_uuid() {
    let id = "0b0c5a5e-1d8e-4a54-9a43-0d1c2f3a4b5c";
    assert_eq!(reconcile::unit_name(id, 2), format!("ossiframe-op-{id}-2"));
}
