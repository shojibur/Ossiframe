//! The operation engine: runs an approved plan, reconciles an interrupted
//! operation and handles the manual "mark resolved" decision.
//!
//! Rules that shape this file:
//! - The remote lock is taken before the first mutating step and held through
//!   verification. It is released only in Succeeded, Failed or Cancelled and
//!   kept in UnknownOutcome.
//! - A durable step runs as a transient systemd unit. Its terminal result is
//!   persisted before any cleanup. A unit that may still run apt or dpkg is
//!   never stopped, killed or signalled. A client-side deadline only stops
//!   waiting.
//! - A disconnect, deadline, identity mismatch or exhausted poll retry gives
//!   UnknownOutcome. No mutating command is ever retried or relaunched.
//! - A cancel request is honoured between steps only.
//! - A successful step never implies plan success: only verification does.
use super::plans::{ApprovedPlan, ServerIdentity, canonical_json, verify_stored_plan};
use super::reconcile::{self, Decision, EvidenceReport, StepFacts, UnitObs, UnitShow, unit_name};
use crate::ipc::cap;
use crate::remote_lock::{
    self as lock, AcquireOutcome, HolderInfo, LockError, OwnerRecord, StaleRefusal,
    check_stale_release,
};
use crate::service::ServerService;
use crate::session::ConnState;
use crate::store::{
    EventRecord, NewOperation, OperationIdentity, OperationRecord, StepRecord, Store, StoreError,
    now,
};
use async_trait::async_trait;
use ossiframe_core::command::{CommandSpec, Privilege};
use ossiframe_core::executor::{CancelToken, ExecError, ExecResult, Executor};
use ossiframe_core::provision::{
    CheckResult, CheckStatus, ComponentVerdict, Drift, Event as StateEvent, OperationState, Plan,
    PlanStep, Provisioner, StepMode, next,
};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;

/// Poll and refresh intervals. The defaults are the production values.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub poll_interval: Duration,
    pub journal_interval: Duration,
    /// Consecutive failed or timed-out polls before the outcome is unknown.
    pub max_poll_misses: u32,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(3),
            journal_interval: Duration::from_secs(15),
            max_poll_misses: 3,
        }
    }
}

/// Notifications for the UI layer. Free of Tauri types, like `SessionEvents`.
pub trait OperationEvents: Send + Sync {
    fn operation_updated(&self, server_id: &str, operation_id: &str, state: OperationState);
}

pub struct NoOperationEvents;

impl OperationEvents for NoOperationEvents {
    fn operation_updated(&self, _: &str, _: &str, _: OperationState) {}
}

/// A connection to one server, as the engine sees it.
pub trait ServerLink: Executor {
    fn server_id(&self) -> &str;
    /// Identity of the live connection, or `None` when not connected.
    fn identity(&self) -> Option<ServerIdentity>;
    /// True once if the session's cancel token was tripped (the shared
    /// `cancel_operation` command). That counts as a cancel request and is
    /// never passed to polling commands.
    fn take_session_cancel(&self) -> bool;
}

/// [`ServerLink`] over `ServerService::with_connection`. Each command takes
/// the per-profile guard only for its own duration.
pub struct ServiceLink {
    service: Arc<ServerService>,
    server_id: String,
    session_cancelled: AtomicBool,
}

impl ServiceLink {
    pub fn new(service: Arc<ServerService>, server_id: &str) -> Self {
        Self {
            service,
            server_id: server_id.to_owned(),
            session_cancelled: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl Executor for ServiceLink {
    async fn run(&self, spec: &CommandSpec, cancel: &CancelToken) -> Result<ExecResult, ExecError> {
        let slot: Arc<Mutex<Option<Result<ExecResult, ExecError>>>> = Arc::default();
        let (spec, cancel, out) = (spec.clone(), cancel.clone(), slot.clone());
        // The session token given to the closure is deliberately not used.
        let r = self
            .service
            .with_connection(&self.server_id, move |conn, _session| async move {
                let res = conn.run(&spec, &cancel).await;
                *out.lock().unwrap_or_else(|e| e.into_inner()) = Some(res);
            })
            .await;
        let captured = slot.lock().unwrap_or_else(|e| e.into_inner()).take();
        match r {
            Ok(()) => captured.unwrap_or(Err(ExecError::ConnectionLost)),
            Err(crate::ServiceError::Exec(ExecError::Cancelled)) => {
                self.session_cancelled.store(true, Ordering::SeqCst);
                captured.unwrap_or(Err(ExecError::Cancelled))
            }
            // The command finished before the connection closed.
            Err(crate::ServiceError::Exec(e)) => captured.unwrap_or(Err(e)),
            Err(crate::ServiceError::NotConnected) => Err(ExecError::ConnectionLost),
            Err(e) => Err(ExecError::Channel(cap(&e.to_string()))),
        }
    }
}

impl ServerLink for ServiceLink {
    fn server_id(&self) -> &str {
        &self.server_id
    }

    fn identity(&self) -> Option<ServerIdentity> {
        let v = self.service.get(&self.server_id).ok()?;
        if v.state != ConnState::Connected {
            return None;
        }
        let pin = v.pin?;
        Some(ServerIdentity {
            host: v.record.host.as_str().to_owned(),
            port: v.record.port.get(),
            user: v.record.username.as_str().to_owned(),
            host_key_sha256: pin.sha256,
        })
    }

    fn take_session_cancel(&self) -> bool {
        self.session_cancelled.swap(false, Ordering::SeqCst)
    }
}

// ---- errors and results ---------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StartError {
    /// Another operation, a local claim or the remote lock holds the server.
    #[error("the server is busy")]
    Busy { holder: Option<Box<HolderInfo>> },
    #[error("the server changed since planning; make a new plan")]
    RePlanRequired(Vec<Drift>),
    /// Mutations are not possible (no `systemd-run`, unclassifiable lock).
    #[error("{0}")]
    Blocked(String),
    #[error("storage: {0}")]
    Storage(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EngineError {
    #[error("operation not found")]
    NotFound,
    #[error("the operation is not in UnknownOutcome")]
    NotUnknownOutcome,
    #[error("the operation is already finished")]
    NotActive,
    #[error("another action is running for this server")]
    Busy,
    #[error("the connection does not match the operation's server")]
    IdentityMismatch,
    #[error("the stored plan failed its integrity check")]
    PlanIntegrity,
    #[error("package management may still be active: {0:?}")]
    Refused(StaleRefusal),
    #[error("storage: {0}")]
    Storage(String),
}

impl From<StoreError> for EngineError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::NotFound => EngineError::NotFound,
            other => EngineError::Storage(other.to_string()),
        }
    }
}

pub struct Started {
    pub operation_id: String,
    /// Finishes when the operation leaves the engine's hands (a terminal
    /// state or UnknownOutcome). The commands layer drops it.
    pub task: JoinHandle<()>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileResult {
    pub state: OperationState,
    pub summary: String,
    pub evidence: EvidenceReport,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveResult {
    pub evidence: EvidenceReport,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VerificationDoc<'a> {
    checks: Vec<CheckResult>,
    verdicts: &'a [ComponentVerdict],
}

// ---- engine -------------------------------------------------------------------------

pub struct Engine {
    store: Arc<Store>,
    provisioner: Arc<dyn Provisioner>,
    events: Arc<dyn OperationEvents>,
    timing: Timing,
    /// Local per-server lock. Held while a task, reconcile or resolution runs.
    claimed: Mutex<HashSet<String>>,
    cancels: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

struct ServerClaim {
    engine: Arc<Engine>,
    server_id: String,
}

impl Drop for ServerClaim {
    fn drop(&mut self) {
        lockset(&self.engine.claimed).remove(&self.server_id);
    }
}

fn lockset<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn same_identity(a: &ServerIdentity, s: &OperationIdentity) -> bool {
    a.host == s.host
        && a.port == s.port
        && a.user == s.user
        && a.host_key_sha256 == s.host_key_sha256
}

impl Engine {
    pub fn new(
        store: Arc<Store>,
        provisioner: Arc<dyn Provisioner>,
        events: Arc<dyn OperationEvents>,
        timing: Timing,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            provisioner,
            events,
            timing,
            claimed: Mutex::default(),
            cancels: Mutex::default(),
        })
    }

    fn claim(self: &Arc<Self>, server_id: &str) -> Option<ServerClaim> {
        lockset(&self.claimed)
            .insert(server_id.to_owned())
            .then(|| ServerClaim {
                engine: self.clone(),
                server_id: server_id.to_owned(),
            })
    }

    /// Records a cancel request. A running step is never interrupted; the
    /// task honours the request before it launches the next step. After a
    /// crash, reconcile reads the stored flag.
    pub fn cancel(&self, operation_id: &str) -> Result<(), EngineError> {
        let rec = self.store.get_operation(operation_id)?;
        if rec.state.is_terminal() {
            return Err(EngineError::NotActive);
        }
        self.store.set_cancel_requested(operation_id)?;
        if let Some(flag) = lockset(&self.cancels).get(operation_id) {
            flag.store(true, Ordering::SeqCst);
        }
        let _ = self.store.append_event(
            operation_id,
            &rec.name,
            &EventRecord {
                kind: "cancel-requested".into(),
                from_state: None,
                to_state: None,
                unit_name: None,
                detail: Some("cancel requested, waiting for current step".into()),
                at: String::new(),
            },
        );
        if let Some(sid) = &rec.server_id {
            self.events.operation_updated(sid, operation_id, rec.state);
        }
        Ok(())
    }

    /// Starts an approved plan. Order: recheck, remote lock, operation record,
    /// background task. Nothing mutating runs before the lock is held.
    pub async fn start(
        self: &Arc<Self>,
        approved: ApprovedPlan,
        link: Arc<dyn ServerLink>,
    ) -> Result<Started, StartError> {
        let plan = approved.plan().clone();
        let target = plan.target.clone();
        if link.server_id() != target.server_id {
            return Err(StartError::Blocked("wrong server connection".into()));
        }
        let claim = self
            .claim(&target.server_id)
            .ok_or(StartError::Busy { holder: None })?;
        match self.store.has_unresolved_operation(&target.server_id) {
            Ok(false) => {}
            Ok(true) => return Err(StartError::Busy { holder: None }),
            Err(e) => return Err(StartError::Storage(e.to_string())),
        }
        let snapshot = OperationIdentity {
            server_id: target.server_id.clone(),
            name: target.name.clone(),
            host: target.host.clone(),
            port: target.port,
            user: target.user.clone(),
            host_key_sha256: target.host_key_sha256.clone(),
        };
        if !link
            .identity()
            .is_some_and(|i| same_identity(&i, &snapshot))
        {
            return Err(StartError::Blocked(
                "the connection no longer matches the plan's server".into(),
            ));
        }
        let token = CancelToken::new();
        if let Err(drift) = self.provisioner.recheck(&*link, &token, &plan).await {
            return Err(StartError::RePlanRequired(drift));
        }
        let op_id = uuid::Uuid::new_v4().to_string();
        let installation = self
            .store
            .installation_id()
            .map_err(|e| StartError::Storage(e.to_string()))?;
        let owner = OwnerRecord::new_now(&op_id, &installation)
            .map_err(|e| StartError::Blocked(e.to_string()))?;
        // The record comes first. A disconnect between taking the remote lock
        // and a later database write can then never leave a lock that names
        // an operation the Store does not know.
        self.store
            .insert_operation(&NewOperation {
                id: op_id.clone(),
                identity: snapshot.clone(),
                kind: "provision".into(),
                plan_digest: approved.digest().to_owned(),
                plan_json: canonical_json(&plan),
            })
            .and_then(|()| {
                plan.steps.iter().enumerate().try_for_each(|(i, s)| {
                    self.store
                        .upsert_step(&op_id, &step_row(i as u32, s, None, "pending"))
                })
            })
            .map_err(|e| {
                // A partial record must not count as an unresolved operation.
                let _ = self.store.update_operation_status(
                    &op_id,
                    OperationState::Failed,
                    Some("the operation could not be recorded; nothing ran"),
                );
                StartError::Storage(e.to_string())
            })?;
        match lock::acquire(&*link, &token, &owner).await {
            Ok(AcquireOutcome::Acquired | AcquireOutcome::AlreadyOwned(_)) => {}
            Ok(AcquireOutcome::Busy(h)) => {
                self.end_unstarted(
                    &target.server_id,
                    &op_id,
                    "the remote lock is held by another owner; nothing ran",
                );
                return Err(StartError::Busy {
                    holder: Some(Box::new(h)),
                });
            }
            Ok(AcquireOutcome::Unavailable(m)) => {
                self.end_unstarted(&target.server_id, &op_id, &m);
                return Err(StartError::Blocked(m));
            }
            Err(e) => {
                // The command may have taken the lock before the connection
                // dropped. The record becomes UnknownOutcome, so the lock has
                // an owner the user can resolve.
                let msg = format!("the remote lock could not be confirmed: {e}");
                self.end_lock_unconfirmed(&target.server_id, &op_id, &msg);
                return Err(StartError::Blocked(msg));
            }
        }
        let cancel = Arc::new(AtomicBool::new(false));
        lockset(&self.cancels).insert(op_id.clone(), cancel.clone());
        let mut ctx = OpCtx {
            eng: self.clone(),
            link,
            id: op_id.clone(),
            server_id: target.server_id.clone(),
            label: target.name.clone(),
            plan,
            snapshot,
            owner,
            state: OperationState::Pending,
            token,
            cancel,
            _claim: claim,
        };
        ctx.notify();
        let task = tokio::spawn(async move {
            ctx.run().await;
            lockset(&ctx.eng.cancels).remove(&ctx.id);
        });
        Ok(Started {
            operation_id: op_id,
            task,
        })
    }

    /// Ends a Pending record whose remote lock was refused. No mutation ran.
    fn end_unstarted(&self, server_id: &str, op_id: &str, reason: &str) {
        self.end_pending(server_id, op_id, StateEvent::StartFailed, reason);
    }

    /// The lock result could not be read. The lock may be held, so the
    /// record is UnknownOutcome and never retried automatically.
    fn end_lock_unconfirmed(&self, server_id: &str, op_id: &str, reason: &str) {
        self.end_pending(server_id, op_id, StateEvent::OutcomeLost, reason);
    }

    fn end_pending(&self, server_id: &str, op_id: &str, ev: StateEvent, reason: &str) {
        let Ok(to) = next(OperationState::Pending, ev) else {
            return;
        };
        let _ = self.store.update_operation_status(op_id, to, Some(reason));
        if let Ok(rec) = self.store.get_operation(op_id) {
            let _ = self.store.append_event(
                op_id,
                &rec.name,
                &EventRecord {
                    kind: "transition".into(),
                    from_state: Some(OperationState::Pending),
                    to_state: Some(to),
                    unit_name: None,
                    detail: Some(reason.to_owned()),
                    at: String::new(),
                },
            );
        }
        self.events.operation_updated(server_id, op_id, to);
    }

    fn load_ctx(
        self: &Arc<Self>,
        operation_id: &str,
        link: Arc<dyn ServerLink>,
    ) -> Result<(OpCtx, OperationRecord), EngineError> {
        let rec = self.store.get_operation(operation_id)?;
        if rec.state != OperationState::UnknownOutcome {
            return Err(EngineError::NotUnknownOutcome);
        }
        let server_id = rec.server_id.clone().ok_or(EngineError::NotFound)?;
        if link.server_id() != server_id {
            return Err(EngineError::IdentityMismatch);
        }
        let claim = self.claim(&server_id).ok_or(EngineError::Busy)?;
        if !verify_stored_plan(&rec.plan_json, &rec.plan_digest) {
            return Err(EngineError::PlanIntegrity);
        }
        let plan: Plan =
            serde_json::from_str(&rec.plan_json).map_err(|_| EngineError::PlanIntegrity)?;
        let snapshot = OperationIdentity {
            server_id: server_id.clone(),
            name: rec.name.clone(),
            host: rec.host.clone(),
            port: rec.port,
            user: rec.user.clone(),
            host_key_sha256: rec.host_key_sha256.clone(),
        };
        let installation = self.store.installation_id()?;
        let owner = OwnerRecord::new_now(operation_id, &installation)
            .map_err(|e| EngineError::Storage(e.to_string()))?;
        let ctx = OpCtx {
            eng: self.clone(),
            link,
            id: operation_id.to_owned(),
            server_id,
            label: rec.name.clone(),
            plan,
            snapshot,
            owner,
            state: rec.state,
            token: CancelToken::new(),
            cancel: Arc::new(AtomicBool::new(rec.cancel_requested)),
            _claim: claim,
        };
        // No command is sent when the connection is not the recorded server.
        if !ctx.identity_ok() {
            return Err(EngineError::IdentityMismatch);
        }
        Ok((ctx, rec))
    }

    /// Read-only evidence collection and the decision table. Never launches,
    /// resumes, stops or kills anything.
    pub async fn reconcile(
        self: &Arc<Self>,
        operation_id: &str,
        link: Arc<dyn ServerLink>,
    ) -> Result<ReconcileResult, EngineError> {
        let (mut ctx, rec) = self.load_ctx(operation_id, link)?;
        let rows = self.store.get_steps(operation_id)?;
        let facts = step_facts(&ctx.plan, &rows);
        let recorded_unit = rows
            .iter()
            .rev()
            .find(|r| matches!(r.status.as_str(), "running" | "unknown"))
            .and_then(|r| r.unit_name.clone());
        let ev =
            reconcile::collect(ctx.exec(), &ctx.token, &ctx.plan, recorded_unit.as_deref()).await;
        if !ctx.identity_ok() {
            return Err(EngineError::IdentityMismatch);
        }
        let report = EvidenceReport::from_set(&ev);
        let decision = reconcile::decide(&facts, &ev, &ctx.plan);
        let unit = recorded_unit.as_deref();
        let summary = match decision {
            Decision::Stay(reason) => reason,
            Decision::NeverLaunched { next } => {
                ctx.skip_pending();
                if rec.cancel_requested {
                    ctx.transition(
                        StateEvent::ReconcileCancelled,
                        None,
                        "no step was launched and cancel was requested",
                    );
                    ctx.release_lock().await;
                    "cancelled before any step was launched".to_owned()
                } else {
                    let msg = format!(
                        "interrupted before step {} was launched; nothing ran for it",
                        next + 1
                    );
                    ctx.transition(StateEvent::ReconcileFailed, Some(&msg), &msg);
                    ctx.release_lock().await;
                    msg
                }
            }
            Decision::UnitFailed { idx, result } => {
                let unit = unit.unwrap_or_default();
                ctx.record_finished_unit(idx, unit, false, &ev).await;
                let msg = format!(
                    "step {} failed on the server ({result}). {RECOVERY_HINT}",
                    idx + 1
                );
                ctx.skip_pending();
                ctx.transition(StateEvent::ReconcileFailed, Some(&msg), &msg);
                ctx.release_lock().await;
                msg
            }
            Decision::UnitSucceeded { idx, last } => {
                let unit = unit.unwrap_or_default();
                ctx.record_finished_unit(idx, unit, true, &ev).await;
                if last {
                    ctx.complete_by_verification("the last step finished successfully")
                        .await;
                    "the last step finished; verification decides the outcome".to_owned()
                } else {
                    "this step finished, but later steps were not run; Ossiframe never resumes a plan"
                        .to_owned()
                }
            }
            Decision::InferredComplete { idx } => {
                ctx.mark_step(idx, "unknown");
                ctx.complete_by_verification("package state shows the work completed")
                    .await;
                "package state shows the work completed; verification decides the outcome"
                    .to_owned()
            }
            Decision::AllStepsRecorded => {
                ctx.complete_by_verification("every step is recorded as finished")
                    .await;
                "every step was recorded as finished; verification decides the outcome".to_owned()
            }
        };
        ctx.event(
            "reconcile",
            None,
            None,
            unit,
            &format!("{summary} | {}", report.render()),
        );
        ctx.notify();
        Ok(ReconcileResult {
            state: ctx.state,
            summary,
            evidence: report,
        })
    }

    /// Closes an UnknownOutcome operation after the user inspected the server.
    /// The backend collects its own fresh evidence and refuses unless no
    /// apt/dpkg process exists, the lock files are free and no step unit is
    /// active. It never retries, repairs or verifies.
    pub async fn mark_resolved(
        self: &Arc<Self>,
        operation_id: &str,
        link: Arc<dyn ServerLink>,
    ) -> Result<ResolveResult, EngineError> {
        let (mut ctx, _rec) = self.load_ctx(operation_id, link)?;
        let ev = reconcile::collect(ctx.exec(), &ctx.token, &ctx.plan, None).await;
        if !ctx.identity_ok() {
            return Err(EngineError::IdentityMismatch);
        }
        let report = EvidenceReport::from_set(&ev);
        if let Err(refusal) = check_stale_release(false, &ev.stale()) {
            ctx.event(
                "mark-resolved-refused",
                None,
                None,
                None,
                &format!("{refusal:?} | {}", report.render()),
            );
            return Err(EngineError::Refused(refusal));
        }
        let doc = serde_json::json!({
            "resolution": "markedResolved",
            "decidedAt": now(),
            "decision": "the user inspected the server and closed this operation",
            "evidence": &report,
        });
        self.store
            .set_resolution_json(operation_id, &doc.to_string())?;
        ctx.event(
            "mark-resolved",
            None,
            None,
            None,
            &format!("user decision recorded | {}", report.render()),
        );
        ctx.skip_pending();
        let msg = "closed by the user after inspection; the outcome of the interrupted step was not verified";
        ctx.transition(StateEvent::MarkedResolved, Some(msg), msg);
        ctx.release_lock().await;
        Ok(ResolveResult { evidence: report })
    }
}

const RECOVERY_HINT: &str = "Ossiframe does not repair package state. Inspect the server, then run a new preflight and plan.";

fn step_row(idx: u32, step: &PlanStep, unit: Option<String>, status: &str) -> StepRecord {
    StepRecord {
        idx,
        step_id: step.id.clone(),
        title: step.title.clone(),
        unit_name: unit,
        status: status.to_owned(),
        started_at: None,
        finished_at: None,
        exit_code: None,
        output: None,
    }
}

fn step_facts(plan: &Plan, rows: &[StepRecord]) -> Vec<StepFacts> {
    plan.steps
        .iter()
        .enumerate()
        .map(|(i, s)| StepFacts {
            idx: i as u32,
            step_id: s.id.clone(),
            mode: s.mode,
            status: rows
                .iter()
                .find(|r| r.idx == i as u32)
                .map_or("pending", |r| r.status.as_str())
                .to_owned(),
        })
        .collect()
}

// ---- one operation -------------------------------------------------------------------

enum Flow {
    Done,
    Failed(String),
    Cancelled,
    Lost(String),
}

enum Polled {
    Show(UnitShow),
    /// Timed out, non-zero exit or unparsable. Retried a bounded number of times.
    Unreadable,
    Lost(String),
}

struct OpCtx {
    eng: Arc<Engine>,
    link: Arc<dyn ServerLink>,
    id: String,
    server_id: String,
    label: String,
    plan: Plan,
    snapshot: OperationIdentity,
    owner: OwnerRecord,
    state: OperationState,
    token: CancelToken,
    cancel: Arc<AtomicBool>,
    _claim: ServerClaim,
}

impl OpCtx {
    fn exec(&self) -> &dyn Executor {
        &*self.link
    }

    fn store(&self) -> &Store {
        &self.eng.store
    }

    /// The live connection is the operation's server, endpoint and pinned key.
    fn identity_ok(&self) -> bool {
        self.link.server_id() == self.server_id
            && self
                .link
                .identity()
                .is_some_and(|i| same_identity(&i, &self.snapshot))
    }

    /// The profile was removed. Nothing more is written for the operation.
    fn removed(&self) -> bool {
        matches!(self.store().get_operation(&self.id), Ok(r) if r.server_id.is_none())
    }

    fn notify(&self) {
        self.eng
            .events
            .operation_updated(&self.server_id, &self.id, self.state);
    }

    fn event(
        &self,
        kind: &str,
        from: Option<OperationState>,
        to: Option<OperationState>,
        unit: Option<&str>,
        detail: &str,
    ) {
        if self.removed() {
            return;
        }
        let _ = self.store().append_event(
            &self.id,
            &self.label,
            &EventRecord {
                kind: kind.to_owned(),
                from_state: from,
                to_state: to,
                unit_name: unit.map(str::to_owned),
                detail: Some(detail.to_owned()),
                at: String::new(),
            },
        );
    }

    /// The only place the state changes.
    fn transition(&mut self, ev: StateEvent, error: Option<&str>, detail: &str) -> bool {
        let from = self.state;
        let Ok(to) = next(from, ev) else {
            self.event(
                "illegal-transition",
                Some(from),
                None,
                None,
                &format!("{ev:?}"),
            );
            return false;
        };
        self.state = to;
        if !self.removed() {
            let _ = self.store().update_operation_status(&self.id, to, error);
            self.event("transition", Some(from), Some(to), None, detail);
            self.notify();
        }
        true
    }

    fn cancel_requested(&self) -> bool {
        if self.link.take_session_cancel() {
            self.cancel.store(true, Ordering::SeqCst);
            let _ = self.store().set_cancel_requested(&self.id);
        }
        self.cancel.load(Ordering::SeqCst)
    }

    fn put_step(&self, rec: &StepRecord) {
        if !self.removed() {
            let _ = self.store().upsert_step(&self.id, rec);
        }
    }

    fn put_output(&self, idx: u32, text: &str) {
        if !self.removed() {
            let _ = self
                .store()
                .replace_step_output(&self.id, idx, text.as_bytes(), &[]);
        }
    }

    fn mark_step(&self, idx: u32, status: &str) {
        if let Ok(rows) = self.store().get_steps(&self.id)
            && let Some(mut r) = rows.into_iter().find(|r| r.idx == idx)
        {
            r.status = status.to_owned();
            self.put_step(&r);
        }
    }

    /// Marks every step that was never launched as skipped.
    fn skip_pending(&self) {
        if let Ok(rows) = self.store().get_steps(&self.id) {
            for mut r in rows.into_iter().filter(|r| r.status == "pending") {
                r.status = "skipped".into();
                self.put_step(&r);
            }
        }
    }

    async fn run(&mut self) {
        self.transition(StateEvent::LockAcquired, None, "remote lock held");
        match self.execute_steps().await {
            Flow::Done => {
                self.transition(StateEvent::StepsDone, None, "all steps finished");
                self.verify_and_finish().await;
            }
            Flow::Failed(msg) => {
                self.skip_pending();
                self.transition(StateEvent::StepFailed, Some(&msg), &msg);
                self.release_lock().await;
            }
            Flow::Cancelled => {
                self.transition(StateEvent::CancelHonoured, None, "cancelled between steps");
                self.release_lock().await;
            }
            // The lock is kept: remote work may still be running.
            Flow::Lost(msg) => {
                self.transition(StateEvent::OutcomeLost, Some(&msg), &msg);
            }
        }
    }

    async fn execute_steps(&mut self) -> Flow {
        let steps = self.plan.steps.clone();
        for (i, step) in steps.iter().enumerate() {
            let idx = i as u32;
            if self.cancel_requested() {
                self.skip_pending();
                return Flow::Cancelled;
            }
            if !self.identity_ok() {
                return Flow::Lost(
                    "the connection no longer matches the operation's server; no further command was sent"
                        .into(),
                );
            }
            let flow = match step.mode {
                StepMode::Gate => self.run_gate(idx, step).await,
                StepMode::Foreground => self.run_foreground(idx, step).await,
                StepMode::Durable => self.run_durable(idx, step).await,
            };
            if !matches!(flow, Flow::Done) {
                return flow;
            }
        }
        Flow::Done
    }

    async fn run_gate(&mut self, idx: u32, step: &PlanStep) -> Flow {
        let mut rec = step_row(idx, step, None, "running");
        rec.started_at = Some(now());
        self.put_step(&rec);
        let res = tokio::time::timeout(
            step.deadline,
            self.eng
                .provisioner
                .gate(self.exec(), &self.token, &self.plan, &step.id),
        )
        .await
        .unwrap_or_else(|_| Err("the check timed out".to_owned()));
        rec.finished_at = Some(now());
        match res {
            Ok(text) => {
                rec.status = "succeeded".into();
                self.put_step(&rec);
                self.put_output(idx, &text);
                Flow::Done
            }
            Err(text) => {
                rec.status = "failed".into();
                self.put_step(&rec);
                self.put_output(idx, &text);
                Flow::Failed(format!(
                    "check '{}' failed: {}. {RECOVERY_HINT}",
                    step.title,
                    cap(&text)
                ))
            }
        }
    }

    async fn run_foreground(&mut self, idx: u32, step: &PlanStep) -> Flow {
        let mut rec = step_row(idx, step, None, "running");
        rec.started_at = Some(now());
        self.put_step(&rec);
        let res = self.link.run(&step.command, &self.token).await;
        rec.finished_at = Some(now());
        match res {
            Ok(r) => {
                rec.exit_code = match r.exit {
                    ossiframe_core::ExitStatus::Code(c) => Some(c),
                    _ => None,
                };
                let ok = r.exit.success();
                rec.status = if ok { "succeeded" } else { "failed" }.into();
                self.put_step(&rec);
                self.put_output(idx, &format!("{}\n{}", r.stdout, r.stderr));
                if ok {
                    Flow::Done
                } else {
                    Flow::Failed(format!("step '{}' failed. {RECOVERY_HINT}", step.title))
                }
            }
            Err(ExecError::PrivilegeDenied) => {
                rec.status = "failed".into();
                self.put_step(&rec);
                Flow::Failed(format!("step '{}' was not allowed to run", step.title))
            }
            Err(e) => {
                rec.status = "unknown".into();
                self.put_step(&rec);
                Flow::Lost(format!("step '{}' may still be running: {e}", step.title))
            }
        }
    }

    async fn run_durable(&mut self, idx: u32, step: &PlanStep) -> Flow {
        let unit = unit_name(&self.id, idx);
        if step.command.privilege() != Privilege::Root {
            let mut rec = step_row(idx, step, None, "failed");
            rec.finished_at = Some(now());
            self.put_step(&rec);
            return Flow::Failed(format!(
                "step '{}' is invalid: a durable step must be privileged. Nothing was run.",
                step.title
            ));
        }
        // Persisted as Running before the launch, so a crash leaves a record
        // that says the unit may exist.
        let mut rec = step_row(idx, step, Some(unit.clone()), "running");
        rec.started_at = Some(now());
        self.put_step(&rec);
        self.event("unit-launch", None, None, Some(&unit), &step.title);
        let launched = self
            .link
            .run(
                &reconcile::launch_command(&unit, &step.command),
                &self.token,
            )
            .await;
        match launched {
            Err(e) => {
                rec.status = "unknown".into();
                self.put_step(&rec);
                return Flow::Lost(format!(
                    "the launch of step '{}' has an unknown outcome and is not retried: {e}",
                    step.title
                ));
            }
            Ok(r) if !r.exit.success() => match self.poll_once(&unit).await {
                Polled::Show(u) if u.obs == UnitObs::NotFound => {
                    rec.status = "failed".into();
                    rec.finished_at = Some(now());
                    if let ossiframe_core::ExitStatus::Code(c) = r.exit {
                        rec.exit_code = Some(c);
                    }
                    self.put_step(&rec);
                    self.put_output(idx, &format!("{}\n{}", r.stdout, r.stderr));
                    return Flow::Failed(format!(
                        "step '{}' could not be started; nothing ran for it. {RECOVERY_HINT}",
                        step.title
                    ));
                }
                Polled::Show(_) => {} // the unit exists: treat it as launched
                _ => {
                    rec.status = "unknown".into();
                    self.put_step(&rec);
                    return Flow::Lost(format!(
                        "the launch of step '{}' has an unknown outcome and is not retried",
                        step.title
                    ));
                }
            },
            Ok(_) => {}
        }
        self.poll_unit(idx, step, rec, &unit).await
    }

    async fn poll_once(&self, unit: &str) -> Polled {
        match self
            .link
            .run(&reconcile::show_command(unit), &self.token)
            .await
        {
            Ok(r) if r.exit.success() => {
                reconcile::parse_unit_show(&r.stdout).map_or(Polled::Unreadable, Polled::Show)
            }
            Ok(_) | Err(ExecError::Timeout) => Polled::Unreadable,
            Err(e) => Polled::Lost(e.to_string()),
        }
    }

    async fn journal(&self, unit: &str) -> Option<String> {
        match self
            .link
            .run(&reconcile::journal_command(unit), &self.token)
            .await
        {
            Ok(r) if r.exit.success() => Some(r.stdout),
            _ => None,
        }
    }

    async fn poll_unit(
        &mut self,
        idx: u32,
        step: &PlanStep,
        mut rec: StepRecord,
        unit: &str,
    ) -> Flow {
        let timing = self.eng.timing;
        let begun = Instant::now();
        let mut last_journal = Instant::now();
        let mut misses = 0u32;
        let lost = |this: &Self, rec: &mut StepRecord, msg: String| {
            rec.status = "unknown".into();
            this.put_step(rec);
            Flow::Lost(msg)
        };
        loop {
            tokio::time::sleep(timing.poll_interval).await;
            if !self.identity_ok() {
                return lost(
                    self,
                    &mut rec,
                    "the connection no longer matches the operation's server; the step may still be running"
                        .into(),
                );
            }
            match self.poll_once(unit).await {
                Polled::Lost(m) => {
                    return lost(
                        self,
                        &mut rec,
                        format!("connection problem while waiting: {m}"),
                    );
                }
                Polled::Unreadable => {
                    misses += 1;
                    if misses >= timing.max_poll_misses {
                        return lost(
                            self,
                            &mut rec,
                            format!(
                                "the step status could not be read {misses} times in a row; it may still be running"
                            ),
                        );
                    }
                }
                Polled::Show(u) => {
                    misses = 0;
                    match &u.obs {
                        UnitObs::Succeeded | UnitObs::Failed { .. } => {
                            return self.finish_unit(idx, step, rec, unit, &u).await;
                        }
                        UnitObs::Running => {
                            if last_journal.elapsed() >= timing.journal_interval {
                                if let Some(j) = self.journal(unit).await {
                                    self.put_output(idx, &j);
                                }
                                last_journal = Instant::now();
                            }
                        }
                        UnitObs::NotFound => {
                            return lost(
                                self,
                                &mut rec,
                                "the step's unit disappeared; its outcome is unknown, not failed"
                                    .into(),
                            );
                        }
                        UnitObs::Other(s) => {
                            return lost(
                                self,
                                &mut rec,
                                format!(
                                    "the step's unit is in state '{s}'; its outcome is unknown"
                                ),
                            );
                        }
                    }
                }
            }
            if begun.elapsed() >= step.deadline {
                // Stop waiting. Nothing stops, kills or signals the unit.
                return lost(
                    self,
                    &mut rec,
                    format!(
                        "step '{}' passed its time limit; it may still be running on the server and was not stopped",
                        step.title
                    ),
                );
            }
        }
    }

    /// Persist first, clean up after.
    async fn finish_unit(
        &mut self,
        idx: u32,
        step: &PlanStep,
        mut rec: StepRecord,
        unit: &str,
        u: &UnitShow,
    ) -> Flow {
        // After an identity change nothing more is sent to this connection.
        let same_server = self.identity_ok();
        let journal = if same_server {
            self.journal(unit).await
        } else {
            None
        };
        let ok = u.obs == UnitObs::Succeeded;
        rec.status = if ok { "succeeded" } else { "failed" }.into();
        rec.finished_at = Some(now());
        rec.exit_code = u.exit_status;
        self.put_step(&rec);
        if let Some(j) = &journal {
            self.put_output(idx, j);
        }
        self.event(
            "unit-finished",
            None,
            None,
            Some(unit),
            &format!("{:?}", u.obs),
        );
        if same_server && self.identity_ok() {
            self.cleanup_unit(unit, u).await;
        }
        if ok {
            Flow::Done
        } else {
            Flow::Failed(format!(
                "step '{}' failed on the server (exit {}). {RECOVERY_HINT}",
                step.title,
                u.exit_status
                    .map_or("unknown".to_owned(), |c| c.to_string())
            ))
        }
    }

    /// Cleans up only a unit confirmed finished. Failure is not fatal.
    async fn cleanup_unit(&self, unit: &str, u: &UnitShow) {
        let cmd = if u.active == "failed" {
            reconcile::reset_failed_command(unit)
        } else if u.is_idle_exited() {
            reconcile::stop_command(unit)
        } else {
            self.event(
                "cleanup-skipped",
                None,
                None,
                Some(unit),
                "unit not confirmed finished",
            );
            return;
        };
        match self.link.run(&cmd, &self.token).await {
            Ok(r) if r.exit.success() => {}
            _ => self.event("cleanup-failed", None, None, Some(unit), "non-fatal"),
        }
    }

    /// Records the finished unit found by reconcile. No cleanup: reconcile
    /// never changes anything on the server.
    async fn record_finished_unit(
        &self,
        idx: u32,
        unit: &str,
        ok: bool,
        ev: &reconcile::EvidenceSet,
    ) {
        let journal = self.journal(unit).await;
        if let Ok(rows) = self.store().get_steps(&self.id)
            && let Some(mut r) = rows.into_iter().find(|r| r.idx == idx)
        {
            r.status = if ok { "succeeded" } else { "failed" }.into();
            r.finished_at = Some(now());
            if let Some(Ok(u)) = &ev.unit {
                r.exit_code = u.exit_status;
            }
            self.put_step(&r);
        }
        if let Some(j) = journal {
            self.put_output(idx, &j);
        }
    }

    async fn complete_by_verification(&mut self, why: &str) {
        if self.transition(StateEvent::ReconcileCompleted, None, why) {
            self.verify_and_finish().await;
        }
    }

    async fn verify_and_finish(&mut self) {
        if !self.identity_ok() {
            self.transition(
                StateEvent::OutcomeLost,
                Some("connection lost before verification"),
                "connection lost before verification",
            );
            return;
        }
        let checks = self
            .eng
            .provisioner
            .verify(self.exec(), &self.token, &self.plan)
            .await;
        if !self.identity_ok() {
            self.transition(
                StateEvent::OutcomeLost,
                Some("connection lost during verification"),
                "connection lost during verification",
            );
            return;
        }
        let checks: Vec<CheckResult> = checks
            .into_iter()
            .map(|mut c| {
                c.detail = c.detail.chars().take(200).collect();
                c
            })
            .collect();
        let verdicts: Vec<ComponentVerdict> = self
            .plan
            .components
            .iter()
            .map(|c| ComponentVerdict::from_checks(c, &checks))
            .collect();
        let essential_bad: Vec<&CheckResult> = checks
            .iter()
            .filter(|c| c.essential && c.status != CheckStatus::Pass)
            .collect();
        let passed =
            !verdicts.is_empty() && verdicts.iter().all(|v| v.verified) && essential_bad.is_empty();
        let doc = VerificationDoc {
            checks: checks.clone(),
            verdicts: &verdicts,
        };
        if let Ok(json) = serde_json::to_string(&doc) {
            let _ = self.store().set_verification_json(&self.id, &json);
        }
        if passed {
            self.transition(
                StateEvent::VerificationPassed,
                None,
                "every component verified",
            );
        } else {
            let mut msg = String::from("verification failed:");
            for c in essential_bad.iter().take(5) {
                msg.push_str(&format!(" {}/{} ({});", c.component, c.title, c.detail));
            }
            if essential_bad.is_empty() {
                msg.push_str(" not every selected component is verified;");
            }
            self.transition(StateEvent::VerificationFailed, Some(&msg), &msg);
        }
        self.release_lock().await;
    }

    /// Only called in a terminal state. The lock stays in UnknownOutcome.
    async fn release_lock(&self) {
        debug_assert!(
            self.state.is_terminal(),
            "lock released outside a terminal state"
        );
        if !self.state.is_terminal() {
            return;
        }
        if !self.identity_ok() {
            self.event(
                "lock-release-skipped",
                None,
                None,
                None,
                "connection does not match the server; the lock needs the stale-lock action",
            );
            return;
        }
        match lock::release_own(self.exec(), &self.token, &self.owner).await {
            Ok(()) => self.event("lock-released", None, None, None, ""),
            Err(LockError::NotOwner(h)) => self.event(
                "lock-release-skipped",
                None,
                None,
                None,
                &format!("lock held by another owner: {}", h.description),
            ),
            Err(e) => self.event("lock-release-failed", None, None, None, &e.to_string()),
        }
    }
}
