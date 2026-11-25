use super::*;
use async_trait::async_trait;
use ossiframe_core::Privilege;
use std::sync::Mutex;

#[derive(Clone)]
pub(crate) enum Reply {
    Out {
        code: i32,
        stdout: String,
        stderr: String,
        truncated: bool,
    },
    Err(ExecError),
}

pub(crate) fn ok(stdout: &str) -> Reply {
    code(0, stdout, "")
}

pub(crate) fn code(code: i32, stdout: &str, stderr: &str) -> Reply {
    Reply::Out {
        code,
        stdout: stdout.to_owned(),
        stderr: stderr.to_owned(),
        truncated: false,
    }
}

pub(crate) fn cut(stdout: &str) -> Reply {
    Reply::Out {
        code: 0,
        stdout: stdout.to_owned(),
        stderr: String::new(),
        truncated: true,
    }
}

/// Answers by substring of the rendered command line; first match wins.
/// An unmatched command panics, so a new probe cannot slip in unnoticed.
pub(crate) struct Fake {
    rules: Vec<(String, Reply)>,
    log: Mutex<Vec<CommandSpec>>,
}

impl Fake {
    pub(crate) fn over(mut self, key: &str, reply: Reply) -> Self {
        self.rules.insert(0, (key.to_owned(), reply));
        self
    }
    pub(crate) fn commands(&self) -> Vec<CommandSpec> {
        self.log.lock().unwrap().clone()
    }
}

#[async_trait]
impl Executor for Fake {
    async fn run(&self, spec: &CommandSpec, _: &CancelToken) -> Result<ExecResult, ExecError> {
        self.log.lock().unwrap().push(spec.clone());
        let line = spec.to_command_line();
        let Some((_, reply)) = self.rules.iter().find(|(k, _)| line.contains(k.as_str())) else {
            panic!("unexpected command: {line}");
        };
        match reply.clone() {
            Reply::Err(e) => Err(e),
            Reply::Out {
                code,
                stdout,
                stderr,
                truncated,
            } => Ok(ExecResult {
                exit: ExitStatus::Code(code),
                stdout,
                stderr,
                truncated,
            }),
        }
    }
}

pub(crate) const K_SS: &str = "'ss'";
pub(crate) const K_INV_DPKG: &str = "${Version}\\t${db:Status";
pub(crate) const K_DPKG: &str = "Status-Abbrev}\\t${Version";
pub(crate) const K_SITES: &str = "'/etc/nginx/sites-enabled'";
pub(crate) const K_UNITS: &str = "'list-units' '--all'";
pub(crate) const K_SVC: &str = "'list-units' '--type=service'";
pub(crate) const K_MEM: &str = "/proc/meminfo";
pub(crate) const K_DF: &str = "'df'";
pub(crate) const K_OS: &str = "/etc/os-release";
pub(crate) const K_SUDO: &str = "'/usr/bin/true'";
pub(crate) const K_HEALTH: &str = "Status-Abbrev}\\t${Package}";
pub(crate) const HEALTHY: &str = "ii \tbase-files\nun \tfoo\nrc \tbar\nii \tdpkg\n";
pub(crate) const K_PGREP: &str = "'/usr/bin/pgrep'";
pub(crate) const K_REBOOT: &str = "/var/run/reboot-required";
pub(crate) const K_CAND: &str = "'/usr/bin/apt-cache'";
pub(crate) const K_MAIN_SRC: &str = "'/etc/apt/sources.list'";
pub(crate) const K_SRC_DIR: &str = "'/etc/apt/sources.list.d' '-mindepth'";
pub(crate) const K_UBUNTU_SRC: &str = "/etc/apt/sources.list.d/ubuntu.sources";

pub(crate) const UBUNTU_SOURCES: &str = "Types: deb\nURIs: http://archive.ubuntu.com/ubuntu/\nSuites: noble\n\
    Components: main\n\nTypes: deb\nURIs: http://security.ubuntu.com/ubuntu/\nSuites: noble-security\n\
    Components: main\n";

pub(crate) const NO_MATCH: &str = "dpkg-query: no packages found matching nginx";

pub(crate) fn os_release(id: &str, version: &str) -> Reply {
    ok(&format!(
        "ID={id}\nVERSION_ID=\"{version}\"\nPRETTY_NAME=\"x\"\n"
    ))
}

pub(crate) fn meminfo(total_kb: u64) -> Reply {
    ok(&format!(
        "MemTotal: {total_kb} kB\nMemAvailable: {total_kb} kB\nSwapTotal: 0 kB\nSwapFree: 0 kB\n"
    ))
}

pub(crate) fn df(avail_bytes: u64) -> Reply {
    ok(&format!(
        "Filesystem 1-blocks Used Available Capacity Mounted on\n\
         /dev/vda1 100000000000 1000 {avail_bytes} 20% /\n"
    ))
}

pub(crate) fn ss_nginx() -> &'static str {
    "tcp LISTEN 0 511 0.0.0.0:80 0.0.0.0:* users:((\"nginx\",pid=5,fd=6))\n"
}

/// A fresh Ubuntu 24.04 server answering every probe.
pub(crate) fn fresh() -> Fake {
    let r: Vec<(&str, Reply)> = vec![
        (K_SUDO, ok("")),
        ("'hostname'", ok("host\n")),
        (K_OS, os_release("ubuntu", "24.04")),
        ("'uname' '-r'", ok("6.8.0\n")),
        ("'uname' '-m'", ok("x86_64\n")),
        ("/proc/uptime", ok("100.0 50.0\n")),
        ("'nproc'", ok("2\n")),
        ("'lscpu'", ok("Model name: Test CPU\n")),
        (K_MEM, meminfo(4_096_000)),
        (K_DF, df(40_000_000_000)),
        ("/proc/stat", ok("cpu  100 0 100 800 0 0 0 0 0 0\n")),
        (
            K_SVC,
            ok(r#"[{"unit":"ssh.service","load":"loaded","active":"active","sub":"running"}]"#),
        ),
        (
            "'list-unit-files'",
            ok(r#"[{"unit_file":"ssh.service","state":"enabled"}]"#),
        ),
        (
            K_SS,
            ok("tcp LISTEN 0 128 0.0.0.0:22 0.0.0.0:* users:((\"sshd\",pid=1,fd=3))\n"),
        ),
        (K_INV_DPKG, ok("python3\t3.12.3\tii \n")),
        (K_DPKG, code(1, "", NO_MATCH)),
        ("'/usr/bin/id'", ok("1000\n")),
        (K_CAND, ok("")),
        (K_HEALTH, ok(HEALTHY)),
        (K_PGREP, code(1, "", "")),
        ("'is-active'", code(3, "inactive\ninactive\n", "")),
        (K_REBOOT, code(1, "", "")),
        (
            K_SITES,
            code(
                1,
                "",
                "find: '/etc/nginx/sites-enabled': No such file or directory\nfind: '/etc/apache2/sites-enabled': No such file or directory\n",
            ),
        ),
        (K_UNITS, ok("")),
        (K_SRC_DIR, ok("/etc/apt/sources.list.d/ubuntu.sources\n")),
        (K_UBUNTU_SRC, ok(UBUNTU_SOURCES)),
        (K_MAIN_SRC, ok("# default\n")),
    ];
    Fake {
        rules: r.into_iter().map(|(k, v)| (k.to_owned(), v)).collect(),
        log: Mutex::new(Vec::new()),
    }
}

pub(crate) fn nginx_installed() -> Reply {
    code(1, "nginx\tii \t1.24.0-2\n", NO_MATCH)
}

pub(crate) async fn run_with(
    fake: &Fake,
    selection: &[&str],
    prior: &[PriorOperation],
) -> Result<(Facts, PreflightReport), OperationError> {
    let input = PreflightInput {
        selection: selection.iter().map(|s| (*s).to_owned()).collect(),
        observed_at: "2025-01-01T00:00:00Z".to_owned(),
        prior_operations: prior.to_vec(),
    };
    preflight_with(fake, &CancelToken::new(), &input).await
}

pub(crate) async fn report(fake: &Fake, prior: &[PriorOperation]) -> PreflightReport {
    run_with(fake, DEFAULT_SELECTION, prior).await.unwrap().1
}

pub(crate) fn blockers(r: &PreflightReport) -> Vec<&str> {
    r.conflicts
        .iter()
        .filter(|c| c.severity == Severity::Blocker)
        .map(|c| c.id.as_str())
        .collect()
}

pub(crate) fn warnings(r: &PreflightReport) -> Vec<&str> {
    r.conflicts
        .iter()
        .filter(|c| c.severity == Severity::Warning)
        .map(|c| c.id.as_str())
        .collect()
}

#[tokio::test]
async fn fresh_server_is_fresh_and_uses_one_privileged_probe_with_absolute_own_paths() {
    let fake = fresh();
    let (facts, r) = run_with(&fake, DEFAULT_SELECTION, &[]).await.unwrap();
    assert_eq!(r.server_class, ServerClass::Fresh);
    assert!(r.can_provision(), "{:?}", r.conflicts);
    assert_eq!(r.sudo, SudoMode::Passwordless);
    assert_eq!(r.os, "ubuntu 24.04");
    assert_eq!(r.total_mem_mb, Some(4000));
    assert_eq!(
        facts.sources.as_ref().unwrap().hosts,
        ["archive.ubuntu.com", "security.ubuntu.com"]
    );
    let cmds = fake.commands();
    let privileged: Vec<_> = cmds
        .iter()
        .filter(|c| c.privilege() == Privilege::Root)
        .collect();
    assert_eq!(privileged.len(), 1);
    assert_eq!(privileged[0].program(), "/usr/bin/true");
    // dpkg health comes from an unprivileged dpkg-query; `dpkg --audit` is not used.
    assert!(cmds.iter().all(|c| c.program() != "/usr/bin/dpkg"));
    assert!(cmds.iter().any(|c| {
        c.privilege() != Privilege::Root
            && c.program() == "/usr/bin/dpkg-query"
            && c.to_command_line().contains(K_HEALTH)
    }));
    // Only the reused inventory collectors use bare program names.
    let inventory_programs = [
        "hostname",
        "cat",
        "uname",
        "nproc",
        "lscpu",
        "df",
        "systemctl",
        "ss",
        "dpkg-query",
    ];
    for c in &cmds {
        assert!(
            c.program().starts_with('/') || inventory_programs.contains(&c.program()),
            "{}",
            c.to_command_line()
        );
    }
}

#[test]
fn sudo_follows_the_exit_code() {
    use ExitStatus::Code;
    let cases = [
        (Code(0), "", SudoMode::Passwordless),
        // Text never turns a failure into success, or success into failure.
        (
            Code(0),
            "sudo: a password is required",
            SudoMode::Passwordless,
        ),
        (
            Code(1),
            "sudo: a password is required",
            SudoMode::PasswordRequired,
        ),
        (
            Code(1),
            "sudo: a terminal is required to read the password",
            SudoMode::PasswordRequired,
        ),
        (
            Code(1),
            "alice is not in the sudoers file",
            SudoMode::NotPermitted,
        ),
        (
            Code(1),
            "Sorry, user alice may not run sudo on host",
            SudoMode::NotPermitted,
        ),
        (Code(1), "", SudoMode::NotPermitted),
        (Code(2), "gibberish", SudoMode::NotPermitted),
        (Code(127), "sudo: not found", SudoMode::Unavailable),
        (
            ExitStatus::Signal("KILL".into()),
            "",
            SudoMode::NotPermitted,
        ),
        (ExitStatus::Unknown, "", SudoMode::NotPermitted),
    ];
    for (exit, stderr, want) in cases {
        assert_eq!(classify_sudo(&exit, stderr), want, "{exit:?} {stderr}");
    }
}

#[tokio::test]
async fn non_passwordless_sudo_blocks() {
    for (reply, id) in [
        (
            code(1, "", "sudo: a password is required"),
            "sudo-password-required",
        ),
        (
            code(1, "", "x is not in the sudoers file"),
            "sudo-not-permitted",
        ),
        (code(127, "", ""), "sudo-unavailable"),
        (Reply::Err(ExecError::Timeout), "sudo-unavailable"),
        (Reply::Err(ExecError::PrivilegeDenied), "sudo-unavailable"),
    ] {
        let r = report(&fresh().over(K_SUDO, reply), &[]).await;
        assert!(!r.can_provision());
        assert!(blockers(&r).contains(&id), "{id}: {:?}", r.conflicts);
    }
}

#[test]
fn package_state_lines_are_parsed() {
    let req: Vec<String> = ["git", "nginx", "mysql-server", "redis-server"]
        .map(String::from)
        .to_vec();
    let out = "git\tii \t1:2.43.0\nnginx\tiF \t1.24\nmysql-server\tun \t\n";
    let s = parse_package_states(out, &req).unwrap();
    assert_eq!(
        s["git"],
        PackageState::Installed {
            version: "1:2.43.0".into()
        }
    );
    assert_eq!(
        s["nginx"],
        PackageState::Other {
            status: "iF".into()
        }
    );
    assert_eq!(s["mysql-server"], PackageState::Absent);
    assert_eq!(s["redis-server"], PackageState::Absent);
    // A package that was not requested is ignored; a malformed line is an error.
    assert_eq!(
        parse_package_states("curl\tii \t1\n", &req).unwrap().len(),
        4
    );
    assert!(parse_package_states("nginx ii 1.24\n", &req).is_err());
}

#[test]
fn candidate_lines_are_parsed() {
    let req: Vec<String> = ["composer", "nginx", "ghost"].map(String::from).to_vec();
    let out = "composer:\n  Installed: (none)\n  Candidate: 2.7.1-2\n  Version table:\n\
               nginx:\n  Installed: (none)\n  Candidate: (none)\n";
    let m = parse_candidates(out, &req);
    assert_eq!(
        (m["composer"], m["nginx"], m["ghost"]),
        (true, false, false)
    );
}

#[test]
fn site_classification() {
    assert!(!is_foreign_site("/etc/nginx/sites-enabled/default"));
    assert!(!is_foreign_site("/etc/nginx/sites-enabled/ossiframe-app"));
    assert!(is_foreign_site("/etc/nginx/sites-enabled/example.com"));
    assert!(is_foreign_site("/etc/nginx/sites-enabled/defaultx-evil"));
    assert!(is_foreign_site(
        "/etc/apache2/sites-enabled/000-default.conf"
    ));
    assert!(is_foreign_site("/srv/other"));
}

#[tokio::test]
async fn existing_sites_block_and_make_the_server_unmanaged() {
    let r = report(
        &fresh().over(K_SITES, ok("/etc/nginx/sites-enabled/example.com\n")),
        &[],
    )
    .await;
    assert!(blockers(&r).contains(&"foreign-sites"));
    assert_eq!(r.server_class, ServerClass::ExistingUnmanaged);

    let r = report(
        &fresh().over(K_SITES, ok("/etc/apache2/sites-enabled/000-default.conf\n")),
        &[],
    )
    .await;
    assert!(blockers(&r).contains(&"foreign-sites"));

    // The distribution default alone does not.
    let r = report(
        &fresh().over(K_SITES, ok("/etc/nginx/sites-enabled/default\n")),
        &[],
    )
    .await;
    assert!(!blockers(&r).contains(&"foreign-sites"));
}

#[tokio::test]
async fn os_rules() {
    let r = report(&fresh().over(K_OS, os_release("ubuntu", "26.04")), &[]).await;
    assert!(blockers(&r).contains(&"os-pending-verification"));
    assert!(
        r.conflicts.iter().any(
            |c| c.id == "os-pending-verification" && c.message.contains("pending verification")
        )
    );
    assert!(!r.can_provision());
    for (id, ver) in [("ubuntu", "22.04"), ("debian", "12"), ("fedora", "24.04")] {
        let r = report(&fresh().over(K_OS, os_release(id, ver)), &[]).await;
        assert!(blockers(&r).contains(&"os-unsupported"), "{id} {ver}");
    }
    let r = report(&fresh().over(K_OS, code(1, "", "no such file")), &[]).await;
    assert!(blockers(&r).contains(&"os-unavailable"));
    let r = report(&fresh().over("'uname' '-m'", ok("riscv64\n")), &[]).await;
    assert!(blockers(&r).contains(&"arch-unsupported"));
    let r = report(&fresh().over("'uname' '-m'", ok("aarch64\n")), &[]).await;
    assert!(r.can_provision());
}

#[tokio::test]
async fn classification_never_trusts_package_presence() {
    // nginx installed and listening, no record: unmanaged and blocked.
    let nginx = |f: Fake| {
        f.over(K_DPKG, nginx_installed())
            .over(K_SS, ok(ss_nginx()))
            .over(K_SITES, ok("/etc/nginx/sites-enabled/default\n"))
    };
    let r = report(&nginx(fresh()), &[]).await;
    assert_eq!(r.server_class, ServerClass::ExistingUnmanaged);
    assert!(blockers(&r).contains(&"existing-unmanaged"));
    assert!(!r.can_provision());

    // A prior record that is not a verified success does not make it managed.
    for prior in [
        PriorOperation::Succeeded {
            all_essential_passed: false,
        },
        PriorOperation::Failed,
        PriorOperation::Cancelled,
        PriorOperation::ClosedManually,
    ] {
        let r = report(&nginx(fresh()), &[prior]).await;
        assert_eq!(r.server_class, ServerClass::Partial, "{prior:?}");
        assert!(r.can_provision(), "{:?}", r.conflicts);
        assert!(warnings(&r).contains(&"partial-previous-operation"));
    }

    // Verified success for this identity: managed, no conflicts for what is installed.
    let r = report(
        &nginx(fresh()),
        &[PriorOperation::Succeeded {
            all_essential_passed: true,
        }],
    )
    .await;
    assert_eq!(r.server_class, ServerClass::OssiframeManaged);
    assert!(r.can_provision(), "{:?}", r.conflicts);
    assert_eq!(
        r.packages["nginx"],
        PackageState::Installed {
            version: "1.24.0-2".into()
        }
    );

    // Earlier failed operation but nothing installed: still fresh.
    let r = report(&fresh(), &[PriorOperation::Failed]).await;
    assert_eq!(r.server_class, ServerClass::Fresh);
}

#[tokio::test]
async fn git_and_composer_alone_do_not_make_a_server_unmanaged() {
    let r = report(
        &fresh().over(
            K_DPKG,
            code(1, "git\tii \t1:2.43\ncomposer\tii \t2.7.1\n", NO_MATCH),
        ),
        &[],
    )
    .await;
    assert_eq!(r.server_class, ServerClass::Fresh);
    assert!(r.can_provision(), "{:?}", r.conflicts);
}

#[tokio::test]
async fn unknown_listener_owner_never_counts_as_owned() {
    let managed = [PriorOperation::Succeeded {
        all_essential_passed: true,
    }];
    let base = |ss: &str| {
        fresh()
            .over(K_DPKG, nginx_installed())
            .over(K_SITES, ok("/etc/nginx/sites-enabled/default\n"))
            .over(K_SS, ok(ss))
    };
    // Owned by nginx, nginx selected: fine.
    let r = report(&base(ss_nginx()), &managed).await;
    assert!(!blockers(&r).contains(&"listener-80"));
    // Owner not visible: blocks even though nginx is selected and installed.
    let r = report(&base("tcp LISTEN 0 511 0.0.0.0:80 0.0.0.0:*\n"), &managed).await;
    assert!(blockers(&r).contains(&"listener-80"));
    // Another process on a managed port blocks.
    let r = report(
        &base("tcp LISTEN 0 5 0.0.0.0:3306 0.0.0.0:* users:((\"evil\",pid=9,fd=3))\n"),
        &managed,
    )
    .await;
    assert!(blockers(&r).contains(&"listener-3306"));
    // nginx on 80 when nginx is not selected blocks.
    let input = ["git"];
    let (_, r) = run_with(&base(ss_nginx()), &input, &managed).await.unwrap();
    assert!(blockers(&r).contains(&"listener-80"));
}

#[tokio::test]
async fn incomplete_or_unavailable_evidence_blocks_the_dependent_rule() {
    let cases: Vec<(&str, Reply, &str)> = vec![
        (K_SS, cut(ss_nginx()), "listeners-evidence"),
        (K_SS, ok("garbage line\n"), "listeners-evidence"),
        (K_SS, Reply::Err(ExecError::Timeout), "listeners-evidence"),
        (K_SVC, Reply::Err(ExecError::Timeout), "services-evidence"),
        (K_SVC, ok("not json"), "services-evidence"),
        (K_INV_DPKG, cut("python3\t3\tii \n"), "software-evidence"),
        (K_INV_DPKG, code(2, "", "boom"), "software-evidence"),
        (K_DPKG, cut(""), "package-evidence"),
        (
            K_DPKG,
            code(1, "", "dpkg: permission denied"),
            "package-evidence",
        ),
        (K_DPKG, ok("nginx ii\n"), "package-evidence"),
        (K_SITES, cut(""), "sites-evidence"),
        (K_SITES, code(2, "", "boom"), "sites-evidence"),
        (K_HEALTH, Reply::Err(ExecError::Timeout), "dpkg-evidence"),
        (K_HEALTH, code(2, "", ""), "dpkg-evidence"),
        (
            K_HEALTH,
            code(2, HEALTHY, "dpkg-query: error: cannot read status\n"),
            "dpkg-evidence",
        ),
        (K_HEALTH, cut(HEALTHY), "dpkg-evidence"),
        (K_HEALTH, ok(""), "dpkg-evidence"),
        (K_HEALTH, ok("garbage line\n"), "dpkg-evidence"),
        (K_DF, code(1, "", "df failed"), "disk-unavailable"),
        (
            K_MAIN_SRC,
            code(1, "", "Permission denied"),
            "sources-evidence",
        ),
        (K_UBUNTU_SRC, cut(UBUNTU_SOURCES), "sources-evidence"),
        (
            K_UBUNTU_SRC,
            ok("garbage without colon\n"),
            "sources-evidence",
        ),
        (
            K_SRC_DIR,
            Reply::Err(ExecError::Timeout),
            "sources-evidence",
        ),
        (
            K_SRC_DIR,
            ok("/etc/apt/sources.list.d/bad name.list\n"),
            "sources-evidence",
        ),
        (K_SRC_DIR, ok("/tmp/x.list\n"), "sources-evidence"),
    ];
    for (key, reply, id) in cases {
        let r = report(&fresh().over(key, reply), &[]).await;
        assert!(!r.can_provision(), "{id}");
        assert!(blockers(&r).contains(&id), "{id}: {:?}", r.conflicts);
        // Unknown state is never presented as fresh.
        if [
            "listeners-evidence",
            "services-evidence",
            "software-evidence",
            "package-evidence",
            "sites-evidence",
        ]
        .contains(&id)
        {
            assert_eq!(r.server_class, ServerClass::ExistingUnmanaged, "{id}");
        }
    }
}

#[tokio::test]
async fn warning_only_rules_do_not_block() {
    let r = report(
        &fresh()
            .over(K_REBOOT, code(0, "", ""))
            .over(K_PGREP, ok("1234 apt-get\n"))
            .over(
                K_CAND,
                ok("composer:\n  Installed: (none)\n  Candidate: (none)\n"),
            ),
        &[],
    )
    .await;
    assert!(r.can_provision(), "{:?}", r.conflicts);
    let w = warnings(&r);
    for id in ["reboot-pending", "apt-busy", "candidate-missing-composer"] {
        assert!(w.contains(&id), "{id}: {w:?}");
    }
    // Failing to check apt activity or the unit scan is a warning only.
    let r = report(
        &fresh()
            .over(K_PGREP, code(2, "", "bad"))
            .over(K_UNITS, Reply::Err(ExecError::Timeout)),
        &[],
    )
    .await;
    assert!(r.can_provision());
    assert!(warnings(&r).contains(&"apt-busy-unknown"));
    assert!(warnings(&r).contains(&"operation-units-unchecked"));
}

#[tokio::test]
async fn dpkg_and_package_state_problems_block() {
    for bad in ["iH ", "iU ", "iF ", "iW ", "it ", "iiR", "riR"] {
        let out = format!("ii \tdpkg\n{bad}\tnginx-extra\nii \tbase-files\n");
        let r = report(&fresh().over(K_HEALTH, ok(&out)), &[]).await;
        assert!(blockers(&r).contains(&"dpkg-broken"), "{bad}");
        assert!(!blockers(&r).contains(&"dpkg-evidence"), "{bad}");
    }
    // Settled states, including held and removed-with-config packages, pass.
    let r = report(
        &fresh().over(K_HEALTH, ok("hi \tkeep\nrc \told\nun \tnone\nii \tdpkg\n")),
        &[],
    )
    .await;
    assert!(r.can_provision(), "{:?}", r.conflicts);
    let r = report(
        &fresh().over(K_DPKG, code(1, "nginx\tiF \t1.24\n", NO_MATCH)),
        &[],
    )
    .await;
    assert!(blockers(&r).contains(&"package-state-nginx"));
}

#[tokio::test]
async fn resource_minimums() {
    // 800 MB with MySQL selected blocks; 1500 MB warns; 4000 MB is quiet.
    let r = report(&fresh().over(K_MEM, meminfo(800 * 1024)), &[]).await;
    assert!(blockers(&r).contains(&"memory-low"));
    let r = report(&fresh().over(K_MEM, meminfo(1500 * 1024)), &[]).await;
    assert!(r.can_provision());
    assert!(warnings(&r).contains(&"memory-tight"));
    // Without MySQL the low memory does not block.
    let (_, r) = run_with(&fresh().over(K_MEM, meminfo(800 * 1024)), &["nginx"], &[])
        .await
        .unwrap();
    assert!(r.can_provision(), "{:?}", r.conflicts);
    // Disk: selection need plus 1 GB.
    let r = report(&fresh().over(K_DF, df(1_000_000_000)), &[]).await;
    assert!(blockers(&r).contains(&"disk-low"));
    let (_, r) = run_with(&fresh().over(K_DF, df(1_200_000_000)), &["git"], &[])
        .await
        .unwrap();
    assert!(r.can_provision(), "{:?}", r.conflicts);
}

#[tokio::test]
async fn foreign_infrastructure_and_operations_block() {
    let sw = |lines: &str| ok(lines);
    for (lines, id) in [
        ("apache2\t2.4\tii \n", "foreign-apache2"),
        ("mariadb-server\t10\tii \n", "foreign-mariadb-server"),
        ("postgresql\t16\tii \n", "foreign-postgresql"),
        ("php8.2-cli\t8.2\tii \n", "php-other-version"),
        ("php7.4-fpm\t7.4\tii \n", "php-other-version"),
    ] {
        let r = report(&fresh().over(K_INV_DPKG, sw(lines)), &[]).await;
        assert!(blockers(&r).contains(&id), "{id}: {:?}", r.conflicts);
        assert_eq!(r.server_class, ServerClass::ExistingUnmanaged);
    }
    // PHP 8.3 packages are the target version, not a conflict.
    let r = report(&fresh().over(K_INV_DPKG, sw("php8.3-cli\t8.3\tii \n")), &[]).await;
    assert!(!blockers(&r).contains(&"php-other-version"));
    // An active foreign service.
    let r = report(
        &fresh().over(
            K_SVC,
            ok(r#"[{"unit":"mariadb.service","load":"loaded","active":"active","sub":"running"}]"#),
        ),
        &[],
    )
    .await;
    assert!(blockers(&r).contains(&"foreign-active-mariadb.service"));
    // A foreign operation unit blocks and names the owner.
    let r = report(
        &fresh().over(
            K_UNITS,
            ok("ossiframe-provision-lock.service loaded active running op=1234 app=ossiframe\n"),
        ),
        &[],
    )
    .await;
    assert!(blockers(&r).contains(&"foreign-operation"));
    assert!(r.conflicts.iter().any(|c| c.message.contains("op=1234")));
}

#[tokio::test]
async fn package_source_allowlist() {
    let sources = |dir: &str, files: Vec<(&str, &str)>| {
        let mut f = fresh().over(K_SRC_DIR, ok(dir));
        for (path, text) in files {
            f = f.over(path, ok(text));
        }
        f
    };
    // A PPA in a one-line file.
    let r = report(
        &sources(
            "/etc/apt/sources.list.d/ubuntu.sources\n/etc/apt/sources.list.d/ppa.list\n",
            vec![(
                "/etc/apt/sources.list.d/ppa.list",
                "deb http://ppa.launchpadcontent.net/ondrej/php/ubuntu noble main\n",
            )],
        ),
        &[],
    )
    .await;
    assert!(blockers(&r).contains(&"sources-not-allowed"));
    assert!(
        r.conflicts
            .iter()
            .any(|c| c.message.contains("ppa.launchpadcontent.net"))
    );
    // Unknown provider mirror in deb822.
    let r = report(
        &sources(
            "/etc/apt/sources.list.d/ubuntu.sources\n",
            vec![(
                K_UBUNTU_SRC,
                "Types: deb\nURIs: https://mirror.provider.example/ubuntu\nSuites: noble\n",
            )],
        ),
        &[],
    )
    .await;
    assert!(blockers(&r).contains(&"sources-not-allowed"));
    // Hostile userinfo trick.
    let r = report(
        &sources(
            "",
            vec![(
                "'/etc/apt/sources.list'",
                "deb http://archive.ubuntu.com@evil.example/ubuntu noble main\n",
            )],
        ),
        &[],
    )
    .await;
    assert!(blockers(&r).contains(&"sources-not-allowed"));
    // A disabled hostile stanza and a cloud mirror are fine; unrelated suffixes are ignored by apt.
    let r = report(
        &sources(
            "/etc/apt/sources.list.d/ubuntu.sources\n/etc/apt/sources.list.d/old.list.save\n",
            vec![(
                K_UBUNTU_SRC,
                "Types: deb\nURIs: http://eu-west-1.ec2.archive.ubuntu.com/ubuntu\nSuites: noble\n\n\
                 Types: deb\nURIs: http://evil.example/\nSuites: n\nEnabled: no\n",
            )],
        ),
        &[],
    )
    .await;
    assert!(r.can_provision(), "{:?}", r.conflicts);
    // Missing main file and missing directory are normal.
    let r = report(
        &fresh()
            .over(
                K_MAIN_SRC,
                code(
                    1,
                    "",
                    "cat: /etc/apt/sources.list: No such file or directory",
                ),
            )
            .over(
                K_SRC_DIR,
                code(
                    1,
                    "",
                    "find: '/etc/apt/sources.list.d': No such file or directory",
                ),
            ),
        &[],
    )
    .await;
    assert!(r.can_provision(), "{:?}", r.conflicts);
}

#[tokio::test]
async fn cancel_and_lost_connection_abort_without_a_report() {
    for e in [ExecError::Cancelled, ExecError::ConnectionLost] {
        for key in [K_OS, K_SUDO, K_SS, K_DPKG, K_UBUNTU_SRC] {
            let err = run_with(
                &fresh().over(key, Reply::Err(e.clone())),
                DEFAULT_SELECTION,
                &[],
            )
            .await
            .unwrap_err();
            assert!(
                matches!(err, OperationError::Exec(ref x) if *x == e),
                "{key}"
            );
        }
    }
}

#[tokio::test]
async fn unknown_component_is_rejected_before_any_command() {
    let fake = fresh();
    let err = run_with(&fake, &["nginx", "laravel"], &[])
        .await
        .unwrap_err();
    assert!(matches!(err, OperationError::InvalidArguments(_)));
    assert!(fake.commands().is_empty());
}

#[test]
fn rfc3339_formatting() {
    assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
    assert_eq!(rfc3339_utc(1_700_000_000), "2023-11-14T22:13:20Z");
    assert_eq!(rfc3339_utc(1_709_164_800), "2024-02-29T00:00:00Z");
}
