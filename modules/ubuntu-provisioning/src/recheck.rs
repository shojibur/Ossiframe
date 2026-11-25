//! Re-checks the material preconditions of a plan just before execution.
//! Any difference is a [`Drift`] and the plan must be made again.
//!
//! Material: OS version, the state of the planned packages, blockers
//! (including operation units and the remote lock owner), sudo mode, the
//! web server site list, package sources, and the plan's minimums for total
//! RAM and free disk. Not material: uptime, apt activity, and memory or disk
//! changes that stay above the minimum.

use crate::components;
use crate::plan::WATCH_SITES;
use crate::preflight::{self, Facts};
use ossiframe_core::{
    CancelToken, Drift, Executor, OperationError, PackageState, Plan, PreflightReport, Severity,
};

/// Blockers that classify the server by history. The Store is not available
/// here, and the plan already passed that decision.
const HISTORY_BLOCKERS: [&str; 1] = ["existing-unmanaged"];
/// Reported through the explicit minimum checks instead.
const MINIMUM_BLOCKERS: [&str; 2] = ["memory-low", "disk-low"];

fn drift(id: impl Into<String>, message: impl Into<String>) -> Drift {
    Drift {
        id: id.into(),
        message: message.into(),
    }
}

fn describe(state: &PackageState) -> String {
    match state {
        PackageState::Absent => "not installed".to_owned(),
        PackageState::Installed { version } => format!("installed ({version})"),
        PackageState::Other { status } => format!("dpkg state {status}"),
    }
}

/// Pure comparison of a fresh report against the plan's preconditions.
pub fn compare(plan: &Plan, report: &PreflightReport) -> Vec<Drift> {
    let pre = &plan.preconditions;
    let mut out = Vec::new();

    if report.os != pre.os_version {
        out.push(drift(
            "os-changed",
            format!(
                "The operating system changed from {} to {}.",
                pre.os_version, report.os
            ),
        ));
    }
    if report.sudo != pre.sudo {
        out.push(drift(
            "sudo-changed",
            format!("sudo changed from {:?} to {:?}.", pre.sudo, report.sudo),
        ));
    }
    for (name, was) in &pre.packages {
        let now = report
            .packages
            .get(name)
            .cloned()
            .unwrap_or(PackageState::Absent);
        if &now != was {
            out.push(drift(
                format!("package-changed-{name}"),
                format!(
                    "Package {name} changed from {} to {}.",
                    describe(was),
                    describe(&now)
                ),
            ));
        }
    }
    if let Some(sites) = pre.watched_listings.get(WATCH_SITES) {
        let mut now = report.sites.clone();
        now.sort();
        if &now != sites {
            out.push(drift(
                "sites-changed",
                "The web server site list changed since the plan.",
            ));
        }
    }
    let mut hosts = report.package_sources.clone();
    hosts.sort();
    hosts.dedup();
    if hosts != pre.package_sources {
        out.push(drift(
            "sources-changed",
            "The package sources changed since the plan.",
        ));
    }

    if pre.min_mem_mb > 0 {
        match report.total_mem_mb {
            Some(mb) if mb >= u64::from(pre.min_mem_mb) => {}
            Some(mb) => out.push(drift(
                "memory-below-minimum",
                format!(
                    "Total memory is {mb} MB; the plan needs at least {} MB.",
                    pre.min_mem_mb
                ),
            )),
            None => out.push(drift(
                "memory-unknown",
                "Total memory could not be read; the plan needs a minimum.",
            )),
        }
    }
    if pre.min_disk_mb > 0 {
        match report.free_disk_mb {
            Some(mb) if mb >= u64::from(pre.min_disk_mb) => {}
            Some(mb) => out.push(drift(
                "disk-below-minimum",
                format!(
                    "Free disk is {mb} MB; the plan needs at least {} MB.",
                    pre.min_disk_mb
                ),
            )),
            None => out.push(drift(
                "disk-unknown",
                "Free disk could not be read; the plan needs a minimum.",
            )),
        }
    }

    for c in &report.conflicts {
        if c.severity == Severity::Blocker
            && !HISTORY_BLOCKERS.contains(&c.id.as_str())
            && !MINIMUM_BLOCKERS.contains(&c.id.as_str())
        {
            out.push(drift(format!("blocker-{}", c.id), c.message.clone()));
        }
    }
    out
}

/// Collects fresh facts for the plan's components and compares them.
pub async fn run(exec: &dyn Executor, cancel: &CancelToken, plan: &Plan) -> Result<(), Vec<Drift>> {
    let selected = components::resolve(&plan.components)
        .map_err(|e| vec![drift("plan-invalid", e.to_string())])?;
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let facts: Facts = preflight::collect_facts(exec, cancel, preflight::rfc3339_utc(secs))
        .await
        .map_err(|e| {
            let id = match e {
                OperationError::Exec(_) => "recheck-interrupted",
                _ => "recheck-failed",
            };
            vec![drift(
                id,
                format!("The server could not be re-checked: {e}"),
            )]
        })?;
    // The Store is not available here. The plan records whether the server
    // was classed managed, and the same history is repeated for the rules
    // that depend on it (listener ownership).
    let prior: &[ossiframe_core::PriorOperation] = if plan.preconditions.server_managed {
        &[ossiframe_core::PriorOperation::Succeeded {
            all_essential_passed: true,
        }]
    } else {
        &[]
    };
    let report = preflight::evaluate(&facts, &selected, prior);
    let found = compare(plan, &report);
    if found.is_empty() { Ok(()) } else { Err(found) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::plan as build_plan;
    use crate::preflight::tests::{
        K_DF, K_DPKG, K_INV_DPKG, K_MEM, K_OS, K_PGREP, K_SITES, K_SUDO, K_UBUNTU_SRC, K_UNITS,
        UBUNTU_SOURCES, code, df, fresh, meminfo, nginx_installed, ok, os_release,
    };

    fn selection() -> Vec<String> {
        components::DEFAULT_SELECTION
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    }

    async fn plan_on(fake: &crate::preflight::tests::Fake) -> Plan {
        let r = crate::preflight::tests::report(fake, &[]).await;
        build_plan(&r, &selection()).unwrap()
    }

    async fn drift_ids(plan: &Plan, fake: &crate::preflight::tests::Fake) -> Vec<String> {
        match run(fake, &CancelToken::new(), plan).await {
            Ok(()) => Vec::new(),
            Err(d) => d.into_iter().map(|d| d.id).collect(),
        }
    }

    #[tokio::test]
    async fn unchanged_server_has_no_drift() {
        let plan = plan_on(&fresh()).await;
        assert!(drift_ids(&plan, &fresh()).await.is_empty());
    }

    #[tokio::test]
    async fn uptime_apt_activity_and_memory_above_minimum_are_not_drift() {
        let plan = plan_on(&fresh()).await;
        let fake = fresh()
            .over("/proc/uptime", ok("99999.0 5.0\n"))
            .over(K_PGREP, ok("1234 apt-get\n"))
            .over(K_MEM, meminfo(2_048_000));
        assert_eq!(drift_ids(&plan, &fake).await, Vec::<String>::new());
    }

    #[tokio::test]
    async fn lock_or_operation_unit_held_is_drift() {
        let plan = plan_on(&fresh()).await;
        let fake = fresh().over(
            K_UNITS,
            ok("ossiframe-provision-lock.service loaded active running Lock\n"),
        );
        assert!(
            drift_ids(&plan, &fake)
                .await
                .contains(&"blocker-foreign-operation".to_owned())
        );
    }

    #[tokio::test]
    async fn new_conflict_is_drift() {
        let plan = plan_on(&fresh()).await;
        let fake = fresh().over(K_INV_DPKG, ok("apache2\t2.4\tii \n"));
        assert!(!drift_ids(&plan, &fake).await.is_empty());
        let fake = fresh().over(K_SITES, ok("/etc/nginx/sites-enabled/other\n"));
        let ids = drift_ids(&plan, &fake).await;
        assert!(ids.contains(&"blocker-foreign-sites".to_owned()), "{ids:?}");
        assert!(ids.contains(&"sites-changed".to_owned()), "{ids:?}");
    }

    #[tokio::test]
    async fn memory_or_disk_below_the_minimum_is_drift() {
        let plan = plan_on(&fresh()).await;
        let ids = drift_ids(&plan, &fresh().over(K_MEM, meminfo(512_000))).await;
        assert!(ids.contains(&"memory-below-minimum".to_owned()), "{ids:?}");
        let ids = drift_ids(&plan, &fresh().over(K_DF, df(1_000_000_000))).await;
        assert!(ids.contains(&"disk-below-minimum".to_owned()), "{ids:?}");
    }

    #[tokio::test]
    async fn package_os_sudo_and_source_changes_are_drift() {
        let plan = plan_on(&fresh()).await;
        let ids = drift_ids(&plan, &fresh().over(K_DPKG, nginx_installed())).await;
        assert!(ids.contains(&"package-changed-nginx".to_owned()), "{ids:?}");

        let ids = drift_ids(&plan, &fresh().over(K_OS, os_release("ubuntu", "26.04"))).await;
        assert!(ids.contains(&"os-changed".to_owned()), "{ids:?}");

        let ids = drift_ids(
            &plan,
            &fresh().over(K_SUDO, code(1, "", "a password is required")),
        )
        .await;
        assert!(ids.contains(&"sudo-changed".to_owned()), "{ids:?}");

        let changed = UBUNTU_SOURCES.replace("security.ubuntu.com", "ports.ubuntu.com");
        let ids = drift_ids(&plan, &fresh().over(K_UBUNTU_SRC, ok(&changed))).await;
        assert!(ids.contains(&"sources-changed".to_owned()), "{ids:?}");
    }

    #[tokio::test]
    async fn managed_server_listeners_are_not_drift_but_unmanaged_ones_are() {
        use crate::preflight::tests::{ALL_ACTIVE, POST_INSTALL_SS, VERIFIED, post_install};
        let fake = post_install(POST_INSTALL_SS, &ALL_ACTIVE);
        let (_, r) = crate::preflight::tests::run_with(&fake, &["redis"], &[VERIFIED])
            .await
            .unwrap();
        let mut plan = build_plan(&r, &["redis".to_owned()]).unwrap();
        assert!(plan.preconditions.server_managed);
        assert_eq!(drift_ids(&plan, &fake).await, Vec::<String>::new());
        plan.preconditions.server_managed = false;
        let ids = drift_ids(&plan, &fake).await;
        assert!(ids.contains(&"blocker-listener-80".to_owned()), "{ids:?}");
    }
}
