//! Step dispatch. Each step collects its evidence once, runs its rules and
//! returns a [`StepResult`].

use crate::certs;
use crate::model::{Category, CheckResult, Outcome, StepResult};
use crate::rules;
use crate::sshd;
use crate::{firewall as fw, updates as upd};
use ossiframe_core::{OperationContext, OperationError};
use ossiframe_mod_linux_inventory as inventory;
use serde_json::json;

pub(crate) async fn run(
    step: &str,
    ctx: &OperationContext<'_>,
) -> Result<StepResult, OperationError> {
    match step {
        "resources" => resources(ctx).await,
        "services" => services(ctx).await,
        "network" => network(ctx).await,
        "software" => software(ctx).await,
        "ssh" => ssh(ctx).await,
        "firewall" => firewall(ctx).await,
        "updates" => updates(ctx).await,
        "certificates" => certificates(ctx).await,
        _ => Err(OperationError::Failed("unknown operation".to_owned())),
    }
}

async fn resources(ctx: &OperationContext<'_>) -> Result<StepResult, OperationError> {
    let at = ctx.observed_at.clone();
    let info = inventory::collect(ctx.exec, ctx.cancel, at.clone()).await?;
    let mut checks = vec![rules::mem_low_available(&info, &at)];
    checks.extend(rules::disk_low_space(&info, &at));
    let evidence = json!({
        "hostname": info.hostname,
        "os": info.os.as_ref().map(|o| json!({
            "prettyName": o.pretty_name,
            "id": o.id,
            "versionId": o.version_id,
        })),
        "kernel": info.kernel,
        "memoryTotalKb": info.memory_total_kb,
        "memoryAvailableKb": info.memory_available_kb,
        "swapTotalKb": info.swap_total_kb,
        "swapFreeKb": info.swap_free_kb,
        "filesystems": info.filesystems,
        "filesystemsTruncated": info.filesystems_truncated,
    });
    Ok(StepResult::new("resources", &at, evidence, checks))
}

async fn services(ctx: &OperationContext<'_>) -> Result<StepResult, OperationError> {
    let at = ctx.observed_at.clone();
    let info = inventory::services::collect(ctx.exec, ctx.cancel, at.clone()).await?;
    let checks = rules::svc_failed(&info, &at);
    let evidence = json!({
        "available": info.available,
        "truncated": info.truncated,
        "unrecognizedRecords": info.unrecognized_records,
        "unitsLoadedOrInstalled": info.services.len(),
        "failedCount": info.services.iter().filter(|s| s.active == "failed").count(),
        "services": info.services,
        "limitations": info.limitations,
    });
    Ok(StepResult::new("services", &at, evidence, checks))
}

async fn network(ctx: &OperationContext<'_>) -> Result<StepResult, OperationError> {
    let at = ctx.observed_at.clone();
    let info = inventory::sockets::collect(ctx.exec, ctx.cancel, at.clone()).await?;
    let checks = rules::net_database_exposed(&info, &at);
    let evidence = json!({
        "available": info.available,
        "truncated": info.truncated,
        "unrecognizedLines": info.unrecognized_lines,
        "socketCount": info.sockets.len(),
        "databasePortSockets": info
            .sockets
            .iter()
            .filter(|s| rules::DATABASE_PORTS.contains(&s.port))
            .collect::<Vec<_>>(),
        "sockets": info.sockets,
        "limitations": info.limitations,
    });
    Ok(StepResult::new("network", &at, evidence, checks))
}

/// Lists known server packages from dpkg. It has no rules: detection of
/// installed software is not a pass or fail. When the package database cannot
/// be read, one `unavailable` check says so.
async fn software(ctx: &OperationContext<'_>) -> Result<StepResult, OperationError> {
    let at = ctx.observed_at.clone();
    let info = inventory::software::collect(ctx.exec, ctx.cancel, at.clone()).await?;
    let checks = if info.available {
        Vec::new()
    } else {
        vec![
            CheckResult::new(
                "software.inventory",
                Category::Services,
                Outcome::Unavailable,
                "Installed server software",
                "Not assessed: the dpkg package database could not be read.",
                &at,
            )
            .limitation(
                "Detection uses the dpkg package database only. Software installed outside \
                 dpkg is not detected.",
            ),
        ]
    };
    let evidence = json!({
        "available": info.available,
        "truncated": info.truncated,
        "packages": info.packages,
        "limitations": info.limitations,
    });
    Ok(StepResult::new("software", &at, evidence, checks))
}

async fn ssh(ctx: &OperationContext<'_>) -> Result<StepResult, OperationError> {
    let at = ctx.observed_at.clone();
    let cfg = sshd::collect(ctx.exec, ctx.cancel).await?;
    let checks = vec![
        rules::ssh_password_auth(&cfg, &at),
        rules::ssh_root_login(&cfg, &at),
    ];
    let evidence = json!({
        "files": cfg.files,
        "values": {
            "passwordAuthentication": cfg.password_authentication.as_ref().map(|s| &s.value),
            "permitRootLogin": cfg.permit_root_login.as_ref().map(|s| &s.value),
        },
        "matchBlocks": cfg.match_blocks,
        "readInFull": cfg.gap.is_none(),
        "limitations": cfg.limitations,
    });
    Ok(StepResult::new("ssh", &at, evidence, checks))
}

async fn firewall(ctx: &OperationContext<'_>) -> Result<StepResult, OperationError> {
    let at = ctx.observed_at.clone();
    let read = fw::collect(ctx.exec, ctx.cancel).await?;
    let (check, enabled) = fw::ufw_config(&read, &at);
    let evidence = json!({ "ufwConfEnabled": enabled });
    Ok(StepResult::new("firewall", &at, evidence, vec![check]))
}

/// Cached data only: never runs apt.
async fn updates(ctx: &OperationContext<'_>) -> Result<StepResult, OperationError> {
    let at = ctx.observed_at.clone();
    let d = upd::collect(ctx.exec, ctx.cancel).await?;
    let (reboot_check, reboot) = upd::reboot_required(&d.reboot, &at);
    let checks = vec![
        upd::security_pending(&d.pending, &d.cache_age, &d.age, &at),
        upd::metadata_stale(&d.age, &at),
        reboot_check,
    ];
    let pending = d.pending.as_ref().ok();
    let evidence = json!({
        "pendingUpdates": pending.map(|p| p.total),
        "pendingSecurity": pending.and_then(|p| p.security),
        "metadataAgeSeconds": d.age.as_ref().ok(),
        "cacheAgeSeconds": d.cache_age.as_ref().ok(),
        "rebootRequired": reboot,
        "source": "cached update-notifier and apt metadata, not live repository state",
    });
    Ok(StepResult::new("updates", &at, evidence, checks))
}

/// Certificates referenced in readable nginx config. No key files, no TLS
/// endpoint scanning.
async fn certificates(ctx: &OperationContext<'_>) -> Result<StepResult, OperationError> {
    let at = ctx.observed_at.clone();
    let c = certs::collect(ctx.exec, ctx.cancel).await?;
    let checks = certs::assess(&c, &at);
    Ok(StepResult::new(
        "certificates",
        &at,
        certs::evidence(&c),
        checks,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use ossiframe_core::{CancelToken, CommandSpec, ExecError, ExecResult, Executor, ExitStatus};

    struct Denied;
    #[async_trait]
    impl Executor for Denied {
        async fn run(&self, _: &CommandSpec, _: &CancelToken) -> Result<ExecResult, ExecError> {
            Ok(ExecResult {
                exit: ExitStatus::Code(1),
                stdout: String::new(),
                stderr: "Permission denied".to_owned(),
                truncated: false,
            })
        }
    }

    struct Lost;
    #[async_trait]
    impl Executor for Lost {
        async fn run(&self, _: &CommandSpec, _: &CancelToken) -> Result<ExecResult, ExecError> {
            Err(ExecError::ConnectionLost)
        }
    }

    fn ctx<'a>(exec: &'a dyn Executor, cancel: &'a CancelToken) -> OperationContext<'a> {
        OperationContext {
            exec,
            cancel,
            observed_at: "2025-01-01T00:00:00Z".to_owned(),
        }
    }

    #[tokio::test]
    async fn failed_collection_never_passes() {
        let cancel = CancelToken::new();
        for step in [
            "resources",
            "services",
            "network",
            "firewall",
            "updates",
            "certificates",
        ] {
            let r = run(step, &ctx(&Denied, &cancel)).await.unwrap();
            assert!(!r.checks.is_empty());
            for c in &r.checks {
                assert_eq!(c.outcome, Outcome::Unavailable, "{step}");
            }
        }
    }

    struct Ident;
    #[async_trait]
    impl Executor for Ident {
        async fn run(&self, spec: &CommandSpec, _: &CancelToken) -> Result<ExecResult, ExecError> {
            let (stdout, code) = match spec.program() {
                "hostname" => ("web-01\n", 0),
                "uname" => ("6.8.0-45-generic\n", 0),
                "cat" if spec.args().first().map(String::as_str) == Some("/etc/os-release") => (
                    "ID=ubuntu\nVERSION_ID=\"24.04\"\nPRETTY_NAME=\"Ubuntu 24.04 LTS\"\n",
                    0,
                ),
                _ => ("", 1),
            };
            Ok(ExecResult {
                exit: ExitStatus::Code(code),
                stdout: stdout.to_owned(),
                stderr: String::new(),
                truncated: false,
            })
        }
    }

    #[tokio::test]
    async fn resources_evidence_has_identity() {
        let cancel = CancelToken::new();
        let r = run("resources", &ctx(&Ident, &cancel)).await.unwrap();
        assert_eq!(r.evidence["hostname"], "web-01");
        assert_eq!(r.evidence["kernel"], "6.8.0-45-generic");
        assert_eq!(r.evidence["os"]["prettyName"], "Ubuntu 24.04 LTS");
        assert_eq!(r.evidence["os"]["versionId"], "24.04");
    }

    struct Exit2;
    #[async_trait]
    impl Executor for Exit2 {
        async fn run(&self, _: &CommandSpec, _: &CancelToken) -> Result<ExecResult, ExecError> {
            Ok(ExecResult {
                exit: ExitStatus::Code(2),
                stdout: String::new(),
                stderr: "boom".to_owned(),
                truncated: false,
            })
        }
    }

    struct Dpkg(&'static str);
    #[async_trait]
    impl Executor for Dpkg {
        async fn run(&self, _: &CommandSpec, _: &CancelToken) -> Result<ExecResult, ExecError> {
            Ok(ExecResult {
                exit: ExitStatus::Code(1),
                stdout: self.0.to_owned(),
                stderr: "dpkg-query: no packages found matching apache2\n".to_owned(),
                truncated: false,
            })
        }
    }

    #[tokio::test]
    async fn software_unavailable_is_one_unavailable_check() {
        let cancel = CancelToken::new();
        let r = run("software", &ctx(&Exit2, &cancel)).await.unwrap();
        assert_eq!(r.checks.len(), 1);
        assert_eq!(r.checks[0].rule_id, "software.inventory");
        assert_eq!(r.checks[0].outcome, Outcome::Unavailable);
        assert_eq!(r.evidence["available"], false);
    }

    #[tokio::test]
    async fn software_lists_packages_without_rules_and_states_dpkg_only() {
        let cancel = CancelToken::new();
        let exec = Dpkg("nginx\t1.24.0-2\tii \nredis-server\t7\trc \n");
        let r = run("software", &ctx(&exec, &cancel)).await.unwrap();
        assert!(r.checks.is_empty());
        let pkgs = r.evidence["packages"].as_array().unwrap();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0]["name"], "nginx");
        assert!(
            r.evidence["limitations"][0]
                .as_str()
                .unwrap()
                .contains("dpkg")
        );
    }

    struct Listing;
    #[async_trait]
    impl Executor for Listing {
        async fn run(&self, spec: &CommandSpec, _: &CancelToken) -> Result<ExecResult, ExecError> {
            let out = match spec.program() {
                "ss" => {
                    "tcp LISTEN 0 511 0.0.0.0:3306 0.0.0.0:* users:((\"mysqld\",pid=7,fd=3))\n\
                         tcp LISTEN 0 128 127.0.0.1:80 0.0.0.0:*\n"
                }
                _ => {
                    "[{\"unit\":\"ssh.service\",\"load\":\"loaded\",\"active\":\"active\",\
                      \"sub\":\"running\",\"description\":\"SSH\"}]"
                }
            };
            Ok(ExecResult {
                exit: ExitStatus::Code(0),
                stdout: out.to_owned(),
                stderr: String::new(),
                truncated: false,
            })
        }
    }

    #[tokio::test]
    async fn services_and_network_evidence_carry_the_bounded_lists() {
        let cancel = CancelToken::new();
        let r = run("network", &ctx(&Listing, &cancel)).await.unwrap();
        assert_eq!(r.evidence["sockets"].as_array().unwrap().len(), 2);
        assert_eq!(
            r.evidence["databasePortSockets"].as_array().unwrap().len(),
            1
        );
        assert_eq!(r.evidence["truncated"], false);
        assert!(r.evidence["limitations"].is_array());
        let r = run("services", &ctx(&Listing, &cancel)).await.unwrap();
        let list = r.evidence["services"].as_array().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["unit"], "ssh.service");
        assert_eq!(r.evidence["unitsLoadedOrInstalled"], 1);
        assert!(r.evidence["limitations"].is_array());
    }

    #[test]
    fn worst_case_services_evidence_fits_the_report_cap() {
        let s = |i: usize| inventory::services::Service {
            unit: format!("{i:0>250}.service"),
            description: Some("d".repeat(256)),
            installed: true,
            load: "loaded".to_owned(),
            active: "active".to_owned(),
            sub: "running".to_owned(),
            enablement: "enabled".to_owned(),
        };
        let list: Vec<_> = (0..inventory::services::MAX_SERVICES).map(s).collect();
        let size = serde_json::to_string(&json!({ "services": list }))
            .unwrap()
            .len();
        assert!(
            size < crate::report::MAX_STEP_EVIDENCE_BYTES - 64 * 1024,
            "{size}"
        );
    }

    #[tokio::test]
    async fn ssh_unreadable_config_is_unavailable_for_both_rules() {
        let cancel = CancelToken::new();
        let r = run("ssh", &ctx(&Denied, &cancel)).await.unwrap();
        let ids: Vec<_> = r.checks.iter().map(|c| c.rule_id).collect();
        assert_eq!(ids, ["ssh.password-auth", "ssh.root-login"]);
        assert!(r.checks.iter().all(|c| c.outcome == Outcome::Unavailable));
        assert_eq!(r.evidence["readInFull"], false);
    }

    /// Answers by program and last argument; records the programs it ran.
    struct Script(std::sync::Mutex<Vec<String>>);
    #[async_trait]
    impl Executor for Script {
        async fn run(&self, spec: &CommandSpec, _: &CancelToken) -> Result<ExecResult, ExecError> {
            let last = spec.args().last().cloned().unwrap_or_default();
            self.0.lock().unwrap().push(spec.program().to_owned());
            let (code, out, err) = match (spec.program(), last.as_str()) {
                ("cat", crate::updates::UPDATES_AVAILABLE) => {
                    (0, "0 updates can be applied immediately.\n", "")
                }
                ("stat", crate::updates::UPDATE_STAMP) => (0, "1000\n", ""),
                ("date", "+%s") => (0, "1000000\n", ""),
                ("stat", _) => (1, "", "stat: cannot statx: No such file or directory"),
                _ => (1, "", "unexpected"),
            };
            Ok(ExecResult {
                exit: ExitStatus::Code(code),
                stdout: out.to_owned(),
                stderr: err.to_owned(),
                truncated: false,
            })
        }
    }

    #[tokio::test]
    async fn updates_with_zero_pending_and_old_cache_is_incomplete_and_runs_no_apt() {
        let cancel = CancelToken::new();
        let exec = Script(Default::default());
        let r = run("updates", &ctx(&exec, &cancel)).await.unwrap();
        let ids: Vec<_> = r.checks.iter().map(|c| c.rule_id).collect();
        assert_eq!(
            ids,
            [
                "upd.security-pending",
                "upd.metadata-stale",
                "upd.reboot-required"
            ]
        );
        assert_eq!(r.checks[0].outcome, Outcome::Incomplete);
        assert_eq!(r.checks[1].outcome, Outcome::Finding);
        assert_eq!(r.checks[2].outcome, Outcome::Passed);
        assert_eq!(r.evidence["rebootRequired"], false);
        assert_eq!(r.evidence["metadataAgeSeconds"], 999_000);
        let ran = exec.0.lock().unwrap();
        assert!(
            ran.iter()
                .all(|p| ["cat", "stat", "date"].contains(&p.as_str()))
        );
    }

    #[tokio::test]
    async fn connection_loss_aborts_the_step() {
        let cancel = CancelToken::new();
        for step in [
            "resources",
            "services",
            "network",
            "software",
            "ssh",
            "firewall",
            "updates",
            "certificates",
        ] {
            let e = run(step, &ctx(&Lost, &cancel)).await.unwrap_err();
            assert_eq!(e, OperationError::Exec(ExecError::ConnectionLost));
        }
    }
}
