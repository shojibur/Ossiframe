//! Plan builder. Pure and synchronous: the same report and selection always
//! give the same plan, so the same digest input.
//!
//! The plan's `target` (server identity snapshot) is not known here. It is a
//! placeholder that the engine overwrites before the plan is stored and
//! digested.

use crate::components::{self, ComponentDef};
use ossiframe_core::{
    CommandSpec, PackageChange, PackageState, Plan, PlanError, PlanStep, PlanTarget, Preconditions,
    PreflightReport, Resources, ServerClass, ServiceEffect, StepMode,
};
use std::collections::BTreeMap;
use std::time::Duration;

pub const STEP_APT_UPDATE: &str = "apt-update";
pub const STEP_CANDIDATE_CHECK: &str = "candidate-check";
pub const STEP_APT_INSTALL: &str = "apt-install";

/// Key of the `watched_listings` entry that holds the `sites-enabled` list.
pub const WATCH_SITES: &str = "sites-enabled";

/// Free disk kept in reserve on top of the component estimates. Equal to the
/// preflight rule, so a plan that passes preflight satisfies its own minimum.
pub const DISK_HEADROOM_MB: u32 = 1024;

const UPDATE_DEADLINE: Duration = Duration::from_secs(5 * 60);
const GATE_DEADLINE: Duration = Duration::from_secs(2 * 60);
const INSTALL_DEADLINE: Duration = Duration::from_secs(20 * 60);

const APT_GET: &str = "/usr/bin/apt-get";
const ENV: &str = "/usr/bin/env";

/// Arguments shared by the simulated and the real install, in this order.
pub const INSTALL_FLAGS: [&str; 2] = ["-y", "--no-install-recommends"];

fn refuse(e: impl std::fmt::Display) -> PlanError {
    PlanError::Refused(e.to_string())
}

fn limitations() -> Vec<String> {
    [
        "There is no rollback. A failed install can leave some packages installed; \
         the recovery is to make a new plan, which skips packages that are already installed.",
        "Services start automatically when their packages are installed, and package \
         scripts may restart other services.",
        "APT may install additional dependency packages and run their maintainer scripts. \
         The exact set is unknown until the package lists are updated; the candidate check \
         lists it before anything is installed.",
        "No firewall change is made. The default NGINX page is reachable on port 80 if the \
         port is open.",
        "Ossiframe never runs dpkg --configure -a, removes packages, or terminates apt or dpkg.",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect()
}

fn placeholder_target() -> PlanTarget {
    PlanTarget {
        server_id: String::new(),
        name: String::new(),
        host: String::new(),
        port: 0,
        user: String::new(),
        host_key_sha256: String::new(),
        app_id: None,
    }
}

/// The argv of `apt-get install`, without the program, for `packages`.
fn install_args(packages: &[String]) -> Vec<&str> {
    let mut args: Vec<&str> = vec!["install"];
    args.extend(INSTALL_FLAGS);
    args.extend(packages.iter().map(String::as_str));
    args
}

fn steps(missing: &[String]) -> Result<Vec<PlanStep>, PlanError> {
    if missing.is_empty() {
        return Ok(Vec::new());
    }
    let update = CommandSpec::privileged(APT_GET, &["update"], UPDATE_DEADLINE).map_err(refuse)?;

    // Read-only simulation. The gate runs it and the origin checks.
    let mut sim_args: Vec<&str> = vec!["-s"];
    sim_args.extend(install_args(missing));
    let sim = CommandSpec::new(APT_GET, &sim_args, GATE_DEADLINE).map_err(refuse)?;

    let mut install_argv: Vec<&str> = vec![
        "DEBIAN_FRONTEND=noninteractive",
        "NEEDRESTART_SUSPEND=1",
        APT_GET,
    ];
    install_argv.extend(install_args(missing));
    // After "install" and the shared flags, before the package names.
    let at = 3 + 1 + INSTALL_FLAGS.len();
    install_argv.splice(at..at, ["-o", "DPkg::Lock::Timeout=300"]);
    let install = CommandSpec::privileged(ENV, &install_argv, INSTALL_DEADLINE).map_err(refuse)?;

    Ok(vec![
        PlanStep {
            id: STEP_APT_UPDATE.to_owned(),
            title: "Update the package lists".to_owned(),
            command: update,
            mode: StepMode::Durable,
            deadline: UPDATE_DEADLINE,
            requires_stdin: false,
        },
        PlanStep {
            id: STEP_CANDIDATE_CHECK.to_owned(),
            title: "Check package candidates and origins".to_owned(),
            command: sim,
            mode: StepMode::Gate,
            deadline: GATE_DEADLINE,
            requires_stdin: false,
        },
        PlanStep {
            id: STEP_APT_INSTALL.to_owned(),
            title: "Install the packages".to_owned(),
            command: install,
            mode: StepMode::Durable,
            deadline: INSTALL_DEADLINE,
            requires_stdin: false,
        },
    ])
}

fn state_of(report: &PreflightReport, pkg: &str) -> PackageState {
    report
        .packages
        .get(pkg)
        .cloned()
        .unwrap_or(PackageState::Absent)
}

/// Builds the plan. `Blocked` when the report has a blocker, `Refused` for a
/// component rule (too little memory) or a package in an unusable dpkg state.
pub fn plan(report: &PreflightReport, selection: &[String]) -> Result<Plan, PlanError> {
    let selected: Vec<&'static ComponentDef> = components::resolve(selection)?;
    if !report.can_provision() {
        return Err(PlanError::Blocked);
    }

    let mut mem_mb_min: u32 = 0;
    for def in &selected {
        if def.min_mem_mb == 0 {
            continue;
        }
        match report.total_mem_mb {
            Some(mb) if mb >= u64::from(def.min_mem_mb) => {}
            Some(mb) => {
                return Err(refuse(format!(
                    "{} needs at least {} MB RAM; this server has {mb} MB.",
                    def.title, def.min_mem_mb
                )));
            }
            None => {
                return Err(refuse(format!(
                    "{} needs at least {} MB RAM and the server memory is unknown.",
                    def.title, def.min_mem_mb
                )));
            }
        }
        mem_mb_min = mem_mb_min.max(def.min_mem_mb);
    }

    let mut missing: Vec<String> = Vec::new();
    let mut present: Vec<String> = Vec::new();
    let mut watched_states: BTreeMap<String, PackageState> = BTreeMap::new();
    let mut disk_need: u32 = 0;
    let mut services: Vec<ServiceEffect> = Vec::new();
    for def in &selected {
        let mut component_missing = false;
        for pkg in def.packages {
            let state = state_of(report, pkg);
            match &state {
                PackageState::Installed { .. } => present.push((*pkg).to_owned()),
                PackageState::Absent => {
                    missing.push((*pkg).to_owned());
                    component_missing = true;
                }
                PackageState::Other { status } => {
                    return Err(refuse(format!(
                        "Package {pkg} is in dpkg state '{status}'. Repair it on the server first."
                    )));
                }
            }
            watched_states.insert((*pkg).to_owned(), state);
        }
        if component_missing {
            disk_need = disk_need.saturating_add(def.est_disk_mb);
            for unit in def.units {
                services.push(ServiceEffect {
                    unit: (*unit).to_owned(),
                    effect: "starts and is enabled when its package is installed".to_owned(),
                });
            }
        }
    }
    missing.sort();
    missing.dedup();
    present.sort();
    present.dedup();

    let steps = steps(&missing)?;
    let disk_mb_needed = if steps.is_empty() {
        0
    } else {
        disk_need.saturating_add(DISK_HEADROOM_MB)
    };

    let mut privileges: Vec<String> = Vec::new();
    if !steps.is_empty() {
        privileges.push(
            "Root through passwordless sudo (sudo -n): apt-get update and apt-get install"
                .to_owned(),
        );
    }
    if selected
        .iter()
        .any(|d| matches!(d.id, "nginx" | "php-fpm" | "mysql"))
    {
        privileges.push(
            "Root through passwordless sudo (sudo -n) during verification: \
             configuration tests and a local database check"
                .to_owned(),
        );
    }

    let mut watched_listings = BTreeMap::new();
    let mut sites = report.sites.clone();
    sites.sort();
    watched_listings.insert(WATCH_SITES.to_owned(), sites);

    let mut hosts = report.package_sources.clone();
    hosts.sort();
    hosts.dedup();

    Ok(Plan {
        target: placeholder_target(),
        os: report.os.clone(),
        components: selected.iter().map(|d| d.id.to_owned()).collect(),
        already_present: present,
        packages: missing
            .iter()
            .map(|n| PackageChange {
                name: n.clone(),
                installed_version: None,
            })
            .collect(),
        files: Vec::new(),
        services,
        resources: Resources {
            disk_mb_needed,
            mem_mb_min,
        },
        privileges,
        limitations: if steps.is_empty() {
            Vec::new()
        } else {
            limitations()
        },
        steps,
        preconditions: Preconditions {
            os_version: report.os.clone(),
            packages: watched_states,
            sudo: report.sudo,
            package_sources: hosts,
            watched_listings,
            min_mem_mb: mem_mb_min,
            min_disk_mb: disk_mb_needed,
            server_managed: report.server_class == ServerClass::OssiframeManaged,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ossiframe_core::{Conflict, Privilege, Severity, SudoMode};

    fn sel(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| (*s).to_owned()).collect()
    }

    fn default_sel() -> Vec<String> {
        sel(components::DEFAULT_SELECTION)
    }

    fn fresh_report() -> PreflightReport {
        PreflightReport {
            observed_at: "2025-01-01T00:00:00Z".into(),
            os: "ubuntu 24.04".into(),
            sudo: SudoMode::Passwordless,
            server_class: ServerClass::Fresh,
            packages: BTreeMap::new(),
            total_mem_mb: Some(4000),
            free_disk_mb: Some(40_000),
            package_sources: vec!["archive.ubuntu.com".into(), "security.ubuntu.com".into()],
            candidate_versions: BTreeMap::new(),
            sites: Vec::new(),
            conflicts: Vec::new(),
        }
    }

    fn installed(v: &str) -> PackageState {
        PackageState::Installed { version: v.into() }
    }

    fn json(p: &Plan) -> String {
        serde_json::to_string(p).unwrap()
    }

    #[test]
    fn same_input_gives_the_same_json_and_order_does_not_matter() {
        let r = fresh_report();
        let a = plan(&r, &default_sel()).unwrap();
        let mut reversed = default_sel();
        reversed.reverse();
        reversed.push("git".into());
        assert_eq!(json(&a), json(&plan(&r, &reversed).unwrap()));
        assert_eq!(json(&a), json(&plan(&r, &default_sel()).unwrap()));
    }

    #[test]
    fn changing_one_package_changes_the_json() {
        let mut r = fresh_report();
        let a = json(&plan(&r, &default_sel()).unwrap());
        r.packages.insert("git".into(), installed("1:2.43"));
        assert_ne!(a, json(&plan(&r, &default_sel()).unwrap()));
    }

    #[test]
    fn components_resolve_with_dependencies_first() {
        let p = plan(&fresh_report(), &sel(&["php-fpm"])).unwrap();
        assert_eq!(p.components, ["php", "php-fpm"]);
        assert!(p.packages.iter().any(|c| c.name == "php8.3-fpm"));
        assert_eq!(
            plan(&fresh_report(), &sel(&["nope"])).unwrap_err(),
            PlanError::UnknownComponent("nope".into())
        );
    }

    #[test]
    fn installed_packages_never_reach_the_install_argv() {
        let mut r = fresh_report();
        r.packages.insert("git".into(), installed("1:2.43"));
        r.packages.insert("nginx".into(), installed("1.24"));
        let p = plan(&r, &default_sel()).unwrap();
        assert_eq!(p.already_present, ["git", "nginx"]);
        assert!(
            !p.packages
                .iter()
                .any(|c| c.name == "git" || c.name == "nginx")
        );
        let mut names: Vec<_> = p.packages.iter().map(|c| c.name.clone()).collect();
        let sorted = {
            let mut s = names.clone();
            s.sort();
            s
        };
        assert_eq!(names, sorted);
        for step in &p.steps {
            let args = step.command.args();
            assert!(!args.iter().any(|a| a == "git" || a == "nginx"), "{args:?}");
        }
        let install = p.steps.last().unwrap();
        assert!(names.drain(..).all(|n| install.command.args().contains(&n)));
        assert!(p.services.iter().all(|s| s.unit != "nginx"));
    }

    #[test]
    fn zero_steps_when_everything_is_installed() {
        let mut r = fresh_report();
        for c in components::COMPONENTS.iter().filter(|c| !c.optional) {
            for pkg in c.packages {
                r.packages.insert((*pkg).into(), installed("1"));
            }
        }
        let p = plan(&r, &default_sel()).unwrap();
        assert!(p.steps.is_empty() && p.packages.is_empty());
        assert!(p.limitations.is_empty());
        assert_eq!(p.resources.disk_mb_needed, 0);
        assert!(!p.already_present.is_empty());
    }

    #[test]
    fn blocker_gives_no_plan_and_a_warning_does_not() {
        let mut r = fresh_report();
        r.conflicts.push(Conflict {
            id: "w".into(),
            severity: Severity::Warning,
            message: "m".into(),
        });
        assert!(plan(&r, &default_sel()).is_ok());
        r.conflicts.push(Conflict {
            id: "b".into(),
            severity: Severity::Blocker,
            message: "m".into(),
        });
        assert_eq!(plan(&r, &default_sel()).unwrap_err(), PlanError::Blocked);
    }

    #[test]
    fn mysql_on_low_or_unknown_memory_is_refused() {
        let mut r = fresh_report();
        r.total_mem_mb = Some(512);
        assert!(matches!(
            plan(&r, &default_sel()),
            Err(PlanError::Refused(m)) if m.contains("512")
        ));
        r.total_mem_mb = None;
        assert!(matches!(
            plan(&r, &default_sel()),
            Err(PlanError::Refused(_))
        ));
        // Without MySQL the same server is fine.
        assert!(plan(&r, &sel(&["nginx"])).is_ok());
    }

    #[test]
    fn broken_dpkg_state_of_a_planned_package_is_refused() {
        let mut r = fresh_report();
        r.packages.insert(
            "nginx".into(),
            PackageState::Other {
                status: "iF".into(),
            },
        );
        assert!(matches!(
            plan(&r, &sel(&["nginx"])),
            Err(PlanError::Refused(_))
        ));
    }

    #[test]
    fn steps_are_privileged_absolute_and_ordered() {
        let p = plan(&fresh_report(), &default_sel()).unwrap();
        let ids: Vec<_> = p.steps.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(
            ids,
            [STEP_APT_UPDATE, STEP_CANDIDATE_CHECK, STEP_APT_INSTALL]
        );
        let modes: Vec<_> = p.steps.iter().map(|s| s.mode).collect();
        assert_eq!(
            modes,
            [StepMode::Durable, StepMode::Gate, StepMode::Durable]
        );

        assert_eq!(
            p.steps[0].command.to_command_line(),
            "'/usr/bin/sudo' '-n' '--' '/usr/bin/apt-get' 'update'"
        );
        assert_eq!(p.steps[0].deadline, Duration::from_secs(300));

        // The gate is read-only: no privilege.
        assert_eq!(p.steps[1].command.privilege(), Privilege::User);
        assert_eq!(p.steps[1].command.program(), "/usr/bin/apt-get");
        assert_eq!(
            &p.steps[1].command.args()[..4],
            ["-s", "install", "-y", "--no-install-recommends"]
        );

        let line = p.steps[2].command.to_command_line();
        assert!(
            line.starts_with(
                "'/usr/bin/sudo' '-n' '--' '/usr/bin/env' 'DEBIAN_FRONTEND=noninteractive' \
                 'NEEDRESTART_SUSPEND=1' '/usr/bin/apt-get' 'install' '-y' \
                 '--no-install-recommends' '-o' 'DPkg::Lock::Timeout=300' 'composer'"
            ),
            "{line}"
        );
        assert_eq!(p.steps[2].deadline, Duration::from_secs(1200));
        for s in &p.steps {
            assert!(s.command.program().starts_with('/'));
        }
    }

    #[test]
    fn plan_discloses_limits_resources_and_services() {
        let p = plan(&fresh_report(), &default_sel()).unwrap();
        let text = p.limitations.join(" ");
        for needle in [
            "no rollback",
            "start automatically",
            "additional dependency",
            "maintainer scripts",
            "No firewall",
            "dpkg --configure -a",
            "port 80",
        ] {
            assert!(text.contains(needle), "{needle}");
        }
        assert_eq!(p.resources.mem_mb_min, 900);
        assert_eq!(
            p.resources.disk_mb_needed,
            60 + 50 + 150 + 20 + 30 + 500 + 1024
        );
        assert_eq!(p.preconditions.min_disk_mb, p.resources.disk_mb_needed);
        assert_eq!(p.preconditions.min_mem_mb, 900);
        let units: Vec<_> = p.services.iter().map(|s| s.unit.as_str()).collect();
        assert_eq!(units, ["nginx", "php8.3-fpm", "mysql"]);
        assert!(p.files.is_empty());
        assert!(!p.privileges.is_empty());
        assert_eq!(p.preconditions.packages.len(), p.packages.len());
    }
}
