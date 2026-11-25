//! Provisioning contracts: the operation state machine, the plan model and the
//! `Provisioner` trait. These types describe packages, services, steps and
//! checks only. They carry no knowledge of any specific application stack.
//!
//! Serialization uses ordered collections (`Vec`, `BTreeMap`) only, so the
//! JSON of a value is deterministic.

use crate::command::CommandSpec;
use crate::executor::{CancelToken, Executor};
use crate::module::OperationError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Operation state machine
// ---------------------------------------------------------------------------

/// Lifecycle of a mutating operation (ADR-0004).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OperationState {
    Pending,
    Running,
    Verifying,
    Succeeded,
    Failed,
    Cancelled,
    UnknownOutcome,
}

impl OperationState {
    pub const ALL: [OperationState; 7] = [
        OperationState::Pending,
        OperationState::Running,
        OperationState::Verifying,
        OperationState::Succeeded,
        OperationState::Failed,
        OperationState::Cancelled,
        OperationState::UnknownOutcome,
    ];

    /// Succeeded, Failed and Cancelled accept no further event.
    /// `UnknownOutcome` is not terminal: it needs reconciliation.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }

    /// States in which the server may not accept a new plan.
    pub fn is_unresolved(self) -> bool {
        !self.is_terminal()
    }
}

/// Facts that move an operation between states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Event {
    /// The remote lock was acquired and execution starts.
    LockAcquired,
    /// Every step finished successfully.
    StepsDone,
    /// A step finished with a failure that is known to be final.
    StepFailed,
    /// A cancel request was honoured between steps.
    CancelHonoured,
    /// Disconnect, deadline, identity mismatch, exhausted poll retries or an
    /// app crash. The remote work may still be running.
    OutcomeLost,
    /// Every selected component is verified.
    VerificationPassed,
    /// A selected component is not verified.
    VerificationFailed,
    /// Reconcile found a failed step or a step that was never launched.
    ReconcileFailed,
    /// Reconcile found evidence of completed work. Verification decides.
    ReconcileCompleted,
    /// Reconcile found that nothing was launched and cancel was requested.
    ReconcileCancelled,
    /// The user closed the operation after backend-checked inspection.
    MarkedResolved,
    /// The operation was recorded but never started: the remote lock was
    /// refused (busy or unavailable). No mutating command ran.
    StartFailed,
}

impl Event {
    pub const ALL: [Event; 12] = [
        Event::LockAcquired,
        Event::StepsDone,
        Event::StepFailed,
        Event::CancelHonoured,
        Event::OutcomeLost,
        Event::VerificationPassed,
        Event::VerificationFailed,
        Event::ReconcileFailed,
        Event::ReconcileCompleted,
        Event::ReconcileCancelled,
        Event::MarkedResolved,
        Event::StartFailed,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("illegal transition from {from:?} on {event:?}")]
pub struct IllegalTransition {
    pub from: OperationState,
    pub event: Event,
}

/// The only way an operation changes state.
///
/// `Pending -> UnknownOutcome` exists for startup recovery: the app may die
/// after the record is created and before the first step is known.
pub fn next(from: OperationState, event: Event) -> Result<OperationState, IllegalTransition> {
    use Event as E;
    use OperationState as S;
    let to = match (from, event) {
        (S::Pending, E::LockAcquired) => S::Running,
        (S::Pending, E::OutcomeLost) => S::UnknownOutcome,
        (S::Pending, E::StartFailed) => S::Failed,

        (S::Running, E::StepsDone) => S::Verifying,
        (S::Running, E::StepFailed) => S::Failed,
        (S::Running, E::CancelHonoured) => S::Cancelled,
        (S::Running, E::OutcomeLost) => S::UnknownOutcome,

        (S::Verifying, E::VerificationPassed) => S::Succeeded,
        (S::Verifying, E::VerificationFailed) => S::Failed,
        (S::Verifying, E::OutcomeLost) => S::UnknownOutcome,

        (S::UnknownOutcome, E::ReconcileFailed) => S::Failed,
        (S::UnknownOutcome, E::ReconcileCompleted) => S::Verifying,
        (S::UnknownOutcome, E::ReconcileCancelled) => S::Cancelled,
        (S::UnknownOutcome, E::MarkedResolved) => S::Failed,

        _ => return Err(IllegalTransition { from, event }),
    };
    Ok(to)
}

// ---------------------------------------------------------------------------
// Plan model
// ---------------------------------------------------------------------------

/// What the local Store knows about an earlier operation for the *same
/// identity snapshot* (endpoint and pinned host key). The caller filters by
/// identity; the provisioner only interprets the outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriorOperation {
    /// `all_essential_passed` is true when verification had no failed
    /// essential check.
    Succeeded {
        all_essential_passed: bool,
    },
    Failed,
    Cancelled,
    ClosedManually,
}

/// Identity snapshot of the server a plan was made for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanTarget {
    pub server_id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    /// SHA-256 of the pinned host key.
    pub host_key_sha256: String,
    /// Application the plan belongs to (S05). Omitted from the JSON when
    /// `None`, so S04 plan digests do not change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageChange {
    pub name: String,
    pub installed_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    pub path: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceEffect {
    pub unit: String,
    pub effect: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resources {
    pub disk_mb_needed: u32,
    pub mem_mb_min: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StepMode {
    /// Runs over the open channel. For short commands only.
    Foreground,
    /// Runs as a transient unit on the server and survives SSH loss.
    Durable,
    /// A read-only check run by `Provisioner::gate`.
    Gate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanStep {
    pub id: String,
    pub title: String,
    pub command: CommandSpec,
    pub mode: StepMode,
    pub deadline: Duration,
    /// The command needs stdin, which is never stored. The engine runs such
    /// a step directly and refuses it if the stdin is missing (a plan
    /// reloaded from storage).
    #[serde(default)]
    pub requires_stdin: bool,
}

/// Material facts that are re-checked before execution starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preconditions {
    pub os_version: String,
    pub packages: BTreeMap<String, PackageState>,
    pub sudo: SudoMode,
    pub package_sources: Vec<String>,
    /// Named directory listings the plan depends on (name -> sorted entries).
    pub watched_listings: BTreeMap<String, Vec<String>>,
    pub min_mem_mb: u32,
    pub min_disk_mb: u32,
    /// The preflight classed the server OssiframeManaged when the plan was
    /// made (a verified earlier Ossiframe operation). The re-check has no
    /// access to the Store and repeats that history so listener ownership is
    /// judged the same way. Absent in plans stored before this field: false.
    #[serde(default)]
    pub server_managed: bool,
}

/// What will be done, shown to the user before approval. The digest of its
/// JSON is what the user approves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub target: PlanTarget,
    pub os: String,
    /// Resolved order: requested components and their dependencies.
    pub components: Vec<String>,
    pub already_present: Vec<String>,
    pub packages: Vec<PackageChange>,
    pub files: Vec<FileChange>,
    pub services: Vec<ServiceEffect>,
    pub resources: Resources,
    pub privileges: Vec<String>,
    /// Recovery limits shown before approval.
    pub limitations: Vec<String>,
    pub steps: Vec<PlanStep>,
    pub preconditions: Preconditions,
}

// ---------------------------------------------------------------------------
// Preflight
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ServerClass {
    Fresh,
    ExistingUnmanaged,
    OssiframeManaged,
    Partial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SudoMode {
    Passwordless,
    PasswordRequired,
    NotPermitted,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    Blocker,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Conflict {
    pub id: String,
    pub severity: Severity,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum PackageState {
    Absent,
    Installed {
        version: String,
    },
    /// Any dpkg state other than absent or installed (for example half-configured).
    Other {
        status: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightReport {
    /// RFC 3339.
    pub observed_at: String,
    pub os: String,
    pub sudo: SudoMode,
    pub server_class: ServerClass,
    pub packages: BTreeMap<String, PackageState>,
    pub total_mem_mb: Option<u64>,
    pub free_disk_mb: Option<u64>,
    /// Sorted, deduplicated hosts of the enabled package sources.
    pub package_sources: Vec<String>,
    /// Sorted entries of the web server `sites-enabled` directories.
    pub sites: Vec<String>,
    /// Installation candidate version of each component package that has one,
    /// from the server's current APT package lists (which may be stale until
    /// `apt-get update` runs). Information only.
    pub candidate_versions: BTreeMap<String, String>,
    pub conflicts: Vec<Conflict>,
}

impl PreflightReport {
    /// False when any conflict is a `Blocker`. Warnings do not block.
    pub fn can_provision(&self) -> bool {
        !self
            .conflicts
            .iter()
            .any(|c| c.severity == Severity::Blocker)
    }
}

// ---------------------------------------------------------------------------
// Verification and drift
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CheckStatus {
    Pass,
    Fail,
    Warn,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckResult {
    pub component: String,
    pub id: String,
    pub title: String,
    /// An essential check must pass for the component to count as verified.
    pub essential: bool,
    pub status: CheckStatus,
    /// Plain text, short.
    pub detail: String,
}

/// What the real state of an application says about an interrupted
/// operation. Evidence for the user only: it never resumes or repairs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AppRecoveryVerdict {
    /// The expected release is live and the health checks pass.
    Deployed,
    /// `current` is still on the previous release.
    NotSwitched,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppRecovery {
    pub verdict: AppRecoveryVerdict,
    /// Plain text, short: what was observed.
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentVerdict {
    pub component: String,
    pub verified: bool,
}

impl ComponentVerdict {
    /// Verified only when the component has at least one essential check and
    /// every essential check is `Pass`. A `Fail` or `Skipped` essential check
    /// is not verified. Missing evidence is never read as success. Checks of
    /// other components and non-essential checks are ignored.
    pub fn from_checks(component: &str, checks: &[CheckResult]) -> Self {
        let mut essential = checks
            .iter()
            .filter(|c| c.component == component && c.essential)
            .peekable();
        let verified =
            essential.peek().is_some() && essential.all(|c| c.status == CheckStatus::Pass);
        Self {
            component: component.to_owned(),
            verified,
        }
    }
}

/// A material change found between planning and execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Drift {
    pub id: String,
    pub message: String,
}

// ---------------------------------------------------------------------------
// Provisioner
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentInfo {
    pub id: String,
    pub title: String,
    pub description: String,
    pub requires: Vec<String>,
    /// Packages that make up the component, for showing installed state.
    pub packages: Vec<String>,
    pub optional: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error("unknown component: {0}")]
    UnknownComponent(String),
    /// The preflight report has blockers.
    #[error("preflight has blockers; no plan can be made")]
    Blocked,
    /// A component rule refused the selection (for example too little memory).
    #[error("{0}")]
    Refused(String),
}

/// Implemented by a module that can plan, gate and verify a provisioning
/// operation. The engine owns approval, execution and recovery.
#[async_trait]
pub trait Provisioner: Send + Sync {
    fn components(&self) -> Vec<ComponentInfo>;

    /// Read-only facts, except for one fixed privilege probe.
    async fn preflight(
        &self,
        exec: &dyn Executor,
        cancel: &CancelToken,
    ) -> Result<PreflightReport, OperationError>;

    /// Like [`Provisioner::preflight`], for the user's selection and with
    /// the Store's earlier operations for the same identity snapshot. The
    /// desktop commands use this one. Unknown component ids are an error.
    async fn preflight_for(
        &self,
        exec: &dyn Executor,
        cancel: &CancelToken,
        selection: &[String],
        prior: &[PriorOperation],
    ) -> Result<PreflightReport, OperationError>;

    /// Pure and deterministic for the same report and selection.
    fn plan(&self, report: &PreflightReport, selection: &[String]) -> Result<Plan, PlanError>;

    /// Re-checks material facts before execution. `Err` lists the drift.
    async fn recheck(
        &self,
        exec: &dyn Executor,
        cancel: &CancelToken,
        plan: &Plan,
    ) -> Result<(), Vec<Drift>>;

    /// Runs a read-only `StepMode::Gate` step. `Ok` is bounded evidence text,
    /// `Err` is a bounded, actionable failure text.
    async fn gate(
        &self,
        exec: &dyn Executor,
        cancel: &CancelToken,
        plan: &Plan,
        step_id: &str,
    ) -> Result<String, String>;

    async fn verify(
        &self,
        exec: &dyn Executor,
        cancel: &CancelToken,
        plan: &Plan,
    ) -> Vec<CheckResult>;

    /// Read-only evidence about the real state of the application behind a
    /// plan whose target has an `app_id`, used by reconcile and "mark
    /// resolved". `None` (the default) means this module has none.
    async fn app_recovery(
        &self,
        _exec: &dyn Executor,
        _cancel: &CancelToken,
        _plan: &Plan,
    ) -> Option<AppRecovery> {
        None
    }
}

/// `dpkg-query -W -f` format for the whole-system dpkg health probe. It runs
/// without root, unlike `dpkg --audit`.
pub const DPKG_HEALTH_FORMAT: &str = "${db:Status-Abbrev}\\t${Package}\\n";

/// Derives dpkg health from the complete `dpkg-query -W` listing written with
/// [`DPKG_HEALTH_FORMAT`]. `Ok(true)` means every package state is settled:
/// the status column is installed (`i`), not-installed (`n`) or config-files
/// (`c`) and the error flag is clear. `Ok(false)` means at least one package
/// is half-installed (`H`), unpacked (`U`), half-configured (`F`),
/// triggers-awaited (`W`), triggers-pending (`t`) or flagged reinst-required
/// (`R`). `Err` for empty output or any line that is not a 3-character status
/// plus a tab plus a name: unreadable evidence is never a clean result.
///
/// Limitation: this does not cover the missing-metadata checks of
/// `dpkg --audit`, which needs root.
pub fn dpkg_states_settled(stdout: &str) -> Result<bool, String> {
    let mut settled = true;
    let mut lines = 0usize;
    for line in stdout.lines().filter(|l| !l.is_empty()) {
        let bad_line = || "unrecognized dpkg-query line".to_owned();
        let (abbrev, name) = line.split_once('\t').ok_or_else(bad_line)?;
        let a: Vec<char> = abbrev.chars().collect();
        let [_desired, status, error] = a[..] else {
            return Err(bad_line());
        };
        if name.trim().is_empty() {
            return Err(bad_line());
        }
        match status {
            'i' | 'n' | 'c' => {}
            'H' | 'U' | 'F' | 'W' | 't' => settled = false,
            _ => return Err(bad_line()),
        }
        match error {
            ' ' => {}
            'R' => settled = false,
            _ => return Err(bad_line()),
        }
        lines += 1;
    }
    if lines == 0 {
        return Err("dpkg-query listed no packages".to_owned());
    }
    Ok(settled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    const ALLOWED: [(OperationState, Event, OperationState); 14] = [
        (
            OperationState::Pending,
            Event::StartFailed,
            OperationState::Failed,
        ),
        (
            OperationState::Pending,
            Event::LockAcquired,
            OperationState::Running,
        ),
        (
            OperationState::Pending,
            Event::OutcomeLost,
            OperationState::UnknownOutcome,
        ),
        (
            OperationState::Running,
            Event::StepsDone,
            OperationState::Verifying,
        ),
        (
            OperationState::Running,
            Event::StepFailed,
            OperationState::Failed,
        ),
        (
            OperationState::Running,
            Event::CancelHonoured,
            OperationState::Cancelled,
        ),
        (
            OperationState::Running,
            Event::OutcomeLost,
            OperationState::UnknownOutcome,
        ),
        (
            OperationState::Verifying,
            Event::VerificationPassed,
            OperationState::Succeeded,
        ),
        (
            OperationState::Verifying,
            Event::VerificationFailed,
            OperationState::Failed,
        ),
        (
            OperationState::Verifying,
            Event::OutcomeLost,
            OperationState::UnknownOutcome,
        ),
        (
            OperationState::UnknownOutcome,
            Event::ReconcileFailed,
            OperationState::Failed,
        ),
        (
            OperationState::UnknownOutcome,
            Event::ReconcileCompleted,
            OperationState::Verifying,
        ),
        (
            OperationState::UnknownOutcome,
            Event::ReconcileCancelled,
            OperationState::Cancelled,
        ),
        (
            OperationState::UnknownOutcome,
            Event::MarkedResolved,
            OperationState::Failed,
        ),
    ];

    #[test]
    fn allowed_transitions_pass_and_all_others_are_illegal() {
        let allowed: HashSet<_> = ALLOWED.iter().map(|(s, e, _)| (*s, *e)).collect();
        for (from, event, to) in ALLOWED {
            assert_eq!(next(from, event), Ok(to));
        }
        for from in OperationState::ALL {
            for event in Event::ALL {
                if !allowed.contains(&(from, event)) {
                    assert_eq!(next(from, event), Err(IllegalTransition { from, event }));
                }
            }
        }
    }

    #[test]
    fn terminal_states_accept_no_event() {
        for from in OperationState::ALL.into_iter().filter(|s| s.is_terminal()) {
            for event in Event::ALL {
                assert!(next(from, event).is_err(), "{from:?} {event:?}");
            }
        }
        assert!(!OperationState::UnknownOutcome.is_terminal());
        assert!(OperationState::UnknownOutcome.is_unresolved());
    }

    #[test]
    fn lost_outcome_never_becomes_failed_directly() {
        for from in [OperationState::Running, OperationState::Verifying] {
            assert_eq!(
                next(from, Event::OutcomeLost),
                Ok(OperationState::UnknownOutcome)
            );
        }
        // Step success never implies plan success: only verification does.
        assert_eq!(
            next(OperationState::Running, Event::StepsDone),
            Ok(OperationState::Verifying)
        );
        assert!(next(OperationState::Running, Event::VerificationPassed).is_err());
    }

    fn report(conflicts: Vec<Conflict>) -> PreflightReport {
        PreflightReport {
            observed_at: "2025-01-01T00:00:00Z".into(),
            os: "ubuntu 24.04".into(),
            sudo: SudoMode::Passwordless,
            server_class: ServerClass::Fresh,
            packages: BTreeMap::new(),
            total_mem_mb: Some(2048),
            free_disk_mb: Some(10_000),
            package_sources: Vec::new(),
            candidate_versions: BTreeMap::new(),
            sites: Vec::new(),
            conflicts,
        }
    }

    fn conflict(severity: Severity) -> Conflict {
        Conflict {
            id: "c".into(),
            severity,
            message: "m".into(),
        }
    }

    #[test]
    fn can_provision_is_false_only_with_a_blocker() {
        assert!(report(vec![]).can_provision());
        assert!(report(vec![conflict(Severity::Warning)]).can_provision());
        assert!(
            !report(vec![
                conflict(Severity::Warning),
                conflict(Severity::Blocker)
            ])
            .can_provision()
        );
    }

    fn plan() -> Plan {
        let mut packages = BTreeMap::new();
        packages.insert("b".to_owned(), PackageState::Absent);
        packages.insert(
            "a".to_owned(),
            PackageState::Installed {
                version: "1".into(),
            },
        );
        Plan {
            target: PlanTarget {
                server_id: "id".into(),
                name: "n".into(),
                host: "h".into(),
                port: 22,
                user: "u".into(),
                host_key_sha256: "k".into(),
                app_id: None,
            },
            os: "ubuntu 24.04".into(),
            components: vec!["x".into()],
            already_present: vec![],
            packages: vec![PackageChange {
                name: "p".into(),
                installed_version: None,
            }],
            files: vec![],
            services: vec![],
            resources: Resources {
                disk_mb_needed: 1,
                mem_mb_min: 1,
            },
            privileges: vec!["root via sudo -n".into()],
            limitations: vec![],
            steps: vec![PlanStep {
                id: "s".into(),
                title: "t".into(),
                command: CommandSpec::privileged("/usr/bin/true", &[], Duration::from_secs(1))
                    .unwrap(),
                mode: StepMode::Durable,
                deadline: Duration::from_secs(60),
                requires_stdin: false,
            }],
            preconditions: Preconditions {
                os_version: "24.04".into(),
                packages,
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
    fn plan_serializes_identically_twice_and_changes_with_content() {
        let a = serde_json::to_string(&plan()).unwrap();
        assert_eq!(a, serde_json::to_string(&plan()).unwrap());
        let mut changed = plan();
        changed.packages[0].name = "q".into();
        assert_ne!(a, serde_json::to_string(&changed).unwrap());
    }

    #[test]
    fn plan_target_without_app_id_deserializes_and_serializes_unchanged() {
        let json =
            r#"{"serverId":"s","name":"n","host":"h","port":22,"user":"u","hostKeySha256":"k"}"#;
        let target: PlanTarget = serde_json::from_str(json).unwrap();
        assert_eq!(target.app_id, None);
        assert_eq!(serde_json::to_string(&target).unwrap(), json);
    }

    #[test]
    fn plan_round_trips_through_json_with_the_same_digest_input() {
        let json = serde_json::to_string(&plan()).unwrap();
        let back: Plan = serde_json::from_str(&json).unwrap();
        assert_eq!(back, plan());
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
    }

    #[test]
    fn dpkg_state_derivation() {
        let ok = |s: &str| dpkg_states_settled(s);
        assert_eq!(ok("ii \tnginx\nun \tfoo\nrc \tbar\nhi \tbaz\n"), Ok(true));
        for bad in ["iH ", "iU ", "iF ", "iW ", "it ", "iiR", "riR"] {
            let line = format!("{bad}\tpkg\nii \tok\n");
            assert_eq!(ok(&line), Ok(false), "{bad}");
        }
        // Unreadable evidence is an error, never clean.
        for junk in [
            "",
            "\n",
            "ii\tnginx\n",
            "ii nginx\n",
            "iz \tx\n",
            "iiX\tx\n",
            "ii \t\n",
        ] {
            assert!(ok(junk).is_err(), "{junk:?}");
        }
    }

    fn check(component: &str, essential: bool, status: CheckStatus) -> CheckResult {
        CheckResult {
            component: component.into(),
            id: "i".into(),
            title: "t".into(),
            essential,
            status,
            detail: String::new(),
        }
    }

    #[test]
    fn verdict_requires_every_essential_check_to_pass() {
        use CheckStatus::*;
        let v = |checks: &[CheckResult]| ComponentVerdict::from_checks("a", checks).verified;
        assert!(v(&[check("a", true, Pass), check("a", false, Warn)]));
        assert!(!v(&[check("a", true, Pass), check("a", true, Fail)]));
        assert!(!v(&[check("a", true, Skipped)]));
        // Another component's pass does not count, and no evidence is not success.
        assert!(!v(&[check("b", true, Pass)]));
        assert!(!v(&[]));
    }
}
