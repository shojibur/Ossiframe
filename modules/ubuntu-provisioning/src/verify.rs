//! Post-install verification. Runs after the install steps, from the
//! `Verifying` state of an approved operation.
//!
//! Every selected component gets checks, each marked essential or optional.
//! A component is verified only when all its essential checks pass
//! ([`ComponentVerdict::from_checks`]). A failed, skipped or unavailable
//! essential check never counts as success. Optional checks yield `Warn` or
//! `Skipped` with the reason in the detail. There are no scores.
//!
//! Commands use absolute program paths and fixed argv. Only `nginx -t`,
//! `php-fpm8.3 -t` and `mysql` run privileged (they read root-only files or
//! authenticate through `auth_socket`). Output is bounded by the executor,
//! details are capped at 200 characters, and no configuration contents are
//! collected: the config tests report their exit code only.

use crate::components::{self, ComponentDef};
use crate::preflight::parse_package_states;
use ossiframe_core::{
    CancelToken, CheckResult, CheckStatus, CommandSpec, ComponentVerdict, ExecResult, Executor,
    ExitStatus, PackageState, Plan, ValidationError,
};
use std::collections::BTreeSet;
use std::net::IpAddr;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(15);
const MYSQL_TIMEOUT: Duration = Duration::from_secs(30);
const DETAIL_MAX: usize = 200;

const SYSTEMCTL: &str = "/usr/bin/systemctl";
const FPM_SOCKET: &str = "/run/php/php8.3-fpm.sock";
const CURL: &str = "/usr/bin/curl";

/// PHP extensions the stack must provide (names as printed by `php -m`,
/// lowercase). Checked for both the CLI and FPM.
const PHP_MODULES: &[&str] = &[
    "bcmath",
    "ctype",
    "curl",
    "dom",
    "fileinfo",
    "gd",
    "intl",
    "mbstring",
    "openssl",
    "pdo",
    "pdo_mysql",
    "pdo_sqlite",
    "tokenizer",
    "xml",
    "zend opcache",
    "zip",
];

type Verdict = (CheckStatus, String);
type Ran = Result<ExecResult, String>;

/// Control characters removed, length bounded.
fn cap(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(DETAIL_MAX)
        .collect()
}

fn pass(detail: &str) -> Verdict {
    (CheckStatus::Pass, cap(detail))
}

fn fail(detail: &str) -> Verdict {
    (CheckStatus::Fail, cap(detail))
}

fn first_line(s: &str) -> &str {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
}

fn exit_text(r: &ExecResult) -> String {
    match &r.exit {
        ExitStatus::Code(c) => format!("exit code {c}"),
        ExitStatus::Signal(_) => "ended by a signal".to_owned(),
        ExitStatus::Unknown => "no exit status".to_owned(),
    }
}

fn code_of(r: &ExecResult) -> Option<i32> {
    match r.exit {
        ExitStatus::Code(c) => Some(c),
        _ => None,
    }
}

fn user(program: &str, args: &[&str]) -> Result<CommandSpec, ValidationError> {
    CommandSpec::new(program, args, TIMEOUT)
}

fn root(program: &str, args: &[&str], timeout: Duration) -> Result<CommandSpec, ValidationError> {
    CommandSpec::privileged(program, args, timeout)
}

// ---------------------------------------------------------------------------
// Pure judgements
// ---------------------------------------------------------------------------

fn unavailable(e: &str) -> Verdict {
    fail(&format!("could not run the check: {e}"))
}

/// `systemctl is-active <unit>` must exit 0 and print `active`.
fn judge_active(unit: &str, r: &Ran) -> Verdict {
    match r {
        Ok(r) if r.exit.success() && first_line(&r.stdout) == "active" => {
            pass(&format!("{unit} is active"))
        }
        Ok(r) => fail(&format!(
            "{unit} is not active ({})",
            cap(first_line(&r.stdout))
        )),
        Err(e) => unavailable(e),
    }
}

/// Success and a first output line starting with `prefix`.
fn judge_prefix(r: &Ran, prefix: &str) -> Verdict {
    match r {
        Ok(r) if r.exit.success() => {
            let line = first_line(&r.stdout);
            if line.starts_with(prefix) {
                pass(line)
            } else {
                fail(&format!("unexpected output: {line}"))
            }
        }
        Ok(r) => fail(&format!("command failed ({})", exit_text(r))),
        Err(e) => unavailable(e),
    }
}

/// Success and trimmed stdout equal to `expected`.
fn judge_exact(r: &Ran, expected: &str, ok_detail: &str) -> Verdict {
    match r {
        Ok(r) if r.exit.success() && r.stdout.trim() == expected => pass(ok_detail),
        Ok(r) if r.exit.success() => fail(&format!("unexpected output: {}", first_line(&r.stdout))),
        Ok(r) => fail(&format!("command failed ({})", exit_text(r))),
        Err(e) => unavailable(e),
    }
}

/// A configuration test: only the exit code is reported, never its output.
fn judge_config_test(name: &str, r: &Ran) -> Verdict {
    match r {
        Ok(r) if r.exit.success() => pass(&format!("{name} succeeded")),
        Ok(r) => fail(&format!("{name} failed ({})", exit_text(r))),
        Err(e) => unavailable(e),
    }
}

/// Exit 0 means the path test passed.
fn judge_path_test(what: &str, r: &Ran) -> Verdict {
    match r {
        Ok(r) if r.exit.success() => pass(&format!("{what} exists")),
        Ok(_) => fail(&format!("{what} does not exist")),
        Err(e) => unavailable(e),
    }
}

/// Module names printed by `php -m` / `php-fpm8.3 -m`, lowercase. Section
/// headers like `[PHP Modules]` are skipped.
pub fn parse_modules(stdout: &str) -> BTreeSet<String> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('['))
        .map(str::to_lowercase)
        .collect()
}

/// Required modules absent from `have`.
pub fn missing_modules(have: &BTreeSet<String>) -> Vec<&'static str> {
    PHP_MODULES
        .iter()
        .copied()
        .filter(|m| !have.contains(*m))
        .collect()
}

fn judge_modules(r: &Ran) -> Verdict {
    match r {
        Ok(r) if r.exit.success() => {
            let missing = missing_modules(&parse_modules(&r.stdout));
            if missing.is_empty() {
                pass(&format!("{} required modules present", PHP_MODULES.len()))
            } else {
                fail(&format!("missing modules: {}", missing.join(", ")))
            }
        }
        Ok(r) => fail(&format!("command failed ({})", exit_text(r))),
        Err(e) => unavailable(e),
    }
}

/// Required package states from `dpkg-query`: all must be `ii`.
fn judge_packages(
    states: &Result<std::collections::BTreeMap<String, PackageState>, String>,
    packages: &[&str],
) -> Verdict {
    let states = match states {
        Ok(s) => s,
        Err(e) => return unavailable(e),
    };
    let bad: Vec<String> = packages
        .iter()
        .filter_map(|p| match states.get(*p) {
            Some(PackageState::Installed { .. }) => None,
            Some(PackageState::Other { status }) => Some(format!("{p} ({status})")),
            _ => Some(format!("{p} (not installed)")),
        })
        .collect();
    if bad.is_empty() {
        pass(&format!("{} package(s) installed", packages.len()))
    } else {
        fail(&format!("not installed: {}", bad.join(", ")))
    }
}

/// One `ss -H -tln` line: `(is_loopback, port)`. `None` when not understood.
fn parse_listen_line(line: &str) -> Option<(bool, u16)> {
    // State, Recv-Q, Send-Q, local address, peer address.
    let local = line.split_whitespace().nth(3)?;
    let (host, port) = local.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    let host = host
        .trim_start_matches('[')
        .split(['%', ']'])
        .next()
        .unwrap_or("");
    if host == "*" {
        return Some((false, port));
    }
    let loopback = match host.parse::<IpAddr>().ok()? {
        IpAddr::V4(ip) => ip.is_loopback(),
        IpAddr::V6(ip) => ip
            .to_ipv4_mapped()
            .map_or_else(|| ip.is_loopback(), |v4| v4.is_loopback()),
    };
    Some((loopback, port))
}

/// TCP listeners on `ports` must all be loopback. Output with a line that is
/// not understood is not evidence and fails.
pub fn judge_loopback(r: &Ran, ports: &[u16]) -> Verdict {
    let r = match r {
        Ok(r) if r.exit.success() => r,
        Ok(r) => return fail(&format!("socket list unreadable ({})", exit_text(r))),
        Err(e) => return unavailable(e),
    };
    let mut exposed: Vec<String> = Vec::new();
    let mut seen: BTreeSet<u16> = BTreeSet::new();
    for line in r.stdout.lines().filter(|l| !l.trim().is_empty()) {
        let Some((loopback, port)) = parse_listen_line(line) else {
            return fail("socket list has a line that was not understood");
        };
        if ports.contains(&port) {
            seen.insert(port);
            if !loopback {
                exposed.push(port.to_string());
            }
        }
    }
    if !exposed.is_empty() {
        return fail(&format!(
            "listening on a non-loopback address: port {}",
            exposed.join(", ")
        ));
    }
    if seen.is_empty() {
        pass("no TCP listener on these ports")
    } else {
        let list: Vec<String> = seen.iter().map(u16::to_string).collect();
        pass(&format!("loopback only: port {}", list.join(", ")))
    }
}

/// `curl -w '%{http_code}'`: any returned HTTP status passes.
pub fn judge_http(r: &Ran) -> Verdict {
    match r {
        Ok(r) if r.exit.success() => {
            let code = r.stdout.trim();
            let is_status =
                code.len() == 3 && code.bytes().all(|b| b.is_ascii_digit()) && code != "000";
            if is_status {
                pass(&format!("HTTP status {code}"))
            } else {
                fail("no HTTP status returned")
            }
        }
        Ok(r) => fail(&format!(
            "no HTTP response on 127.0.0.1:80 ({})",
            exit_text(r)
        )),
        Err(e) => unavailable(e),
    }
}

/// Optional version text: first line of stdout, else of stderr. Failure is a
/// warning, not a failure.
fn judge_version(r: &Ran) -> Verdict {
    match r {
        Ok(r) if r.exit.success() => {
            let line = match first_line(&r.stdout) {
                "" => first_line(&r.stderr),
                l => l,
            };
            if line.is_empty() {
                (CheckStatus::Warn, "no version text".to_owned())
            } else {
                pass(line)
            }
        }
        Ok(r) => (
            CheckStatus::Warn,
            cap(&format!("version unavailable ({})", exit_text(r))),
        ),
        Err(e) => (CheckStatus::Warn, cap(&format!("version unavailable: {e}"))),
    }
}

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

struct Verifier<'a> {
    exec: &'a dyn Executor,
    cancel: &'a CancelToken,
    checks: Vec<CheckResult>,
}

impl Verifier<'_> {
    async fn run(&self, spec: Result<CommandSpec, ValidationError>) -> Ran {
        let spec = spec.map_err(|e| cap(&e.to_string()))?;
        match self.exec.run(&spec, self.cancel).await {
            Ok(r) if r.truncated => Err("output was cut at the size limit".to_owned()),
            Ok(r) => Ok(r),
            Err(e) => Err(cap(&e.to_string())),
        }
    }

    fn push(
        &mut self,
        component: &str,
        id: &str,
        title: &str,
        essential: bool,
        (status, detail): Verdict,
    ) {
        self.checks.push(CheckResult {
            component: component.to_owned(),
            id: id.to_owned(),
            title: title.to_owned(),
            essential,
            status,
            detail: cap(&detail),
        });
    }

    async fn services(&mut self, def: &ComponentDef) {
        for unit in def.units {
            let r = self.run(user(SYSTEMCTL, &["is-active", unit])).await;
            self.push(
                def.id,
                &format!("service-{unit}"),
                &format!("Service {unit} is active"),
                true,
                judge_active(unit, &r),
            );
        }
    }

    async fn version(&mut self, def: &ComponentDef, program: &str, args: &[&str]) {
        let r = self.run(user(program, args)).await;
        self.push(def.id, "version", "Version", false, judge_version(&r));
    }

    async fn modules(&mut self, def: &ComponentDef, program: &str) {
        let r = self.run(user(program, &["-m"])).await;
        self.push(
            def.id,
            "modules",
            "Required PHP modules",
            true,
            judge_modules(&r),
        );
    }

    async fn check(&mut self, def: &ComponentDef, sockets: &Ran) {
        match def.id {
            "git" => {
                let r = self.run(user("/usr/bin/git", &["--version"])).await;
                self.push(
                    def.id,
                    "version-runs",
                    "git --version runs",
                    true,
                    judge_prefix(&r, "git version"),
                );
            }
            "composer" => {
                let r = self
                    .run(user("/usr/bin/composer", &["--version", "--no-ansi"]))
                    .await;
                self.push(
                    def.id,
                    "version-runs",
                    "Composer 2 runs",
                    true,
                    judge_prefix(&r, "Composer version 2."),
                );
            }
            "nginx" => {
                self.services(def).await;
                let r = self.run(root("/usr/sbin/nginx", &["-t"], TIMEOUT)).await;
                self.push(
                    def.id,
                    "config-test",
                    "nginx -t",
                    true,
                    judge_config_test("nginx -t", &r),
                );
                self.http(def).await;
                self.version(def, "/usr/sbin/nginx", &["-v"]).await;
            }
            "php" => {
                let r = self.run(user("/usr/bin/php", &["-v"])).await;
                self.push(
                    def.id,
                    "version-runs",
                    "PHP 8.3 runs",
                    true,
                    judge_prefix(&r, "PHP 8.3."),
                );
                self.modules(def, "/usr/bin/php").await;
            }
            "php-fpm" => {
                self.services(def).await;
                let r = self.run(user("/usr/bin/test", &["-S", FPM_SOCKET])).await;
                self.push(
                    def.id,
                    "socket",
                    "FPM socket exists",
                    true,
                    judge_path_test(FPM_SOCKET, &r),
                );
                let r = self
                    .run(root("/usr/sbin/php-fpm8.3", &["-t"], TIMEOUT))
                    .await;
                self.push(
                    def.id,
                    "config-test",
                    "php-fpm8.3 -t",
                    true,
                    judge_config_test("php-fpm8.3 -t", &r),
                );
                self.modules(def, "/usr/sbin/php-fpm8.3").await;
            }
            "mysql" => {
                self.services(def).await;
                let r = self
                    .run(root(
                        "/usr/bin/mysql",
                        &["--no-defaults", "-N", "-B", "-e", "SELECT 1"],
                        MYSQL_TIMEOUT,
                    ))
                    .await;
                self.push(
                    def.id,
                    "local-connection",
                    "Local connection as root (auth_socket)",
                    true,
                    judge_exact(&r, "1", "SELECT 1 returned 1"),
                );
                self.push(
                    def.id,
                    "loopback-only",
                    "Ports 3306 and 33060 are loopback only",
                    true,
                    judge_loopback(sockets, &[3306, 33060]),
                );
                self.version(def, "/usr/sbin/mysqld", &["--version"]).await;
            }
            "redis" => {
                self.services(def).await;
                self.push(
                    def.id,
                    "loopback-only",
                    "Port 6379 is loopback only",
                    true,
                    judge_loopback(sockets, &[6379]),
                );
                let r = self.run(user("/usr/bin/redis-cli", &["ping"])).await;
                self.push(
                    def.id,
                    "ping",
                    "redis-cli ping",
                    true,
                    judge_exact(&r, "PONG", "PONG"),
                );
                self.version(def, "/usr/bin/redis-server", &["--version"])
                    .await;
            }
            other => self.push(
                other,
                "known-component",
                "Component is known",
                true,
                fail("no verification exists for this component"),
            ),
        }
    }

    /// Bounded local request. Essential when `curl` exists, otherwise a
    /// skipped optional check that records the limitation.
    async fn http(&mut self, def: &ComponentDef) {
        const ID: &str = "http-local";
        const TITLE: &str = "Local HTTP request";
        let have = self.run(user("/usr/bin/test", &["-x", CURL])).await;
        match have.as_ref().map(code_of) {
            Ok(Some(0)) => {
                let r = self
                    .run(user(
                        CURL,
                        &[
                            "-sS",
                            "--max-time",
                            "5",
                            "-o",
                            "/dev/null",
                            "-w",
                            "%{http_code}",
                            "http://127.0.0.1/",
                        ],
                    ))
                    .await;
                self.push(def.id, ID, TITLE, true, judge_http(&r));
            }
            Ok(Some(1)) => self.push(
                def.id,
                ID,
                TITLE,
                false,
                (
                    CheckStatus::Skipped,
                    "curl is not installed; the local HTTP request was not made".to_owned(),
                ),
            ),
            // Could not tell: missing evidence is a failure.
            Ok(_) => self.push(
                def.id,
                ID,
                TITLE,
                true,
                fail("could not tell whether curl is installed"),
            ),
            Err(e) => self.push(def.id, ID, TITLE, true, unavailable(e)),
        }
    }
}

/// Runs the checks of every component in `plan.components` (and nothing
/// else). Never panics and never returns an empty list for a selected
/// component: each one has at least its package check.
pub async fn run(exec: &dyn Executor, cancel: &CancelToken, plan: &Plan) -> Vec<CheckResult> {
    let mut v = Verifier {
        exec,
        cancel,
        checks: Vec::new(),
    };
    let mut defs: Vec<&ComponentDef> = Vec::new();
    for id in &plan.components {
        match components::find(id) {
            Some(d) => defs.push(d),
            None => v.push(
                id,
                "known-component",
                "Component is known",
                true,
                fail("no verification exists for this component"),
            ),
        }
    }

    // One package query for every selected component.
    let mut packages: Vec<&str> = defs
        .iter()
        .flat_map(|d| d.packages.iter().copied())
        .collect();
    packages.sort_unstable();
    packages.dedup();
    let states = if packages.is_empty() {
        Ok(Default::default())
    } else {
        let mut args: Vec<&str> = vec![
            "-W",
            "-f",
            "${Package}\\t${db:Status-Abbrev}\\t${Version}\\n",
        ];
        args.extend(&packages);
        match v.run(user("/usr/bin/dpkg-query", &args)).await {
            // Exit 1 is "some package not found"; the output still lists the rest.
            Ok(r) if matches!(code_of(&r), Some(0 | 1)) => {
                let requested: Vec<String> = packages.iter().map(|p| (*p).to_owned()).collect();
                parse_package_states(&r.stdout, &requested)
            }
            Ok(r) => Err(format!("dpkg-query failed ({})", exit_text(&r))),
            Err(e) => Err(e),
        }
    };

    // One socket listing when a component needs it.
    let needs_sockets = defs.iter().any(|d| matches!(d.id, "mysql" | "redis"));
    let sockets: Ran = if needs_sockets {
        v.run(user("/usr/bin/ss", &["-H", "-tln"])).await
    } else {
        Err("not requested".to_owned())
    };

    for def in defs {
        v.push(
            def.id,
            "packages",
            "Packages installed",
            true,
            judge_packages(&states, def.packages),
        );
        v.check(def, &sockets).await;
    }
    v.checks
}

/// One verdict per component of the plan, in plan order.
pub fn verdicts(plan: &Plan, checks: &[CheckResult]) -> Vec<ComponentVerdict> {
    plan.components
        .iter()
        .map(|c| ComponentVerdict::from_checks(c, checks))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::plan as build_plan;
    use crate::preflight::tests::{Fake, Reply, code, fresh, ok};
    use ossiframe_core::{ExecError, PreflightReport, Privilege, ServerClass, SudoMode};
    use std::collections::BTreeMap;

    const LOOPBACK: &str = "LISTEN 0 128 0.0.0.0:22 0.0.0.0:*\n\
        LISTEN 0 151 127.0.0.1:3306 0.0.0.0:*\n\
        LISTEN 0 70 [::1]:33060 [::]:*\n\
        LISTEN 0 511 127.0.0.1:6379 0.0.0.0:*\n";

    fn plan_for(selection: &[&str]) -> Plan {
        let r = PreflightReport {
            observed_at: String::new(),
            os: "ubuntu 24.04".into(),
            sudo: SudoMode::Passwordless,
            server_class: ServerClass::Fresh,
            packages: BTreeMap::new(),
            total_mem_mb: Some(4000),
            free_disk_mb: Some(40_000),
            package_sources: Vec::new(),
            candidate_versions: BTreeMap::new(),
            sites: Vec::new(),
            conflicts: Vec::new(),
        };
        let sel: Vec<String> = selection.iter().map(|s| (*s).to_owned()).collect();
        build_plan(&r, &sel).unwrap()
    }

    fn modules_ok() -> String {
        format!(
            "[PHP Modules]\n{}\n\n[Zend Modules]\n",
            PHP_MODULES
                .join("\n")
                .replace("zend opcache", "Zend OPcache")
        )
    }

    /// A server where every check passes, for all components.
    fn healthy() -> Fake {
        let mut dpkg = String::new();
        for c in components::COMPONENTS {
            for p in c.packages {
                dpkg.push_str(&format!("{p}\tii \t1.0\n"));
            }
        }
        let modules = modules_ok();
        fresh()
            .over("Status-Abbrev}\\t${Version", ok(&dpkg))
            .over("'/usr/bin/systemctl' 'is-active'", ok("active\n"))
            .over("'/usr/bin/git' '--version'", ok("git version 2.43.0\n"))
            .over(
                "'/usr/bin/composer' '--version'",
                ok("Composer version 2.7.1 2024-02-09\n"),
            )
            .over("'/usr/sbin/nginx' '-t'", code(0, "", "syntax is ok\n"))
            .over(
                "'/usr/sbin/nginx' '-v'",
                code(0, "", "nginx version: nginx/1.24.0\n"),
            )
            .over("'/usr/bin/test' '-x'", ok(""))
            .over("'/usr/bin/test' '-S'", ok(""))
            .over("'/usr/bin/curl' '-sS'", ok("200"))
            .over("'/usr/bin/php' '-v'", ok("PHP 8.3.6 (cli) (built: x)\n"))
            .over("'/usr/bin/php' '-m'", ok(&modules))
            .over(
                "'/usr/sbin/php-fpm8.3' '-t'",
                code(0, "", "test is successful\n"),
            )
            .over("'/usr/sbin/php-fpm8.3' '-m'", ok(&modules))
            .over("'/usr/bin/mysql'", ok("1\n"))
            .over("'/usr/sbin/mysqld' '--version'", ok("mysqld  Ver 8.0\n"))
            .over("'/usr/bin/ss'", ok(LOOPBACK))
            .over("'/usr/bin/redis-cli' 'ping'", ok("PONG\n"))
            .over("'/usr/bin/redis-server'", ok("Redis server v=7.0\n"))
    }

    const ALL: &[&str] = &[
        "git", "nginx", "php", "php-fpm", "composer", "mysql", "redis",
    ];

    async fn verify(fake: &Fake, selection: &[&str]) -> (Plan, Vec<CheckResult>) {
        let plan = plan_for(selection);
        let checks = run(fake, &CancelToken::new(), &plan).await;
        (plan, checks)
    }

    fn verified(plan: &Plan, checks: &[CheckResult], id: &str) -> bool {
        verdicts(plan, checks)
            .iter()
            .find(|v| v.component == id)
            .unwrap()
            .verified
    }

    fn find<'a>(checks: &'a [CheckResult], comp: &str, id: &str) -> &'a CheckResult {
        checks
            .iter()
            .find(|c| c.component == comp && c.id == id)
            .unwrap_or_else(|| panic!("no check {comp}/{id}"))
    }

    #[tokio::test]
    async fn healthy_server_verifies_every_component_with_minimal_privilege() {
        let fake = healthy();
        let (plan, checks) = verify(&fake, ALL).await;
        for c in &plan.components {
            assert!(verified(&plan, &checks, c), "{c}: {checks:#?}");
        }
        let mut privileged: Vec<String> = fake
            .commands()
            .iter()
            .filter(|c| c.privilege() == Privilege::Root)
            .map(|c| c.program().to_owned())
            .collect();
        privileged.sort();
        assert_eq!(
            privileged,
            ["/usr/bin/mysql", "/usr/sbin/nginx", "/usr/sbin/php-fpm8.3"]
        );
        assert!(fake.commands().iter().all(|c| c.program().starts_with('/')));
        assert!(checks.iter().all(|c| c.detail.chars().count() <= 200));
    }

    #[tokio::test]
    async fn failed_essential_check_means_not_verified() {
        let fake = healthy().over(
            "'/usr/sbin/nginx' '-t'",
            code(1, "", "bad config at /etc/x"),
        );
        let (plan, checks) = verify(&fake, ALL).await;
        assert_eq!(
            find(&checks, "nginx", "config-test").status,
            CheckStatus::Fail
        );
        assert!(!verified(&plan, &checks, "nginx"));
        assert!(verified(&plan, &checks, "php-fpm"));
        // Config output is never copied into the detail.
        assert!(
            !find(&checks, "nginx", "config-test")
                .detail
                .contains("/etc/x")
        );
    }

    #[tokio::test]
    async fn unavailable_or_skipped_essential_check_means_not_verified() {
        let fake = healthy().over("'/usr/bin/git' '--version'", Reply::Err(ExecError::Timeout));
        let (plan, checks) = verify(&fake, ALL).await;
        assert_eq!(
            find(&checks, "git", "version-runs").status,
            CheckStatus::Fail
        );
        assert!(!verified(&plan, &checks, "git"));

        let skipped = vec![CheckResult {
            component: "x".into(),
            id: "a".into(),
            title: String::new(),
            essential: true,
            status: CheckStatus::Skipped,
            detail: String::new(),
        }];
        assert!(!ComponentVerdict::from_checks("x", &skipped).verified);

        // Package missing: not verified.
        let fake = healthy().over(
            "Status-Abbrev}\\t${Version",
            code(
                1,
                "git\tii \t1\n",
                "dpkg-query: no packages found matching nginx",
            ),
        );
        let (plan, checks) = verify(&fake, &["git", "nginx"]).await;
        assert!(verified(&plan, &checks, "git"));
        assert!(!verified(&plan, &checks, "nginx"));
    }

    #[tokio::test]
    async fn optional_warning_keeps_component_verified() {
        let fake = healthy().over("'/usr/sbin/nginx' '-v'", code(127, "", ""));
        let (plan, checks) = verify(&fake, &["nginx"]).await;
        let v = find(&checks, "nginx", "version");
        assert_eq!(v.status, CheckStatus::Warn);
        assert!(!v.essential);
        assert!(verified(&plan, &checks, "nginx"));
    }

    #[tokio::test]
    async fn mysql_non_loopback_or_unreadable_listener_fails() {
        let wide = "LISTEN 0 151 127.0.0.1:3306 0.0.0.0:*\nLISTEN 0 70 *:33060 *:*\n";
        for out in [
            ok(wide),
            ok("LISTEN 0 151 0.0.0.0:3306 0.0.0.0:*\n"),
            ok("LISTEN 0 151 [::]:3306 [::]:*\n"),
            ok("garbage\n"),
            code(1, "", "ss failed"),
            Reply::Err(ExecError::Timeout),
        ] {
            let fake = healthy().over("'/usr/bin/ss'", out);
            let (plan, checks) = verify(&fake, &["mysql"]).await;
            assert_eq!(
                find(&checks, "mysql", "loopback-only").status,
                CheckStatus::Fail
            );
            assert!(!verified(&plan, &checks, "mysql"));
        }
        // Loopback IPv4 and IPv6 pass.
        let (plan, checks) = verify(&healthy(), &["mysql"]).await;
        assert!(verified(&plan, &checks, "mysql"));
    }

    #[tokio::test]
    async fn mysql_login_must_return_one() {
        let fake = healthy().over("'/usr/bin/mysql'", code(1, "", "Access denied"));
        let (plan, checks) = verify(&fake, &["mysql"]).await;
        assert!(!verified(&plan, &checks, "mysql"));
        let fake = healthy().over("'/usr/bin/mysql'", ok("0\n"));
        let (plan, checks) = verify(&fake, &["mysql"]).await;
        assert!(!verified(&plan, &checks, "mysql"));
    }

    #[tokio::test]
    async fn missing_extension_fails_php_and_fpm() {
        let without = modules_ok().replace("mbstring\n", "");
        assert_eq!(missing_modules(&parse_modules(&without)), vec!["mbstring"]);
        let fake = healthy().over("'/usr/bin/php' '-m'", ok(&without));
        let (plan, checks) = verify(&fake, &["php", "php-fpm"]).await;
        let m = find(&checks, "php", "modules");
        assert_eq!(m.status, CheckStatus::Fail);
        assert!(m.detail.contains("mbstring"));
        assert!(!verified(&plan, &checks, "php"));
        assert!(verified(&plan, &checks, "php-fpm"));

        let fake = healthy().over("'/usr/sbin/php-fpm8.3' '-m'", ok(&without));
        let (plan, checks) = verify(&fake, &["php", "php-fpm"]).await;
        assert!(verified(&plan, &checks, "php"));
        assert!(!verified(&plan, &checks, "php-fpm"));
    }

    #[tokio::test]
    async fn php_must_be_8_3_and_socket_must_exist() {
        let fake = healthy().over("'/usr/bin/php' '-v'", ok("PHP 8.2.1 (cli)\n"));
        let (plan, checks) = verify(&fake, &["php"]).await;
        assert!(!verified(&plan, &checks, "php"));
        let fake = healthy().over("'/usr/bin/test' '-S'", code(1, "", ""));
        let (plan, checks) = verify(&fake, &["php-fpm"]).await;
        assert_eq!(find(&checks, "php-fpm", "socket").status, CheckStatus::Fail);
        assert!(!verified(&plan, &checks, "php-fpm"));
    }

    #[tokio::test]
    async fn http_check_depends_on_curl() {
        // Any HTTP status passes, including an error status.
        let fake = healthy().over("'/usr/bin/curl' '-sS'", ok("404"));
        let (plan, checks) = verify(&fake, &["nginx"]).await;
        let h = find(&checks, "nginx", "http-local");
        assert!(h.essential && h.status == CheckStatus::Pass && h.detail.contains("404"));
        assert!(verified(&plan, &checks, "nginx"));

        // Connection failure or timeout fails an essential check.
        for r in [code(7, "000", "refused"), code(28, "000", "timeout")] {
            let fake = healthy().over("'/usr/bin/curl' '-sS'", r);
            let (plan, checks) = verify(&fake, &["nginx"]).await;
            assert!(!verified(&plan, &checks, "nginx"));
        }

        // curl absent: skipped, non-essential, limitation recorded, no request.
        let fake = healthy().over("'/usr/bin/test' '-x'", code(1, "", ""));
        let (plan, checks) = verify(&fake, &["nginx"]).await;
        let h = find(&checks, "nginx", "http-local");
        assert!(!h.essential && h.status == CheckStatus::Skipped);
        assert!(h.detail.contains("curl is not installed"));
        assert!(verified(&plan, &checks, "nginx"));
        assert!(
            fake.commands()
                .iter()
                .all(|c| c.program() != "/usr/bin/curl")
        );

        // The curl request is bounded.
        let fake = healthy();
        let _ = verify(&fake, &["nginx"]).await;
        let curl = fake
            .commands()
            .into_iter()
            .find(|c| c.program() == "/usr/bin/curl")
            .unwrap();
        assert!(curl.args().windows(2).any(|w| w == ["--max-time", "5"]));
    }

    #[tokio::test]
    async fn redis_must_be_loopback_and_answer_ping() {
        let (plan, checks) = verify(&healthy(), &["redis"]).await;
        assert!(verified(&plan, &checks, "redis"));
        let fake = healthy().over("'/usr/bin/ss'", ok("LISTEN 0 511 0.0.0.0:6379 0.0.0.0:*\n"));
        let (plan, checks) = verify(&fake, &["redis"]).await;
        assert!(!verified(&plan, &checks, "redis"));
        let fake = healthy().over("'/usr/bin/redis-cli' 'ping'", ok("LOADING\n"));
        let (plan, checks) = verify(&fake, &["redis"]).await;
        assert!(!verified(&plan, &checks, "redis"));
    }

    #[tokio::test]
    async fn selection_limits_which_checks_run() {
        let fake = healthy();
        let (plan, checks) = verify(&fake, &["git"]).await;
        assert_eq!(plan.components, ["git"]);
        assert!(checks.iter().all(|c| c.component == "git"));
        assert!(verified(&plan, &checks, "git"));
        let programs: Vec<String> = fake
            .commands()
            .iter()
            .map(|c| c.program().to_owned())
            .collect();
        assert_eq!(programs, ["/usr/bin/dpkg-query", "/usr/bin/git"]);
    }

    #[test]
    fn details_are_capped_and_stripped() {
        let long = "x".repeat(500) + "\u{1b}[31m";
        assert_eq!(cap(&long).chars().count(), 200);
        assert!(!cap("a\nb\u{1b}c").contains(['\n', '\u{1b}']));
    }
}
