//! In-memory plan store, plan digest and the `ApprovedPlan` type.
//!
//! The backend keeps the plan it showed to the user. Approval submits only
//! a plan id and the digest of that plan. The digest binds the user's
//! consent to the exact plan content and detects a plan that was altered
//! after storage. It does not protect against a compromised frontend: the
//! boundary there is that only the stored plan is ever executed.
//!
//! `ApprovedPlan` has private fields and no public constructor. The only
//! way to get one is [`PlanStore::take_approved`], which runs every
//! approval check.
use crate::ipc::PlanReason;
use ossiframe_core::module::ModuleId;
use ossiframe_core::provision::Plan;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const PLAN_TTL: Duration = Duration::from_secs(10 * 60);

/// Canonical JSON of a plan. `Plan` uses ordered collections only, so equal
/// plans give equal bytes.
pub fn canonical_json(plan: &Plan) -> String {
    // `Plan` holds only strings, numbers and ordered collections, so
    // serialization cannot fail.
    serde_json::to_string(plan).expect("plan serializes")
}

fn digest_of(json: &str) -> String {
    let hash = Sha256::digest(json.as_bytes());
    let mut out = String::with_capacity(64);
    for b in hash.iter() {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Lowercase hex SHA-256 of the plan's canonical JSON.
pub fn plan_digest(plan: &Plan) -> String {
    digest_of(&canonical_json(plan))
}

/// Re-verifies a plan read back from SQLite: the stored JSON text must hash
/// to the stored digest.
pub fn verify_stored_plan(plan_json: &str, stored_digest: &str) -> bool {
    is_digest(stored_digest) && digest_of(plan_json) == stored_digest
}

// --- input validators -------------------------------------------------------

pub fn is_uuid(s: &str) -> bool {
    let groups: Vec<&str> = s.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(g, n)| g.len() == n && g.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Component ids follow the module id rules.
pub fn is_component_id(s: &str) -> bool {
    ModuleId::parse(s).is_ok()
}

/// 64 lowercase hex characters.
pub fn is_digest(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

// --- store ------------------------------------------------------------------

/// What the user is shown after planning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanHandle {
    pub id: String,
    pub digest: String,
    pub expires_in: Duration,
}

/// Current facts about the server, supplied by the caller at approval time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerIdentity {
    pub host: String,
    pub port: u16,
    pub user: String,
    /// SHA-256 of the currently pinned host key.
    pub host_key_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalContext {
    pub identity: ServerIdentity,
    pub connected: bool,
    /// An operation is active or in UnknownOutcome for this server.
    pub busy: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ApprovalError {
    #[error("no plan exists for this server")]
    UnknownPlan,
    #[error("a newer plan replaced this one")]
    Superseded,
    #[error("the plan expired")]
    Expired,
    #[error("the plan digest does not match")]
    Mismatch,
    #[error("the server or its pinned host key changed since planning")]
    WrongServer,
    #[error("the server is not connected")]
    NotConnected,
    #[error("another operation is running or needs review on this server")]
    Busy,
}

impl ApprovalError {
    /// The IPC reason, or `None` when the error maps to `notConnected`.
    pub fn reason(self) -> Option<PlanReason> {
        match self {
            Self::UnknownPlan => Some(PlanReason::UnknownPlan),
            Self::Superseded => Some(PlanReason::Superseded),
            Self::Expired => Some(PlanReason::Expired),
            Self::Mismatch => Some(PlanReason::Mismatch),
            Self::WrongServer => Some(PlanReason::WrongServer),
            Self::Busy => Some(PlanReason::Busy),
            Self::NotConnected => None,
        }
    }
}

struct StoredPlan {
    id: String,
    plan: Plan,
    json: String,
    digest: String,
    expires: Instant,
}

/// A plan that passed every approval check. It cannot be built elsewhere.
#[derive(Debug)]
pub struct ApprovedPlan {
    plan_id: String,
    digest: String,
    plan: Plan,
}

impl ApprovedPlan {
    pub fn plan_id(&self) -> &str {
        &self.plan_id
    }
    pub fn digest(&self) -> &str {
        &self.digest
    }
    pub fn plan(&self) -> &Plan {
        &self.plan
    }
}

/// At most one plan per server. Storing a new plan supersedes the old one.
pub struct PlanStore {
    ttl: Duration,
    plans: Mutex<HashMap<String, StoredPlan>>,
}

impl Default for PlanStore {
    fn default() -> Self {
        Self::new(PLAN_TTL)
    }
}

impl PlanStore {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            plans: Mutex::new(HashMap::new()),
        }
    }

    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<String, StoredPlan>> {
        self.plans.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Stores the plan under its target server id and returns its handle.
    pub fn insert(&self, plan: Plan) -> PlanHandle {
        self.insert_at(plan, Instant::now())
    }

    pub(crate) fn insert_at(&self, plan: Plan, now: Instant) -> PlanHandle {
        let json = canonical_json(&plan);
        let digest = digest_of(&json);
        let id = uuid::Uuid::new_v4().to_string();
        let server_id = plan.target.server_id.clone();
        let mut map = self.map();
        map.retain(|_, p| p.expires > now);
        map.insert(
            server_id,
            StoredPlan {
                id: id.clone(),
                plan,
                json,
                digest: digest.clone(),
                expires: now + self.ttl,
            },
        );
        PlanHandle {
            id,
            digest,
            expires_in: self.ttl,
        }
    }

    /// Runs the approval checks in order and consumes the plan.
    pub fn take_approved(
        &self,
        server_id: &str,
        plan_id: &str,
        submitted_digest: &str,
        ctx: &ApprovalContext,
    ) -> Result<ApprovedPlan, ApprovalError> {
        self.take_approved_at(server_id, plan_id, submitted_digest, ctx, Instant::now())
    }

    pub(crate) fn take_approved_at(
        &self,
        server_id: &str,
        plan_id: &str,
        submitted_digest: &str,
        ctx: &ApprovalContext,
        now: Instant,
    ) -> Result<ApprovedPlan, ApprovalError> {
        let mut map = self.map();
        let stored = map.get(server_id).ok_or(ApprovalError::UnknownPlan)?;
        // A stale request must not destroy the newer plan.
        if stored.id != plan_id {
            return Err(ApprovalError::Superseded);
        }
        // From here the plan is consumed, whatever the outcome.
        let stored = map.remove(server_id).ok_or(ApprovalError::UnknownPlan)?;
        if stored.expires <= now {
            return Err(ApprovalError::Expired);
        }
        if submitted_digest != stored.digest
            || digest_of(&canonical_json(&stored.plan)) != stored.digest
            || digest_of(&stored.json) != stored.digest
        {
            return Err(ApprovalError::Mismatch);
        }
        let t = &stored.plan.target;
        let id = &ctx.identity;
        if t.server_id != server_id
            || t.host != id.host
            || t.port != id.port
            || t.user != id.user
            || t.host_key_sha256 != id.host_key_sha256
        {
            return Err(ApprovalError::WrongServer);
        }
        if !ctx.connected {
            return Err(ApprovalError::NotConnected);
        }
        if ctx.busy {
            return Err(ApprovalError::Busy);
        }
        Ok(ApprovedPlan {
            plan_id: stored.id,
            digest: stored.digest,
            plan: stored.plan,
        })
    }

    /// Drops the plan of a server, for example when the server is removed.
    pub fn discard(&self, server_id: &str) {
        self.map().remove(server_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ossiframe_core::command::CommandSpec;
    use ossiframe_core::provision::*;
    use std::collections::BTreeMap;

    fn plan_for(server: &str, pkg: &str) -> Plan {
        Plan {
            target: PlanTarget {
                server_id: server.into(),
                name: "n".into(),
                host: "example.com".into(),
                port: 22,
                user: "deploy".into(),
                host_key_sha256: "k1".into(),
                app_id: None,
            },
            os: "ubuntu 24.04".into(),
            components: vec!["web".into()],
            already_present: vec![],
            packages: vec![PackageChange {
                name: pkg.into(),
                installed_version: None,
            }],
            files: vec![],
            services: vec![],
            resources: Resources {
                disk_mb_needed: 1,
                mem_mb_min: 1,
            },
            privileges: vec![],
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

    fn ctx() -> ApprovalContext {
        ApprovalContext {
            identity: ServerIdentity {
                host: "example.com".into(),
                port: 22,
                user: "deploy".into(),
                host_key_sha256: "k1".into(),
            },
            connected: true,
            busy: false,
        }
    }

    #[test]
    fn digest_is_deterministic_and_changes_with_content() {
        let a = plan_digest(&plan_for("s1", "nginx"));
        assert!(is_digest(&a));
        assert_eq!(a, plan_digest(&plan_for("s1", "nginx")));
        assert_ne!(a, plan_digest(&plan_for("s1", "nginx-extras")));
    }

    #[test]
    fn approval_succeeds_once_then_the_plan_is_gone() {
        let store = PlanStore::default();
        let h = store.insert(plan_for("s1", "nginx"));
        let ok = store.take_approved("s1", &h.id, &h.digest, &ctx()).unwrap();
        assert_eq!(ok.plan_id(), h.id);
        assert_eq!(ok.digest(), h.digest);
        assert_eq!(
            store
                .take_approved("s1", &h.id, &h.digest, &ctx())
                .unwrap_err(),
            ApprovalError::UnknownPlan
        );
    }

    #[test]
    fn unknown_plan_for_server_without_plan() {
        let store = PlanStore::default();
        let err = store.take_approved("s1", "x", &"0".repeat(64), &ctx());
        assert_eq!(err.unwrap_err(), ApprovalError::UnknownPlan);
    }

    #[test]
    fn newer_plan_supersedes_and_stale_request_keeps_the_new_plan() {
        let store = PlanStore::default();
        let old = store.insert(plan_for("s1", "nginx"));
        let new = store.insert(plan_for("s1", "nginx-extras"));
        assert_eq!(
            store
                .take_approved("s1", &old.id, &old.digest, &ctx())
                .unwrap_err(),
            ApprovalError::Superseded
        );
        assert!(
            store
                .take_approved("s1", &new.id, &new.digest, &ctx())
                .is_ok()
        );
    }

    #[test]
    fn plans_are_kept_per_server() {
        let store = PlanStore::default();
        let a = store.insert(plan_for("s1", "nginx"));
        let _b = store.insert(plan_for("s2", "nginx"));
        assert!(store.take_approved("s1", &a.id, &a.digest, &ctx()).is_ok());
    }

    #[test]
    fn expired_plan_is_refused_and_removed() {
        let store = PlanStore::new(Duration::from_secs(600));
        let t0 = Instant::now();
        let h = store.insert_at(plan_for("s1", "nginx"), t0);
        let late = t0 + Duration::from_secs(600);
        assert_eq!(
            store
                .take_approved_at("s1", &h.id, &h.digest, &ctx(), late)
                .unwrap_err(),
            ApprovalError::Expired
        );
        assert_eq!(
            store
                .take_approved_at("s1", &h.id, &h.digest, &ctx(), t0)
                .unwrap_err(),
            ApprovalError::UnknownPlan
        );
        let h2 = store.insert_at(plan_for("s1", "nginx"), t0);
        let just_before = t0 + Duration::from_secs(599);
        assert!(
            store
                .take_approved_at("s1", &h2.id, &h2.digest, &ctx(), just_before)
                .is_ok()
        );
    }

    #[test]
    fn wrong_digest_is_a_mismatch_and_consumes_the_plan() {
        let store = PlanStore::default();
        let h = store.insert(plan_for("s1", "nginx"));
        assert_eq!(
            store
                .take_approved("s1", &h.id, &"a".repeat(64), &ctx())
                .unwrap_err(),
            ApprovalError::Mismatch
        );
        assert_eq!(
            store
                .take_approved("s1", &h.id, &h.digest, &ctx())
                .unwrap_err(),
            ApprovalError::UnknownPlan
        );
    }

    #[test]
    fn changed_endpoint_or_pinned_key_is_wrong_server() {
        let cases: [fn(&mut ApprovalContext); 4] = [
            |c| c.identity.host = "other.example".into(),
            |c| c.identity.port = 2222,
            |c| c.identity.user = "root".into(),
            |c| c.identity.host_key_sha256 = "k2".into(),
        ];
        for change in cases {
            let store = PlanStore::default();
            let h = store.insert(plan_for("s1", "nginx"));
            let mut c = ctx();
            change(&mut c);
            assert_eq!(
                store.take_approved("s1", &h.id, &h.digest, &c).unwrap_err(),
                ApprovalError::WrongServer
            );
        }
    }

    #[test]
    fn disconnected_and_busy_servers_are_refused() {
        let store = PlanStore::default();
        let h = store.insert(plan_for("s1", "nginx"));
        let mut c = ctx();
        c.connected = false;
        assert_eq!(
            store.take_approved("s1", &h.id, &h.digest, &c).unwrap_err(),
            ApprovalError::NotConnected
        );
        let h = store.insert(plan_for("s1", "nginx"));
        let mut c = ctx();
        c.busy = true;
        assert_eq!(
            store.take_approved("s1", &h.id, &h.digest, &c).unwrap_err(),
            ApprovalError::Busy
        );
    }

    #[test]
    fn tampered_stored_plan_is_detected() {
        let plan = plan_for("s1", "nginx");
        let json = canonical_json(&plan);
        let digest = plan_digest(&plan);
        assert!(verify_stored_plan(&json, &digest));
        assert!(!verify_stored_plan(&json.replace("nginx", "evil"), &digest));
        assert!(!verify_stored_plan(&json, "not-a-digest"));
    }

    #[test]
    fn validators_reject_malformed_input() {
        assert!(is_uuid(&uuid::Uuid::new_v4().to_string()));
        for bad in ["", "abc", "g0000000-0000-0000-0000-000000000000", "../x"] {
            assert!(!is_uuid(bad), "{bad:?}");
        }
        assert!(is_component_id("php-fpm"));
        for bad in ["", "Web", "a b", "a;b", "../x", "-a"] {
            assert!(!is_component_id(bad), "{bad:?}");
        }
        assert!(is_digest(&"0f".repeat(32)));
        for bad in [
            String::new(),
            "A".repeat(64),
            "0".repeat(63),
            "0".repeat(65),
            "g".repeat(64),
        ] {
            assert!(!is_digest(&bad), "{bad:?}");
        }
    }
}
