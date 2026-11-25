//! The ten `provision_*` IPC commands.
//!
//! Inputs are typed and validated here: server, plan and operation ids
//! (UUIDs), component ids, a bounded selection list, a 64-hex digest and a
//! list limit. The frontend never sends a program, argv, path, privilege or
//! plan content. Approval is the explicit approval of the plan the backend
//! stored and showed; the digest binds that consent to the stored content.
//!
//! All logic lives in [`Provisioning`] so it can be tested without a Tauri
//! app. The `#[tauri::command]` functions are thin wrappers.
use super::engine::{
    Engine, EngineError, OperationEvents, ServerLink, ServiceLink, StartError, Timing,
};
use super::plans::{
    ApprovalContext, ApprovalError, PlanStore, ServerIdentity, is_component_id, is_digest, is_uuid,
};
use super::reconcile::{self, EvidenceReport};
use crate::ipc::{IpcError, PlanReason, cap};
use crate::remote_lock::{self as lock, LockError, LockState, StaleRefusal};
use crate::service::ServerService;
use crate::store::{EventRecord, MAX_LIST, OperationRecord, Store, StoreError};
use ossiframe_core::executor::CancelToken;
use ossiframe_core::module::OperationError;
use ossiframe_core::provision::{
    ComponentInfo, Conflict, OperationState, Plan, PlanError, PlanTarget, PreflightReport,
    PriorOperation, Provisioner,
};
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

/// Bare names of the provisioning commands, for `AppManifest::commands` and
/// capability `allow-` entries. Separate from `tauri_support::COMMAND_NAMES`,
/// which stays unchanged so apps without provisioning keep their lists.
pub const PROVISION_COMMAND_NAMES: &[&str] = &[
    "provision_components",
    "provision_preflight",
    "provision_create_plan",
    "provision_approve_plan",
    "provision_cancel",
    "provision_reconcile",
    "provision_mark_resolved",
    "provision_release_stale_lock",
    "provision_get_operation",
    "provision_list_operations",
];

/// Most components a selection may name.
pub const MAX_SELECTION: usize = 16;
const DEFAULT_LIST: usize = 20;

// ---- DTOs -----------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightDto {
    pub report: PreflightReport,
    pub can_provision: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanDto {
    pub plan_id: String,
    pub digest: String,
    pub expires_in_secs: u64,
    pub plan: Plan,
    /// Warnings from the preflight that did not block planning.
    pub warnings: Vec<Conflict>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationStartedDto {
    pub operation_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconcileDto {
    pub state: OperationState,
    pub summary: String,
    pub evidence: EvidenceReport,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveDto {
    pub evidence: EvidenceReport,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotDto {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub host_key_sha256: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationSummaryDto {
    pub id: String,
    /// `None` after the server profile was removed.
    pub server_id: Option<String>,
    pub server: SnapshotDto,
    pub kind: String,
    pub plan_digest: String,
    pub state: OperationState,
    pub cancel_requested: bool,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepDto {
    pub idx: u32,
    pub step_id: String,
    pub title: String,
    pub unit_name: Option<String>,
    pub status: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub exit_code: Option<i32>,
    /// Redacted, bounded plain text. Render it as text.
    pub output: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventDto {
    pub kind: String,
    pub from_state: Option<OperationState>,
    pub to_state: Option<OperationState>,
    pub unit_name: Option<String>,
    pub detail: Option<String>,
    pub at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationDetailDto {
    #[serde(flatten)]
    pub summary: OperationSummaryDto,
    pub plan: Option<serde_json::Value>,
    pub steps: Vec<StepDto>,
    pub verification: Option<serde_json::Value>,
    pub resolution: Option<serde_json::Value>,
    pub events: Vec<EventDto>,
}

fn summary(r: &OperationRecord) -> OperationSummaryDto {
    OperationSummaryDto {
        id: r.id.clone(),
        server_id: r.server_id.clone(),
        server: SnapshotDto {
            name: r.name.clone(),
            host: r.host.clone(),
            port: r.port,
            user: r.user.clone(),
            host_key_sha256: r.host_key_sha256.clone(),
        },
        kind: r.kind.clone(),
        plan_digest: r.plan_digest.clone(),
        state: r.state,
        cancel_requested: r.cancel_requested,
        created_at: r.created_at.clone(),
        started_at: r.started_at.clone(),
        finished_at: r.finished_at.clone(),
        error: r.error.as_deref().map(cap),
    }
}

fn json(s: &Option<String>) -> Option<serde_json::Value> {
    s.as_deref().and_then(|t| serde_json::from_str(t).ok())
}

// ---- validation and error mapping --------------------------------------------

fn server_id(id: &str) -> Result<&str, IpcError> {
    if is_uuid(id) {
        Ok(id)
    } else {
        Err(IpcError::NotFound)
    }
}

fn operation_id(id: &str) -> Result<&str, IpcError> {
    server_id(id)
}

fn plan_error(reason: PlanReason, message: &str) -> IpcError {
    IpcError::Plan {
        reason,
        message: message.to_owned(),
    }
}

/// A non-empty, bounded list of valid component ids.
pub fn check_selection(selection: &[String]) -> Result<(), IpcError> {
    let bad = |m: &str| IpcError::Validation {
        field: "selection".into(),
        message: m.into(),
    };
    if selection.is_empty() {
        return Err(bad("select at least one component"));
    }
    if selection.len() > MAX_SELECTION {
        return Err(bad("too many components"));
    }
    if !selection.iter().all(|c| is_component_id(c)) {
        return Err(bad("invalid component id"));
    }
    Ok(())
}

/// Default 20, at least 1, at most the store's limit of 50.
pub fn clamp_limit(limit: Option<u32>) -> usize {
    limit
        .map_or(DEFAULT_LIST, |l| l as usize)
        .clamp(1, MAX_LIST)
}

fn storage(e: &StoreError) -> IpcError {
    IpcError::Storage {
        message: cap(&e.to_string()),
    }
}

fn store_lookup(e: StoreError) -> IpcError {
    match e {
        StoreError::NotFound => IpcError::NotFound,
        other => storage(&other),
    }
}

/// `NotConnected` has no plan reason and maps to its own error kind.
pub fn approval_error(e: ApprovalError) -> IpcError {
    match e.reason() {
        Some(reason) => plan_error(reason, &e.to_string()),
        None => IpcError::NotConnected,
    }
}

fn holder_text(h: &lock::HolderInfo) -> String {
    match &h.owner {
        Some(o) => format!(
            "another Ossiframe operation ({}) holds the remote lock since {}",
            o.operation_id(),
            o.started()
        ),
        None => format!("the remote lock is held: {}", cap(&h.description)),
    }
}

fn start_error(e: StartError) -> IpcError {
    match e {
        StartError::Busy { holder } => plan_error(
            PlanReason::Busy,
            &holder.map_or_else(
                || "another operation is running on this server".to_owned(),
                |h| holder_text(&h),
            ),
        ),
        StartError::RePlanRequired(drift) => {
            let text: Vec<&str> = drift.iter().take(5).map(|d| d.message.as_str()).collect();
            plan_error(
                PlanReason::RePlanRequired,
                &cap(&format!(
                    "the server changed since planning: {}. Make a new plan.",
                    text.join("; ")
                )),
            )
        }
        StartError::Blocked(m) => plan_error(PlanReason::Blocked, &cap(&m)),
        StartError::Storage(m) => IpcError::Storage { message: cap(&m) },
    }
}

fn engine_error(e: EngineError) -> IpcError {
    match e {
        EngineError::NotFound => IpcError::NotFound,
        EngineError::Busy => plan_error(PlanReason::Busy, "another action is running"),
        EngineError::IdentityMismatch => plan_error(
            PlanReason::WrongServer,
            "the connection does not match the operation's server",
        ),
        EngineError::Refused(r) => plan_error(PlanReason::Busy, stale_text(r)),
        EngineError::Storage(m) => IpcError::Storage { message: cap(&m) },
        other => IpcError::OperationFailed {
            message: cap(&other.to_string()),
        },
    }
}

fn stale_text(r: StaleRefusal) -> &'static str {
    match r {
        StaleRefusal::LocalOperationActive => {
            "a local operation still needs review; use reconcile or mark resolved"
        }
        StaleRefusal::StepUnits => "an Ossiframe step unit may still be active on the server",
        StaleRefusal::AptProcesses => "an apt or dpkg process may still be running",
        StaleRefusal::LockFiles => "an apt or dpkg lock file is held or could not be checked",
    }
}

fn lock_error(e: LockError) -> IpcError {
    match e {
        LockError::Exec(x) => x.into(),
        LockError::StaleRefused(r) => plan_error(PlanReason::Busy, stale_text(r)),
        LockError::HolderChanged => plan_error(
            PlanReason::Mismatch,
            "the lock holder changed; inspect it again",
        ),
        other => IpcError::OperationFailed {
            message: cap(&other.to_string()),
        },
    }
}

fn operation_error(e: OperationError) -> IpcError {
    match e {
        OperationError::InvalidArguments(m) => IpcError::Validation {
            field: "selection".into(),
            message: cap(&m),
        },
        OperationError::Exec(x) => x.into(),
        OperationError::Failed(m) => IpcError::OperationFailed { message: cap(&m) },
    }
}

fn planning_error(e: PlanError) -> IpcError {
    match e {
        PlanError::UnknownComponent(_) => IpcError::Validation {
            field: "selection".into(),
            message: "unknown component".into(),
        },
        PlanError::Blocked => plan_error(
            PlanReason::Blocked,
            "preflight found blockers; resolve them and run preflight again",
        ),
        PlanError::Refused(m) => plan_error(PlanReason::Blocked, &cap(&m)),
    }
}

/// Maps a stored operation to what the preflight needs to know. In-flight
/// states and operations of other kinds say nothing about the server.
pub fn prior_from_record(r: &OperationRecord) -> Option<PriorOperation> {
    if r.kind != "provision" {
        return None;
    }
    match r.state {
        OperationState::Succeeded => Some(PriorOperation::Succeeded {
            all_essential_passed: essentials_passed(r.verification_json.as_deref()),
        }),
        OperationState::Failed if r.resolution_json.is_some() => {
            Some(PriorOperation::ClosedManually)
        }
        OperationState::Failed => Some(PriorOperation::Failed),
        OperationState::Cancelled => Some(PriorOperation::Cancelled),
        _ => None,
    }
}

/// True only when the stored verification has verdicts, all verified, and no
/// essential check that is not a pass. Missing or unreadable data is false.
fn essentials_passed(verification: Option<&str>) -> bool {
    let Some(v) = verification.and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok())
    else {
        return false;
    };
    let (Some(checks), Some(verdicts)) = (v["checks"].as_array(), v["verdicts"].as_array()) else {
        return false;
    };
    !verdicts.is_empty()
        && verdicts.iter().all(|d| d["verified"] == true)
        && checks
            .iter()
            .all(|c| c["essential"] != true || c["status"] == "pass")
}

// ---- state ----------------------------------------------------------------------

/// What the commands need, managed as Tauri state by [`super::setup`].
pub struct Provisioning {
    service: Arc<ServerService>,
    store: Arc<Store>,
    provisioner: Arc<dyn Provisioner>,
    engine: Arc<Engine>,
    plans: PlanStore,
}

struct Profile {
    identity: ServerIdentity,
    name: String,
    connected: bool,
}

impl Provisioning {
    pub fn new(
        service: Arc<ServerService>,
        provisioner: Arc<dyn Provisioner>,
        events: Arc<dyn OperationEvents>,
    ) -> Self {
        let store = service.store();
        let engine = Engine::new(
            store.clone(),
            provisioner.clone(),
            events,
            Timing::default(),
        );
        Self {
            service,
            store,
            provisioner,
            engine,
            plans: PlanStore::default(),
        }
    }

    fn link(&self, server_id: &str) -> Arc<ServiceLink> {
        Arc::new(ServiceLink::new(self.service.clone(), server_id))
    }

    fn profile(&self, id: &str) -> Result<Profile, IpcError> {
        let v = self.service.get(id)?;
        let connected = v.state == crate::session::ConnState::Connected;
        Ok(Profile {
            identity: ServerIdentity {
                host: v.record.host.as_str().to_owned(),
                port: v.record.port.get(),
                user: v.record.username.as_str().to_owned(),
                host_key_sha256: v.pin.map(|p| p.sha256).unwrap_or_default(),
            },
            name: v.record.name.clone().into(),
            connected,
        })
    }

    fn connected_profile(&self, id: &str) -> Result<Profile, IpcError> {
        let p = self.profile(id)?;
        if p.connected {
            Ok(p)
        } else {
            Err(IpcError::NotConnected)
        }
    }

    /// Earlier operations for the same endpoint and pinned key.
    fn prior_operations(
        &self,
        server_id: &str,
        ident: &ServerIdentity,
    ) -> Result<Vec<PriorOperation>, IpcError> {
        Ok(self
            .store
            .list_operations(Some(server_id), MAX_LIST)
            .map_err(|e| storage(&e))?
            .iter()
            .filter(|r| {
                r.host == ident.host
                    && r.port == ident.port
                    && r.user == ident.user
                    && r.host_key_sha256 == ident.host_key_sha256
            })
            .filter_map(prior_from_record)
            .collect())
    }

    pub fn components(&self) -> Vec<ComponentInfo> {
        self.provisioner.components()
    }

    /// Runs the preflight. Nothing is stored.
    async fn preflight_report(
        &self,
        id: &str,
        selection: &[String],
    ) -> Result<(Profile, PreflightReport), IpcError> {
        let id = server_id(id)?;
        check_selection(selection)?;
        let profile = self.connected_profile(id)?;
        let prior = self.prior_operations(id, &profile.identity)?;
        let link = self.link(id);
        let report = self
            .provisioner
            .preflight_for(&*link, &CancelToken::new(), selection, &prior)
            .await
            .map_err(operation_error)?;
        // The evidence belongs to the server that was connected at the start.
        if link.identity().as_ref() != Some(&profile.identity) {
            return Err(plan_error(
                PlanReason::WrongServer,
                "the connection changed during the preflight",
            ));
        }
        Ok((profile, report))
    }

    pub async fn preflight(
        &self,
        id: &str,
        selection: &[String],
    ) -> Result<PreflightDto, IpcError> {
        let (_, report) = self.preflight_report(id, selection).await?;
        Ok(PreflightDto {
            can_provision: report.can_provision(),
            report,
        })
    }

    /// Fresh preflight, then the plan. Refuses on blockers. A failed or
    /// cancelled preflight stores nothing.
    pub async fn create_plan(&self, id: &str, selection: &[String]) -> Result<PlanDto, IpcError> {
        let (profile, report) = self.preflight_report(id, selection).await?;
        if !report.can_provision() {
            return Err(planning_error(PlanError::Blocked));
        }
        let mut plan = self
            .provisioner
            .plan(&report, selection)
            .map_err(planning_error)?;
        // The identity snapshot is set here, before the plan is stored and
        // digested, whatever the provisioner put in it.
        plan.target = PlanTarget {
            server_id: id.to_owned(),
            name: profile.name,
            host: profile.identity.host,
            port: profile.identity.port,
            user: profile.identity.user,
            host_key_sha256: profile.identity.host_key_sha256,
        };
        let handle = self.plans.insert(plan.clone());
        Ok(PlanDto {
            plan_id: handle.id,
            digest: handle.digest,
            expires_in_secs: handle.expires_in.as_secs(),
            plan,
            warnings: report.conflicts,
        })
    }

    pub async fn approve_plan(
        &self,
        id: &str,
        plan_id: &str,
        digest: &str,
    ) -> Result<OperationStartedDto, IpcError> {
        let id = server_id(id)?;
        if !is_uuid(plan_id) {
            return Err(plan_error(PlanReason::UnknownPlan, "unknown plan"));
        }
        if !is_digest(digest) {
            return Err(IpcError::Validation {
                field: "digest".into(),
                message: "must be 64 lowercase hex characters".into(),
            });
        }
        let profile = self.profile(id)?;
        let busy = self
            .store
            .has_unresolved_operation(id)
            .map_err(|e| storage(&e))?;
        let approved = self
            .plans
            .take_approved(
                id,
                plan_id,
                digest,
                &ApprovalContext {
                    identity: profile.identity,
                    connected: profile.connected,
                    busy,
                },
            )
            .map_err(approval_error)?;
        let started = self
            .engine
            .start(approved, self.link(id))
            .await
            .map_err(start_error)?;
        // The task owns the operation from here. Progress arrives through
        // `operation-updated` and `provision_get_operation`.
        drop(started.task);
        Ok(OperationStartedDto {
            operation_id: started.operation_id,
        })
    }

    pub fn cancel(&self, op: &str) -> Result<(), IpcError> {
        self.engine.cancel(operation_id(op)?).map_err(engine_error)
    }

    /// The operation record and a connected link to its server.
    fn operation_link(&self, op: &str) -> Result<(String, Arc<ServiceLink>), IpcError> {
        let rec = self
            .store
            .get_operation(operation_id(op)?)
            .map_err(store_lookup)?;
        let sid = rec.server_id.ok_or(IpcError::NotFound)?;
        self.connected_profile(&sid)?;
        let link = self.link(&sid);
        Ok((rec.id, link))
    }

    pub async fn reconcile(&self, op: &str) -> Result<ReconcileDto, IpcError> {
        let (id, link) = self.operation_link(op)?;
        let r = self
            .engine
            .reconcile(&id, link)
            .await
            .map_err(engine_error)?;
        Ok(ReconcileDto {
            state: r.state,
            summary: cap(&r.summary),
            evidence: r.evidence,
        })
    }

    pub async fn mark_resolved(&self, op: &str) -> Result<ResolveDto, IpcError> {
        let (id, link) = self.operation_link(op)?;
        let r = self
            .engine
            .mark_resolved(&id, link)
            .await
            .map_err(engine_error)?;
        Ok(ResolveDto {
            evidence: r.evidence,
        })
    }

    /// Explicit user action for a lock whose owner is not a local unresolved
    /// operation. The backend gathers its own evidence and the decision is
    /// recorded as an event of the server's latest operation.
    pub async fn release_stale_lock(&self, id: &str) -> Result<(), IpcError> {
        let id = server_id(id)?;
        self.connected_profile(id)?;
        let link = self.link(id);
        let token = CancelToken::new();
        let LockState::Held(holder) = lock::inspect(&*link, &token).await.map_err(lock_error)?
        else {
            return Ok(());
        };
        let owner_active = holder
            .owner
            .as_ref()
            .and_then(|o| self.store.get_operation(o.operation_id()).ok())
            .is_some_and(|r| r.state.is_unresolved());
        let local_active = owner_active
            || self
                .store
                .has_unresolved_operation(id)
                .map_err(|e| storage(&e))?;
        let evidence = reconcile::collect_stale(&*link, &token).await;
        let result =
            lock::release_stale(&*link, &token, &holder.description, local_active, &evidence).await;
        if let Ok(latest) = self.store.list_operations(Some(id), 1)
            && let Some(op) = latest.first()
        {
            let outcome = if result.is_ok() {
                "stale-lock-released"
            } else {
                "stale-lock-release-refused"
            };
            let _ = self.store.append_event(
                &op.id,
                &op.name,
                &EventRecord {
                    kind: outcome.into(),
                    from_state: None,
                    to_state: None,
                    unit_name: None,
                    detail: Some(format!(
                        "user decision | holder: {} | evidence: {evidence:?}",
                        holder.description
                    )),
                    at: String::new(),
                },
            );
        }
        result.map_err(lock_error)
    }

    pub fn get_operation(&self, op: &str) -> Result<OperationDetailDto, IpcError> {
        let rec = self
            .store
            .get_operation(operation_id(op)?)
            .map_err(store_lookup)?;
        let steps = self.store.get_steps(&rec.id).map_err(|e| storage(&e))?;
        let events = self.store.get_events(&rec.id).map_err(|e| storage(&e))?;
        Ok(OperationDetailDto {
            summary: summary(&rec),
            plan: serde_json::from_str(&rec.plan_json).ok(),
            steps: steps
                .into_iter()
                .map(|s| StepDto {
                    idx: s.idx,
                    step_id: s.step_id,
                    title: s.title,
                    unit_name: s.unit_name,
                    status: s.status,
                    started_at: s.started_at,
                    finished_at: s.finished_at,
                    exit_code: s.exit_code,
                    output: s.output,
                })
                .collect(),
            verification: json(&rec.verification_json),
            resolution: json(&rec.resolution_json),
            events: events
                .into_iter()
                .map(|e| EventDto {
                    kind: e.kind,
                    from_state: e.from_state,
                    to_state: e.to_state,
                    unit_name: e.unit_name,
                    detail: e.detail,
                    at: e.at,
                })
                .collect(),
        })
    }

    pub fn list_operations(
        &self,
        id: Option<&str>,
        limit: Option<u32>,
    ) -> Result<Vec<OperationSummaryDto>, IpcError> {
        let id = id.map(server_id).transpose()?;
        Ok(self
            .store
            .list_operations(id, clamp_limit(limit))
            .map_err(|e| storage(&e))?
            .iter()
            .map(summary)
            .collect())
    }
}

// ---- Tauri glue --------------------------------------------------------------

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdatedPayload<'a> {
    server_id: &'a str,
    operation_id: &'a str,
    state: OperationState,
}

/// Emits `operation-updated` to the frontend. The UI then reads the operation
/// with `provision_get_operation`.
pub struct TauriOperationEvents<R: Runtime>(pub AppHandle<R>);

impl<R: Runtime> OperationEvents for TauriOperationEvents<R> {
    fn operation_updated(&self, server_id: &str, operation_id: &str, state: OperationState) {
        let _ = self.0.emit(
            "operation-updated",
            UpdatedPayload {
                server_id,
                operation_id,
                state,
            },
        );
    }
}

/// Registers the provisioning state and runs startup recovery. Call after
/// `tauri_support::setup`, which provides the shared service.
pub fn setup<R: Runtime>(
    app: &mut tauri::App<R>,
    provisioner: Arc<dyn Provisioner>,
) -> Result<(), Box<dyn std::error::Error>> {
    let service = app
        .try_state::<ServerService>()
        .ok_or("tauri_support::setup must run before provisioning::setup")?
        .inner()
        .clone();
    let service = Arc::new(service);
    // The app may have stopped mid-operation. Nothing is retried.
    service
        .store()
        .recover_in_flight_operations()
        .map_err(|e| format!("cannot recover operations: {e}"))?;
    let events = Arc::new(TauriOperationEvents(app.handle().clone()));
    app.manage(Provisioning::new(service, provisioner, events));
    Ok(())
}

type Prov<'a> = State<'a, Provisioning>;

#[tauri::command]
pub async fn provision_components(state: Prov<'_>) -> Result<Vec<ComponentInfo>, IpcError> {
    Ok(state.components())
}

#[tauri::command]
pub async fn provision_preflight(
    state: Prov<'_>,
    server_id: String,
    selection: Vec<String>,
) -> Result<PreflightDto, IpcError> {
    state.preflight(&server_id, &selection).await
}

#[tauri::command]
pub async fn provision_create_plan(
    state: Prov<'_>,
    server_id: String,
    selection: Vec<String>,
) -> Result<PlanDto, IpcError> {
    state.create_plan(&server_id, &selection).await
}

#[tauri::command]
pub async fn provision_approve_plan(
    state: Prov<'_>,
    server_id: String,
    plan_id: String,
    digest: String,
) -> Result<OperationStartedDto, IpcError> {
    state.approve_plan(&server_id, &plan_id, &digest).await
}

#[tauri::command]
pub async fn provision_cancel(state: Prov<'_>, operation_id: String) -> Result<(), IpcError> {
    state.cancel(&operation_id)
}

#[tauri::command]
pub async fn provision_reconcile(
    state: Prov<'_>,
    operation_id: String,
) -> Result<ReconcileDto, IpcError> {
    state.reconcile(&operation_id).await
}

#[tauri::command]
pub async fn provision_mark_resolved(
    state: Prov<'_>,
    operation_id: String,
) -> Result<ResolveDto, IpcError> {
    state.mark_resolved(&operation_id).await
}

#[tauri::command]
pub async fn provision_release_stale_lock(
    state: Prov<'_>,
    server_id: String,
) -> Result<(), IpcError> {
    state.release_stale_lock(&server_id).await
}

#[tauri::command]
pub async fn provision_get_operation(
    state: Prov<'_>,
    operation_id: String,
) -> Result<OperationDetailDto, IpcError> {
    state.get_operation(&operation_id)
}

#[tauri::command]
pub async fn provision_list_operations(
    state: Prov<'_>,
    server_id: Option<String>,
    limit: Option<u32>,
) -> Result<Vec<OperationSummaryDto>, IpcError> {
    state.list_operations(server_id.as_deref(), limit)
}

#[cfg(test)]
mod tests {
    use super::super::engine::NoOperationEvents;
    use super::*;
    use crate::service::{Connector, ProfileInput};
    use crate::session::{Connection, NoEvents, tests::FakeConn};
    use crate::store::AuthConfig;
    use crate::vault::MemoryVault;
    use async_trait::async_trait;
    use ossiframe_core::executor::Executor;
    use ossiframe_core::host_key::{PinnedHostKey, PresentedHostKey};
    use ossiframe_core::provision::{
        CheckResult, Conflict, Drift, Preconditions, Resources, ServerClass, Severity, SudoMode,
    };
    use ossiframe_core::{Hostname, Port, ServerName, SshUser};
    use ossiframe_ssh::{AuthMethod, ConnectError, ConnectTarget};
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    const EXPECTED: [&str; 10] = [
        "provision_components",
        "provision_preflight",
        "provision_create_plan",
        "provision_approve_plan",
        "provision_cancel",
        "provision_reconcile",
        "provision_mark_resolved",
        "provision_release_stale_lock",
        "provision_get_operation",
        "provision_list_operations",
    ];

    #[test]
    fn command_names_are_exactly_the_ten_and_do_not_touch_the_shared_list() {
        assert_eq!(PROVISION_COMMAND_NAMES, EXPECTED);
        let src = include_str!("commands.rs");
        for n in PROVISION_COMMAND_NAMES {
            assert!(
                src.contains(&format!("pub async fn {n}(")),
                "missing fn {n}"
            );
            assert!(!crate::tauri_support::COMMAND_NAMES.contains(n));
        }
        assert_eq!(crate::tauri_support::COMMAND_NAMES.len(), 11);
    }

    #[test]
    fn command_parameters_are_only_ids_a_selection_a_digest_and_a_limit() {
        let src = include_str!("commands.rs");
        let allowed = [
            "state",
            "server_id",
            "selection",
            "plan_id",
            "digest",
            "operation_id",
            "limit",
        ];
        let mut seen = 0;
        for part in src.split("#[tauri::command]\npub async fn ").skip(1) {
            let sig = &part[..part.find(") ->").unwrap()];
            let params = &sig[sig.find('(').unwrap() + 1..];
            for p in params.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                let name = p.split(':').next().unwrap().trim();
                assert!(allowed.contains(&name), "unexpected parameter {name}");
            }
            seen += 1;
        }
        assert_eq!(seen, EXPECTED.len());
    }

    #[test]
    fn inputs_are_validated_and_the_list_limit_is_clamped() {
        assert!(check_selection(&["git".into(), "php-fpm".into()]).is_ok());
        for bad in [
            vec![],
            vec!["Git".to_owned()],
            vec!["a b".to_owned()],
            vec!["x".to_owned(); MAX_SELECTION + 1],
            vec!["../x".to_owned()],
        ] {
            assert!(check_selection(&bad).is_err(), "{bad:?}");
        }
        assert_eq!(clamp_limit(None), 20);
        assert_eq!(clamp_limit(Some(0)), 1);
        assert_eq!(clamp_limit(Some(10_000)), 50);
        assert_eq!(server_id("nope"), Err(IpcError::NotFound));
    }

    #[test]
    fn approval_errors_map_to_plan_reasons_and_not_connected_to_its_own_kind() {
        use ApprovalError as A;
        let reason = |e| match approval_error(e) {
            IpcError::Plan { reason, .. } => reason,
            other => panic!("{other:?}"),
        };
        assert_eq!(reason(A::UnknownPlan), PlanReason::UnknownPlan);
        assert_eq!(reason(A::Superseded), PlanReason::Superseded);
        assert_eq!(reason(A::Expired), PlanReason::Expired);
        assert_eq!(reason(A::Mismatch), PlanReason::Mismatch);
        assert_eq!(reason(A::WrongServer), PlanReason::WrongServer);
        assert_eq!(reason(A::Busy), PlanReason::Busy);
        assert_eq!(approval_error(A::NotConnected), IpcError::NotConnected);
    }

    fn record(
        state: OperationState,
        verification: Option<&str>,
        resolved: bool,
    ) -> OperationRecord {
        OperationRecord {
            id: "o".into(),
            server_id: Some("s".into()),
            name: "n".into(),
            host: "h".into(),
            port: 22,
            user: "u".into(),
            host_key_sha256: "k".into(),
            kind: "provision".into(),
            plan_digest: "0".repeat(64),
            plan_json: "{}".into(),
            state,
            cancel_requested: false,
            created_at: String::new(),
            started_at: None,
            finished_at: None,
            error: None,
            verification_json: verification.map(str::to_owned),
            resolution_json: resolved.then(|| "{}".to_owned()),
        }
    }

    #[test]
    fn prior_operations_are_derived_conservatively() {
        let good = r#"{"checks":[{"essential":true,"status":"pass"},{"essential":false,"status":"warn"}],"verdicts":[{"verified":true}]}"#;
        let bad =
            r#"{"checks":[{"essential":true,"status":"fail"}],"verdicts":[{"verified":false}]}"#;
        let p = |s, v, r| prior_from_record(&record(s, v, r));
        assert_eq!(
            p(OperationState::Succeeded, Some(good), false),
            Some(PriorOperation::Succeeded {
                all_essential_passed: true
            })
        );
        for v in [Some(bad), None, Some("not json")] {
            assert_eq!(
                p(OperationState::Succeeded, v, false),
                Some(PriorOperation::Succeeded {
                    all_essential_passed: false
                })
            );
        }
        assert_eq!(
            p(OperationState::Failed, None, true),
            Some(PriorOperation::ClosedManually)
        );
        assert_eq!(
            p(OperationState::Failed, None, false),
            Some(PriorOperation::Failed)
        );
        assert_eq!(
            p(OperationState::Cancelled, None, false),
            Some(PriorOperation::Cancelled)
        );
        assert_eq!(p(OperationState::UnknownOutcome, None, false), None);
    }

    // ---- planning through a fake connection ----

    struct FakeProv {
        report: Mutex<Result<PreflightReport, OperationError>>,
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
            unreachable!()
        }
        async fn preflight_for(
            &self,
            _: &dyn Executor,
            _: &CancelToken,
            _: &[String],
            _: &[PriorOperation],
        ) -> Result<PreflightReport, OperationError> {
            self.report.lock().unwrap().clone()
        }
        fn plan(&self, _: &PreflightReport, _: &[String]) -> Result<Plan, PlanError> {
            Ok(Plan {
                target: PlanTarget {
                    server_id: "provisioner-wrote-this".into(),
                    name: String::new(),
                    host: String::new(),
                    port: 0,
                    user: String::new(),
                    host_key_sha256: String::new(),
                },
                os: "ubuntu".into(),
                components: vec!["git".into()],
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
                steps: vec![],
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
            })
        }
        async fn recheck(
            &self,
            _: &dyn Executor,
            _: &CancelToken,
            _: &Plan,
        ) -> Result<(), Vec<Drift>> {
            Ok(())
        }
        async fn gate(
            &self,
            _: &dyn Executor,
            _: &CancelToken,
            _: &Plan,
            _: &str,
        ) -> Result<String, String> {
            Ok(String::new())
        }
        async fn verify(&self, _: &dyn Executor, _: &CancelToken, _: &Plan) -> Vec<CheckResult> {
            vec![]
        }
    }

    struct FakeConnector(Arc<FakeConn>);

    #[async_trait]
    impl Connector for FakeConnector {
        async fn connect(
            &self,
            _: &ConnectTarget,
            _: &AuthMethod,
            _: Option<PinnedHostKey>,
        ) -> Result<Arc<dyn Connection>, ConnectError> {
            Ok(self.0.clone())
        }
    }

    fn report(blocker: bool) -> PreflightReport {
        PreflightReport {
            observed_at: "2026-01-01T00:00:00Z".into(),
            os: "ubuntu 24.04".into(),
            sudo: SudoMode::Passwordless,
            server_class: ServerClass::Fresh,
            packages: BTreeMap::new(),
            total_mem_mb: Some(4096),
            free_disk_mb: Some(40_000),
            package_sources: vec![],
            candidate_versions: BTreeMap::new(),
            sites: vec![],
            conflicts: if blocker {
                vec![Conflict {
                    id: "x".into(),
                    severity: Severity::Blocker,
                    message: "blocked".into(),
                }]
            } else {
                vec![]
            },
        }
    }

    async fn connected(
        report: Result<PreflightReport, OperationError>,
    ) -> (Provisioning, String, Arc<FakeConn>) {
        let conn = Arc::new(FakeConn::default());
        let svc = ServerService::new(
            Store::open_in_memory().unwrap(),
            Arc::new(MemoryVault::default()),
            Arc::new(FakeConnector(conn.clone())),
            Arc::new(NoEvents),
        );
        let id = svc
            .add(ProfileInput {
                name: ServerName::parse("web").unwrap(),
                host: Hostname::parse("example.com").unwrap(),
                port: Port::SSH_DEFAULT,
                username: SshUser::parse("deploy").unwrap(),
                auth: AuthConfig::Password,
            })
            .unwrap()
            .record
            .id;
        svc.store()
            .pin_host_key(
                &id,
                &PresentedHostKey {
                    key_type: "ssh-ed25519".into(),
                    public_key: vec![1, 2, 3],
                    sha256: "SHA256:abc".into(),
                },
            )
            .unwrap();
        svc.connect(&id, None, false).await.unwrap();
        let prov = Provisioning::new(
            Arc::new(svc),
            Arc::new(FakeProv {
                report: Mutex::new(report),
            }),
            Arc::new(NoOperationEvents),
        );
        (prov, id, conn)
    }

    /// True when the server has no stored plan.
    fn no_plan(p: &Provisioning, id: &str) -> bool {
        let ctx = ApprovalContext {
            identity: ServerIdentity {
                host: String::new(),
                port: 0,
                user: String::new(),
                host_key_sha256: String::new(),
            },
            connected: true,
            busy: false,
        };
        matches!(
            p.plans.take_approved(id, "x", &"0".repeat(64), &ctx),
            Err(ApprovalError::UnknownPlan)
        )
    }

    #[tokio::test]
    async fn failed_or_blocked_preflight_stores_no_plan() {
        let sel = vec!["git".to_owned()];
        let (p, id, _) = connected(Err(OperationError::Exec(
            ossiframe_core::ExecError::Cancelled,
        )))
        .await;
        assert_eq!(
            p.create_plan(&id, &sel).await.unwrap_err(),
            IpcError::Cancelled
        );
        assert!(no_plan(&p, &id));

        let (p, id, _) = connected(Ok(report(true))).await;
        let err = p.create_plan(&id, &sel).await.unwrap_err();
        assert!(matches!(
            err,
            IpcError::Plan {
                reason: PlanReason::Blocked,
                ..
            }
        ));
        assert!(no_plan(&p, &id));
        // The preflight itself still reports the blocker to the user.
        assert!(!p.preflight(&id, &sel).await.unwrap().can_provision);
    }

    #[tokio::test]
    async fn create_plan_fills_the_identity_snapshot_before_storing() {
        let (p, id, _) = connected(Ok(report(false))).await;
        let dto = p.create_plan(&id, &["git".to_owned()]).await.unwrap();
        let t = &dto.plan.target;
        assert_eq!(t.server_id, id);
        assert_eq!(
            (t.name.as_str(), t.host.as_str(), t.port, t.user.as_str()),
            ("web", "example.com", 22, "deploy")
        );
        assert_eq!(t.host_key_sha256, "SHA256:abc");
        assert_eq!(dto.digest, super::super::plans::plan_digest(&dto.plan));
        // A wrong digest is refused and consumes nothing but that plan.
        let err = p
            .approve_plan(&id, &dto.plan_id, &"0".repeat(64))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            IpcError::Plan {
                reason: PlanReason::Mismatch,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn malformed_ids_are_refused_before_any_command_runs() {
        let (p, id, conn) = connected(Ok(report(false))).await;
        let before = conn.runs.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            p.preflight("bad", &["git".to_owned()]).await.unwrap_err(),
            IpcError::NotFound
        );
        assert!(matches!(
            p.preflight(&id, &["Bad Id".to_owned()]).await.unwrap_err(),
            IpcError::Validation { .. }
        ));
        assert!(matches!(
            p.approve_plan(&id, &id, "short").await.unwrap_err(),
            IpcError::Validation { .. }
        ));
        assert_eq!(p.cancel("nope"), Err(IpcError::NotFound));
        assert_eq!(
            p.list_operations(Some("nope"), None).unwrap_err(),
            IpcError::NotFound
        );
        assert_eq!(conn.runs.load(std::sync::atomic::Ordering::SeqCst), before);
    }
}
