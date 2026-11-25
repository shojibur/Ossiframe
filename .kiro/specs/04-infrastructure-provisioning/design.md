# Design Document: Infrastructure Provisioning (S04)

## Overview

S04 adds the first server mutations: installing a base stack (Git, NGINX, PHP 8.3 CLI and FPM, Composer, MySQL, optional Redis) on a fresh Ubuntu 24.04 server. It follows the flow preflight, plan, explicit approval, execution, verification. It reuses the S01 to S03 stack: `Executor`/`CommandSpec`, russh transport, `ServerService` connections, the module `Registry`, `ServerWorkspace` and `@ossiframe/client`.

Three artifacts are new: a privilege on `CommandSpec`; a module crate `modules/ubuntu-provisioning` with preflight, component data and plan building; and a generic plan/approval/operation engine in `ossiframe-desktop`, compiled only with the `provisioning` feature. A new minimal app `apps/deploy` consumes them. Doctor enables none of it and stays read-only.

Scope decisions made here (details in the sections and the decision table):

- Passwordless sudo only (`sudo -n`). No sudo password in S04.
- Long apt steps run as transient systemd units (`systemd-run`) so they survive an SSH disconnect.
- `apt-get update` is a visible, approved plan step.
- Config-file writes and stdin plumbing are deferred to S05. S04 writes no files and sends no stdin, because the distro defaults need no change.
- Plan digest uses `sha2` (already in `Cargo.lock` through russh).
- A remote exclusion lock (the transient unit `ossiframe-provision-lock`) covers the whole mutation lifecycle. It is cooperative among Ossiframe instances.
- Operation history survives profile removal through an identity snapshot, and a profile cannot be removed or materially edited while an operation is unresolved.
- A missing transient unit never proves a step did not run. Reconcile decides from evidence, and a user "mark resolved" action exists for the rest.

Out of scope: Laravel and app lifecycle, vhosts and FPM pools for apps, database users and databases, SSL/Certbot, firewall or SSH changes, multi-PHP and PPAs, Node/Python/container adapters, adoption of existing servers, monitoring, automatic cloud creation.

## Architecture

```mermaid
flowchart TB
  subgraph Apps
    Deploy["apps/deploy"]
    Doctor["apps/doctor (unchanged, read-only)"]
    Gen["generated app (opt-in only)"]
  end
  subgraph Modules
    Prov["ubuntu-provisioning"]
    Inv["linux-inventory"]
  end
  subgraph Skeleton
    Core["ossiframe-core: CommandSpec+Privilege, ReadOnly, Provisioner trait, Plan types"]
    Desk["ossiframe-desktop: store, ServerService, [feature provisioning] engine + commands"]
    Ssh["ossiframe-ssh"]
  end
  Deploy --> Prov & Inv & Desk
  Doctor --> Inv & Desk
  Gen -. "--enable-provisioning" .-> Prov
  Prov --> Inv & Core
  Desk --> Core & Ssh
```

Layering constraint: `ossiframe-desktop` is skeleton and must not depend on a module. So the engine is generic and the module plugs in through a small `Provisioner` trait defined in Core. S05 (Laravel deploy plans) reuses the same engine, which is the concrete reuse need that justifies the trait.

Flow:

```mermaid
sequenceDiagram
    participant UI as Deploy UI
    participant E as Engine (desktop)
    participant P as Provisioner (module)
    participant S as Server
    UI->>E: provision_preflight(serverId)
    E->>P: preflight(exec)
    P->>S: read probes + sudo -n true
    E-->>UI: PreflightReport
    UI->>E: provision_create_plan(serverId, components)
    E->>P: preflight again, plan(report, components)
    E-->>UI: Plan{id, digest, expiresAt}
    UI->>E: provision_approve_plan(serverId, planId, digest)
    E->>E: checks (expiry, supersede, digest, host key)
    E->>P: recheck(plan)
    E-->>UI: operationId (Pending)
    loop per step
      E->>S: systemd-run unit, poll systemctl show
      E-->>UI: event operation-updated
    end
    E->>P: verify(plan) (Verifying)
    E-->>UI: Succeeded / Failed / UnknownOutcome
```

## Privilege model and Core changes

`CommandSpec` gets a privilege and nothing else new. Argv-only, every word quoted, as today.

```rust
// ossiframe-core/src/command.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Privilege { User, Root }

impl CommandSpec {
    /// Rendered as: '/usr/bin/sudo' '-n' '--' '<program>' '<arg>'...
    /// `program` must be an absolute path (no PATH or secure_path ambiguity).
    pub fn privileged(program: &str, args: &[&str], timeout: Duration) -> Result<Self, ValidationError>;
    pub fn privilege(&self) -> Privilege;
}
```

- `CommandSpec::new` keeps its signature and yields `Privilege::User`. `to_command_line()` adds the `/usr/bin/sudo -n --` prefix (fixed sudo path) only for `Root`.
- `sudo -n` fails instead of prompting. A sudo password prompt can never hang a channel.
- No stdin, no sudo password. The sudo password path (`sudo -S -p ''`, secret over stdin, redaction, one-operation lifetime) is listed as a decision. Deferring it removes the only S04 caller of stdin. When S05 needs stdin for file content, it is an additive `CommandSpec` field and a change to `SshExecutor`, which today sends EOF right after exec.
- Privilege detection happens in preflight, never assumed (see Preflight).
- New `ExecError::PrivilegeDenied` (command was not run). `IpcError` maps it to `operationFailed` with a fixed message.

`ossiframe-core` also gains `OperationState` (the seven ADR-0004 states), the `Plan` types and the `Provisioner` trait in a new `provision.rs`. They carry no NGINX, PHP or MySQL knowledge: a plan is packages, services, files, steps and checks.

```rust
// ossiframe-core/src/provision.rs
pub struct Plan {
    pub target: PlanTarget,              // identity snapshot: server_id, name, host, port, user, pinned host key sha256
    pub os: String,                      // "ubuntu 24.04"
    pub components: Vec<String>,         // resolved order, requested + dependencies
    pub already_present: Vec<String>,
    pub packages: Vec<PackageChange>,    // name, installed_version: Option<String>
    pub files: Vec<FileChange>,          // empty in S04
    pub services: Vec<ServiceEffect>,    // unit, "started by package scripts"
    pub resources: Resources,            // disk_mb_needed, mem_mb_min
    pub privileges: Vec<String>,         // e.g. "root via sudo -n"
    pub limitations: Vec<String>,        // recovery limits shown before approval
    pub steps: Vec<PlanStep>,
    pub preconditions: Preconditions,    // material facts re-checked before run
}
pub struct PlanStep { pub id: String, pub title: String, pub command: CommandSpec, pub mode: StepMode, pub deadline: Duration }
pub enum StepMode { Foreground, Durable, Gate }  // Durable = transient systemd unit, Gate = read-only check by the Provisioner
pub enum ServerClass { Fresh, ExistingUnmanaged, OssiframeManaged, Partial }
pub struct CheckResult { pub component: String, pub id: String, pub title: String, pub essential: bool, pub status: CheckStatus, pub detail: String }

#[async_trait]
pub trait Provisioner: Send + Sync {
    fn components(&self) -> Vec<ComponentInfo>;           // id, title, description, requires, optional
    async fn preflight(&self, exec: &dyn Executor, cancel: &CancelToken) -> Result<PreflightReport, OperationError>;
    fn plan(&self, report: &PreflightReport, selection: &[String]) -> Result<Plan, PlanError>;
    async fn recheck(&self, exec: &dyn Executor, cancel: &CancelToken, plan: &Plan) -> Result<(), Vec<Drift>>;
    async fn gate(&self, exec: &dyn Executor, cancel: &CancelToken, plan: &Plan, step_id: &str) -> Result<String, String>; // Ok = bounded evidence text
    async fn verify(&self, exec: &dyn Executor, cancel: &CancelToken, plan: &Plan) -> Vec<CheckResult>;
}
```

`Plan` derives `Serialize` with `Vec`/`BTreeMap` only, so its JSON is deterministic.

## Enforcement

Module metadata is not a sandbox. Enforcement is structural and layered:

1. **Privilege rejected on the module-operation path.** Core adds `ReadOnly<'a>(&'a dyn Executor)`, whose `run` returns `ExecError::PrivilegeDenied` for any spec whose privilege is not `User`. `dispatch::run_operation` builds `OperationContext { exec: &ReadOnly(&*conn), .. }`. The guard checks the `Privilege` field only. It is not a command classifier, it does not stop a user-level command from changing server state, and it does not stop trusted Rust code (modules are compile-time crates) from building its own executor and calling sudo. Doctor's read-only behavior comes from its registered modules (predefined read-only argv), the restricted typed IPC surface, and the absence of any provisioning registration or setup (layers 2 to 5). The `ubuntu-provisioning` module registers with zero `OperationInfo` entries, so `run_operation` has nothing in it to reach. `OperationInfo` and `Module` do not change (S02 contract stays stable).
2. **Privileged path only through typed engine commands.** Ten `provision_*` commands (below) exist only with `ossiframe-desktop` features `tauri` and `provisioning`, and only an app that calls `provisioning::setup(app, Arc<dyn Provisioner>)` has state for them. The commands take ids, component ids and a digest, never a program, argv, path or privilege.
3. **Privileged execution only after approval.** The engine's `execute(ApprovedPlan)` takes a type whose only constructor is `PlanStore::take_approved`, which performs the approval checks. The privileged `CommandSpec`s are built by trusted `Provisioner` code and are executed only by the engine. The one privileged command before approval is the fixed preflight probe `/usr/bin/sudo -n -- /usr/bin/true`. After approval, privileged commands (lock, steps, journal reads, reconcile and resolution evidence reads, verification) are tied to an Operation created from an `ApprovedPlan` and use fixed argv with absolute program paths.
4. **App composition.** A command is reachable only when it appears in the app's `generate_handler!`, its `build.rs` `AppManifest`, and its capability file. Doctor has none of the `provision_*` names in any of the three. With Cargo feature unification, `cargo build --workspace` compiles the engine into Doctor's dependency graph when Deploy enables the feature. Dead code in Doctor is acceptable only because of the three-way gate, so the gate is what the tests assert. Release builds use `cargo tauri build` per app.
5. **Drift tests** (Doctor): capability, `build.rs` and handler lists equal `COMMAND_NAMES` plus report commands exactly (existing tests). New: no permission contains `provision`; Doctor `Cargo.toml` names neither `ossiframe-mod-ubuntu-provisioning` nor the `provisioning` feature. Deploy has the mirror tests against `COMMAND_NAMES + PROVISION_COMMAND_NAMES + inventory`.

## Preflight

Read-only apart from the sudo probe. It reuses `linux-inventory` library functions (`collect` for server info, `software`, `services`, `sockets`) and adds a small number of fixed probes. All commands use absolute paths.

| Fact | Source | Rule |
|---|---|---|
| OS, arch | inventory `server-info` (`/etc/os-release`, `uname -m`) | `ubuntu` + `24.04` supported. `26.04` reported as "pending verification" and blocked. Other OS or version blocked. `x86_64` and `aarch64` accepted, only tested architectures claimed in docs |
| Memory, swap, disk | inventory | Block if RAM < 900 MB and MySQL selected. Warn below ~1.8 GB. Block if free disk on `/` < estimated need + 1 GB |
| Connectivity, uid | the session itself, `/usr/bin/id -u` | Connected required |
| Sudo | `/usr/bin/sudo -n -- /usr/bin/true` | exit 0 gives `Passwordless`. Stderr "password is required" gives `PasswordRequired`. "not in the sudoers" or "may not run" gives `NotPermitted`. Exit 127 gives `Unavailable`. Anything but `Passwordless` blocks, with an actionable message. Matching is on English text only as a hint; the exit code decides the block |
| Package states | `/usr/bin/dpkg-query -W -f '${Package}\t${db:Status-Abbrev}\t${Version}\n'` on planned packages | Per package: absent, `ii` (installed), or other state (blocker: broken dpkg state) |
| Package candidates | `/usr/bin/apt-cache policy <pkgs>` | `Candidate: (none)` is a warning before the update step (lists may be stale), a failure afterwards. Composer is in `universe`; warn if candidate missing |
| dpkg health | `/usr/bin/dpkg-query -W -f '${db:Status-Abbrev}\t${Package}\n'` (all packages, no root; `dpkg --audit` needs root and is not used) | Blocker `dpkg-broken` when any package is half-installed (`H`), unpacked (`U`), half-configured (`F`), triggers-awaited (`W`), triggers-pending (`t`) or has the reinst-required error flag (`R`). Installed (`i`), not-installed (`n`) and config-files (`c`) pass. A non-zero exit, timeout, truncated or unparseable output is missing evidence (`dpkg-evidence`, blocker). Limitation: it does not cover the missing-metadata checks of `dpkg --audit`. The message tells the user to inspect manually; Ossiframe never runs `dpkg --configure -a`. Parser: `provision::dpkg_states_settled` in Core, shared with reconcile |
| apt/dpkg busy | `/usr/bin/pgrep -x -l` for `apt`, `apt-get`, `dpkg`, `unattended-upgr`, plus `systemctl is-active apt-daily.service apt-daily-upgrade.service` | Warning only. Unprivileged `fuser` cannot see other users' lock holders, so process names are used. The install step waits with `-o DPkg::Lock::Timeout=300` |
| Reboot pending | `/usr/bin/test -f /var/run/reboot-required` | Warning |
| Web configs | `/usr/bin/find /etc/nginx/sites-enabled /etc/apache2/sites-enabled -maxdepth 1 -mindepth 1` | Any apache2 site, or nginx entry other than `default` and `ossiframe-*` blocks |
| Existing DB/web infra | inventory `software` + `services` | Installed or active `apache2`, `mariadb-server`, `postgresql` block. A different PHP minor than 8.3 blocks |
| Listeners | inventory `sockets` | Ports 80, 443, 3306, 33060, 6379 held by a process not belonging to a selected or installed managed component block. A listener whose owner cannot be determined (an unprivileged `ss` hides other users' processes) never counts as owned by itself. It is accepted only by the "installed and managed" rule below |

**Listener ownership without root.** `ss` run as the SSH user shows no owner for sockets of other users, which is every listener of nginx, mysqld and redis-server. Treating unknown owners as always foreign made a server that Ossiframe itself provisioned fail its own next preflight (ports 80, 3306 and 33060 blocked after a verified install), so no idempotent re-run and no later Redis install was possible. Rule: a listener on a managed port is explained when (1) the Server_Class is OssiframeManaged, meaning the Store holds a verified Succeeded operation for this identity snapshot, (2) every package of the port's component is `ii`, and (3) every systemd unit of that component is active in the inventory `services` list. A named process still has to match the expected name, and is accepted when its component is selected or explained. Evidence that is missing or unreadable explains nothing. Not chosen: reading `/proc` or `systemctl show -p MainPID` to match PIDs to sockets (needs root or does not give socket ownership), and per-component install records (the Store keeps the server-level outcome only). Residual risk, accepted: on a managed server, a different process holding a managed port while the component's unit is active cannot be told apart without root. The unit could not have bound that port itself, and it would normally fail. The Re-check cannot read the Store, so the plan records `preconditions.serverManaged` and the Re-check evaluates listeners with the same history.
| Foreign operations | `/usr/bin/systemctl list-units --all --plain --no-legend 'ossiframe-op-*' 'ossiframe-provision-lock*'` | Any unit blocks, with owner details when the lock description is readable. This scan is advisory. The remote lock taken at approval is authoritative |
| Package sources | read `/etc/apt/sources.list`, `/etc/apt/sources.list.d/*.list` and `*.sources` (deb822) | Every enabled source host must be on the allowlist: `archive.ubuntu.com`, `*.archive.ubuntu.com` (covers cloud mirrors such as `*.ec2.archive.ubuntu.com`), `security.ubuntu.com`, `ports.ubuntu.com`. Anything else (PPA, third-party or unknown provider mirror) blocks with an explanation. Unreadable or unparsable files block. Sources are never modified. Provider mirrors are added to the allowlist data only after verification |

`PreflightReport` carries `observedAt`, the facts above, a `ServerClass`, a list of `Conflict { id, severity: Blocker|Warning, message }`, and a derived `canProvision`. Blockers prevent plan creation. Existing servers can still be inspected with Doctor.

**Evidence completeness.** A probe that times out, fails to parse or hits an output cap makes every rule that depends on it a Blocker ("evidence unavailable"). Absence of evidence is never read as absence of a conflict.

**Server classification** (decided from observed facts plus the local Store, with no adoption path):

| Class | Rule | Effect |
|---|---|---|
| Fresh | No service-bearing planned package (`nginx`, `php*-fpm`, `mysql-server`, `redis-server`), no foreign web or DB package, no non-default site, no listener on a managed port | Plan allowed. `git` and `composer` may be preinstalled and are skipped as already present |
| ExistingUnmanaged | Any of the above present and no Ossiframe record for this identity | Blocker. Use Doctor to inspect |
| OssiframeManaged | The Store holds a Succeeded operation for this identity snapshot (endpoint and pinned host key) whose verification had no failed essential check. Package presence, file names or unit names never establish this | Planned packages already installed are listed as already present. Used to add components later |
| Partial | An earlier Failed, Cancelled or manually closed operation for this identity, and service-bearing planned packages present | Warning. Plan allowed only when the dpkg health listing shows no interrupted or unsettled package and every package state is absent or `ii`; installed packages are skipped |

The preflight and the plan carry one explicit connection identity (server id, endpoint, pinned host key) that the operation keeps for its whole life.

## Components and Interfaces

### The ubuntu-provisioning module

One new crate, `modules/ubuntu-provisioning` (`ossiframe-mod-ubuntu-provisioning`), depending on `ossiframe-core` and `ossiframe-mod-linux-inventory` (the manifest declares `linux-inventory`). A second crate is not justified: components are data plus verification functions of the same shape. Optional `ui/` directory holds the React pages (below).

```rust
struct ComponentDef {
    id: &'static str, title: &'static str,
    packages: &'static [&'static str],
    requires: &'static [&'static str],
    units: &'static [&'static str],            // expected active after install
    est_disk_mb: u32, min_mem_mb: u32,
    blocks_on_packages: &'static [&'static str],
    optional: bool,
}
```

| id | Packages (Ubuntu 24.04 archive only, no PPAs) | Requires | Units | Notes |
|---|---|---|---|---|
| `git` | `git` | | | |
| `nginx` | `nginx` | | `nginx` | Package starts nginx with the distro default site on port 80. Blocks on `apache2` |
| `php` | `php8.3-cli php8.3-common php8.3-opcache php8.3-mbstring php8.3-xml php8.3-curl php8.3-zip php8.3-bcmath php8.3-intl php8.3-mysql php8.3-sqlite3 php8.3-gd` | | | CLI and Laravel extensions. Names to be re-checked with `apt-cache policy` on a noble host before coding. `php8.3-opcache` and `composer` 2.7.1 confirmed in the noble archive |
| `php-fpm` | `php8.3-fpm` | `php` | `php8.3-fpm` | Default pool and socket `/run/php/php8.3-fpm.sock`. Not wired to NGINX (S05) |
| `composer` | `composer` | `php` | | Archive package (universe, 2.7.1-2). Official installer rejected: it downloads and executes code from the network and needs checksum handling, against the "archive only" rule |
| `mysql` | `mysql-server` | | `mysql` | MySQL 8.0 from noble. Local root via `auth_socket`. Blocks on `mariadb-server`. Hard memory minimum |
| `redis` (optional) | `redis-server` | | `redis-server` | Unchecked by default |

Default selection: `git nginx php php-fpm composer mysql`. PostgreSQL, Node.js, Supervisor and Certbot use the same `ComponentDef` pattern later and are not implemented now. Versions are whatever noble provides and are recorded in docs only after verification. Servers stay general-purpose: any subset can be chosen and more components can be added in a later operation. No runtime-typed server profile exists.

Plan building (`plan(report, selection)`, pure and synchronous):

1. Resolve the selection with dependencies (reuse `RegistryBuilder::resolve_selection` over component manifests, or a ten-line equivalent if the types do not fit). Unknown id: `PlanError::UnknownComponent`.
2. Refuse if the report has any blocker, or a selected component's own rule fires (for example MySQL on low RAM).
3. Subtract packages already `ii`. Sort the rest.
4. Emit steps (none when everything is present, in which case the operation only verifies):
   - `apt-update`: durable, privileged `/usr/bin/apt-get update`, deadline 5 min. It modifies only package lists and is listed in the plan.
   - `candidate-check`: gate (read-only, runs through `Provisioner::gate`). After the update it checks that `apt-cache policy` gives every package a candidate whose origin host is on the source allowlist, and that `apt-get install -s --no-install-recommends <pkgs>` lists only packages from allowlisted origins (this covers dependencies and apt pinning). The exact install set is stored as step output for history. On failure the operation fails before any install, with text telling the user what to fix and to re-plan.
   - `apt-install`: durable, one dpkg transaction for all missing packages, deadline 20 min. It runs `/usr/bin/apt-get install -y --no-install-recommends -o DPkg::Lock::Timeout=300 <pkgs>`.
5. Fill `services`, `resources`, `privileges`, and `limitations`: no rollback; a failed install can leave partial packages; recovery is re-planning, which is idempotent; services start automatically on install and package scripts may restart services (actual `needrestart` behavior is recorded only from live verification, and no "no restarts" claim is made); APT may install additional dependencies and run maintainer scripts; no firewall changes; Ossiframe never runs `dpkg --configure -a`, removes packages or terminates apt or dpkg; the default NGINX page is publicly reachable if port 80 is open.
6. Transitive dependencies: when package lists are present, `plan` shows the `apt-get install -s` estimate. On a fresh image the lists are usually empty, so the plan says the set is unknown until after the update and relies on the gate's origin check and the stored exact set.

## Plan and approval

`PlanStore` (desktop, in memory) holds at most one plan per server, which makes "superseded" automatic.

```rust
struct StoredPlan { plan: Plan, id: Uuid, digest: String, created: Instant, expires: Instant }
// digest = hex(sha256(serde_json::to_vec(&plan)))   // id and timestamps are not part of the plan body
```

- **Digest:** SHA-256 over the canonical JSON of the `Plan` (deterministic field and `Vec` order; no maps with unordered keys). Add `sha2` to `ossiframe-desktop` (optional, `provisioning` feature). It is RustCrypto, MIT/Apache-2.0, maintained, and already in `Cargo.lock`. No custom cryptography. The digest is a consent binding and integrity check for what the user saw, and it is re-verified when a stored plan is read back from SQLite. It is not the security boundary against a hostile frontend. That boundary is that the backend executes only its stored plan.
- **Binding:** `plan.target` holds the identity snapshot (server id, name, host, port, user, pinned host key SHA-256) at preflight. Approval compares host, port, user and key with the current server record and the live connection. The frontend submits only ids and the digest, never commands or plan contents. The user confirms through a dialog (see Deploy app), not by typing the digest.
- **Lifetime:** 10 minutes, single use, replaced by any newer plan for the same server.
- **Approve:** `provision_approve_plan(serverId, planId, planDigest)`. In order: validate ids; take the stored plan (`UnknownPlan`); plan id equal (`Superseded`); not expired (`Expired`); `planDigest` equals stored digest and a recomputed digest (`Mismatch`); server and pinned key unchanged (`WrongServer`); connected; no active operation or unresolved `UnknownOutcome` for the server (`Busy`). The plan is consumed at this point. Then `Provisioner::recheck` re-runs the material probes. Any `Drift` aborts with `RePlanRequired{reasons}` and nothing runs. If recheck is clean, the engine acquires the remote lock (below). A failed acquisition returns `busy` with the owner details and nothing runs. Material means OS version, package states of the planned packages, blockers (including foreign operation units and the lock owner), sudo mode, the nginx site list, package sources, and the minimums: total RAM and free disk are re-evaluated against the plan's resource needs, and drift is reported when either is below its minimum. Ordinary memory fluctuation above the minimum, uptime and apt-busy are not drift.
- A plan with zero steps is valid and results in a verify-only operation (this also serves as re-verification).

## Operation engine

### State machine

```mermaid
stateDiagram-v2
    [*] --> Pending: approved
    Pending --> Running: lock acquired
    Running --> Verifying: all steps done
    Running --> Failed: step exit != 0
    Running --> Cancelled: cancel honoured between steps
    Running --> UnknownOutcome: disconnect, deadline, crash
    Verifying --> Succeeded: no failed check
    Verifying --> Failed: a check failed
    Verifying --> UnknownOutcome: disconnect
    UnknownOutcome --> Failed: reconcile finds a failed step or an unlaunched step, or the user marks it resolved after inspection
    UnknownOutcome --> Verifying: reconcile finds evidence of completed work
    UnknownOutcome --> Cancelled: step record shows nothing was launched and cancel was requested
```

The transition function is a pure `fn next(OperationState, Event) -> Result<OperationState, IllegalTransition>` with a unit test. Terminal states: Succeeded, Failed, Cancelled. A server with an operation in Pending, Running, Verifying or UnknownOutcome accepts no new plan.

### Durable execution: recommendation

Evaluated options:

| Option | Survives SSH loss | Complexity | Verdict |
|---|---|---|---|
| Foreground exec over the channel | No. A closed channel may or may not kill apt | Lowest | Rejected for apt steps. Used for short checks |
| `nohup`/`setsid` plus status file | Yes | Needs a shell string or script, file cleanup, own bookkeeping | Rejected |
| Transient systemd unit (`systemd-run`) | Yes, owned by systemd | Moderate, all argv, state held by systemd | Recommended |

Durable step protocol (Ubuntu 24.04 has systemd 255; behaviour to be confirmed in live verification):

1. Persist the step with `unit_name = ossiframe-op-<full operation UUID>-<idx>` and status Running **before** launch. The name is deterministic and unique per operation, so reconcile can find the unit without any stored remote state.
2. Launch (privileged, 15 s timeout): `/usr/bin/systemd-run --unit=<name> --service-type=exec --remain-after-exit --quiet -E DEBIAN_FRONTEND=noninteractive -E NEEDRESTART_SUSPEND=1 -- /usr/bin/apt-get ...`. `--remain-after-exit` keeps the finished unit loaded so its result can be read; without it a successful transient unit is garbage-collected and its exit status lost.
3. Poll every 3 s with `systemctl show <name> --property=LoadState,ActiveState,SubState,Result,ExecMainStatus` (unprivileged). Done when `Result` is set and the unit is `active/exited` or `failed`. Success only if `Result=success` and `ExecMainStatus=0`.
4. Every 15 s and at completion run `sudo -n /usr/bin/journalctl -u <name> -o cat --no-pager -n 200` and replace the stored output (journal read needs privilege on non-`adm` users).
5. Persist the terminal step result first (exit status, end time, bounded log). Only then clean up, and only a unit confirmed finished: `reset-failed` for `ActiveState=failed`, `systemctl stop` for `active/exited` with no running process. Never stop, kill or signal a unit that may still run apt or dpkg. Cleanup is best effort and non-fatal.
6. A poll that times out is retried up to 3 consecutive times. This retries only read-only polls, never the mutation.

Behaviour on disconnect, step deadline, launch timeout, identity mismatch or app crash: the step and operation become `UnknownOutcome`. The engine never relaunches. A client-side deadline or cancel means the engine stops waiting and reports "may still be running". It never signals apt, dpkg or the unit. Before each step launch, poll batch, reconcile and resolution, the engine compares the connection identity (server id, endpoint, pinned host key) with the operation's snapshot and stops sending commands on a mismatch.

**Reconcile.** `provision_reconcile(operationId)` collects evidence read-only: the recorded step history, `systemctl show` on the recorded unit if present, dpkg health from the same unprivileged `dpkg-query` listing, apt and dpkg processes and lock holders, `dpkg-query` on the plan's packages, service state. A missing transient unit does not prove the step never ran (the unit may have been collected, or the host rebooted), so it is never sufficient for Failed. Decision table:

| Evidence | Result |
|---|---|
| Recorded unit active | Stay UnknownOutcome ("still running") |
| Unit finished, `Result=success` and `ExecMainStatus=0` | Record that step as completed. All steps completed gives Verifying. Steps remaining stays UnknownOutcome (never resumed). A completed `apt-update` says nothing about `apt-install`, and a completed step never implies plan success |
| Unit finished failure | Step and operation Failed, with the capped journal tail |
| Unit missing and the local record shows the step was never launched (still Pending) and earlier steps are terminal | Failed "interrupted before step N", based on the step record |
| Unit missing and the step was recorded as launched | Inconclusive alone. For `apt-install`: dpkg health listing settled, no apt or dpkg process and every plan package `ii` gives Verifying (verification decides success). Otherwise stay UnknownOutcome and show the evidence. For `apt-update` it stays UnknownOutcome (list state cannot be proven) |
| Evidence cannot be collected | Stay UnknownOutcome |

Reconcile never mutates, launches, stops or kills anything. Continuing a half-finished plan means creating a new plan from fresh preflight, which is idempotent because installed packages are skipped.

**Mark resolved after inspection.** `provision_mark_resolved(operationId)` lets the user close an UnknownOutcome operation that reconcile cannot settle. The backend re-collects evidence itself and allows it only when no apt or dpkg process exists, the dpkg and apt lock files are free (`sudo -n /usr/bin/fuser` on the lock files, read-only) and no `ossiframe-op-*` unit is active. It then stores the user decision, the time and the evidence in `operation_events` and `resolution_json`, sets Failed with resolution `markedResolved`, and releases the remote lock if this operation owns it. It does not retry, resume, repair or verify. The next step is a fresh preflight and a new plan. Error text for an interrupted dpkg tells the user which commands to run manually (for example `dpkg --configure -a` after checking no apt is running), and Ossiframe never runs them.

Foreground steps (verification, short probes) use `Executor::run` with 30 s timeouts. Failures there also end in UnknownOutcome if the connection drops mid-run, otherwise Failed.

### Cancellation and locking

- `provision_cancel(operationId)` sets `cancel_requested` and signals the engine. The engine checks the flag before launching each step. Between steps it moves to Cancelled and marks remaining steps skipped. During a step it never stops apt or dpkg or the unit; the request is recorded, shown as "cancel requested, waiting for current step", and honoured after the step ends. The shared `cancel_operation` command also trips the session token passed by `with_connection`. The engine treats that token as the same cancel request and does not feed it to the polling commands.
- Engine commands use `ServerService::with_connection` per command (launch, each poll), not one long call. The per-profile guard is therefore held only briefly. `disconnect_server` during an operation is allowed and results in UnknownOutcome, matching "user abandoned the session".
- **Local lock:** an in-memory per-server set plus the persisted rule that blocks new plans while any operation is non-terminal or UnknownOutcome.
- **Remote lock:** see "Remote exclusion lock". The native apt and dpkg locks stay in force, and the preflight unit scan is advisory.

### Remote exclusion lock

Mechanism (one chosen): the transient unit **`ossiframe-provision-lock`**. systemd refuses to create a second unit with an existing name, so acquisition is atomic on the host without a lock file, directory or stdin. The unit belongs to PID 1, so it survives SSH loss and desktop crashes. Alternatives rejected for S04: `flock` on a file under `/run` (needs a directory and file creation, and an ownership record would need a file write or stdin) and the preflight unit scan alone (racy).

```text
sudo -n /usr/bin/systemd-run --unit=ossiframe-provision-lock --service-type=exec --quiet \
  --description='ossiframe-lock op=<operation uuid> app=dev.ossiframe.deploy desktop=<installation id> started=<rfc3339>' \
  -- /usr/bin/sleep infinity
```

- **Ownership record:** the unit description. It is built only from a validated UUID, the fixed app id, the Store's random installation id and a timestamp. It is parsed strictly when read back (`systemctl show -p Description,ActiveState,SubState`).
- **Lifecycle:** acquired after approval and recheck, before Pending becomes Running. Held through all steps and verification. Released (stop and `reset-failed` of the holder, which only runs `sleep`) at Succeeded, Failed or Cancelled. Kept at UnknownOutcome. A step unit runs separately from the holder.
- **Contention:** a second start fails. The engine returns `busy` with the parsed owner. It never stops or removes a lock whose record names another operation.
- **Stale handling:** a holder whose operation is not a non-terminal operation in the local Store (another desktop, or a lost database) is reported as possibly stale with the owner details. `provision_release_stale_lock(serverId)` is an explicit user action. The backend allows it only after its own evidence shows no active `ossiframe-op-*` unit, no apt or dpkg process and free lock files, and it records the decision and evidence. A holder owned by a local UnknownOutcome operation is released through mark-resolved.
- **Unavailable:** if `systemd-run` is missing or the result cannot be classified, mutating steps are refused.
- **Limitations (stated in docs and UI, no concurrent-safety claim beyond them):** the lock is cooperative, so it only excludes other Ossiframe instances; a human running apt or another tool is excluded only by apt and dpkg's own locks; a host reboot clears the unit and any running step, after which state is judged from evidence; unit-name uniqueness behavior is confirmed in live verification before any claim in docs.

### Events

`OperationEvents` trait (Tauri-free, like `SessionEvents`) with `operation_updated(server_id, operation_id, status)`. The Tauri glue emits `operation-updated`. The UI refetches via `provision_get_operation`. The task runs on the Tokio runtime and writes to SQLite on state changes and on output refresh only.

## Data Models

In-memory and IPC models are the `Plan`, `PreflightReport`, `CheckResult` and `OperationState` types above and in the IPC section. Persisted models are below (migration 2, unconditional so Doctor and Deploy share one schema history; the tables stay empty and unused in Doctor).

```sql
CREATE TABLE operations (
  id TEXT PRIMARY KEY,
  server_id TEXT REFERENCES servers(id) ON DELETE SET NULL,   -- history survives profile removal
  server_name TEXT NOT NULL, server_host TEXT NOT NULL, server_port INTEGER NOT NULL,
  server_user TEXT NOT NULL, host_key_sha256 TEXT NOT NULL,   -- identity snapshot
  kind TEXT NOT NULL, plan_digest TEXT NOT NULL, plan_json TEXT NOT NULL,
  status TEXT NOT NULL, cancel_requested INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL, started_at TEXT, finished_at TEXT,
  error TEXT, verification_json TEXT, resolution_json TEXT);
CREATE TABLE operation_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  operation_id TEXT REFERENCES operations(id) ON DELETE CASCADE,  -- NULL for stale-lock release
  server_label TEXT NOT NULL, at TEXT NOT NULL, kind TEXT NOT NULL,
  from_state TEXT, to_state TEXT, unit_name TEXT, detail TEXT);   -- detail bounded and plain text
CREATE TABLE operation_steps (
  operation_id TEXT NOT NULL REFERENCES operations(id) ON DELETE CASCADE,
  idx INTEGER NOT NULL, step_id TEXT NOT NULL, title TEXT NOT NULL,
  unit_name TEXT, status TEXT NOT NULL, started_at TEXT, finished_at TEXT,
  exit_code INTEGER, output TEXT, PRIMARY KEY (operation_id, idx));
CREATE INDEX operations_server ON operations(server_id, created_at);
```

No secret column exists (the S01 schema test is extended). `output` is best-effort redacted of known patterns (S04 sends no secret to servers; no claim of universal redaction), valid UTF-8, capped at 64 KiB per step (the last 200 journal lines), and rendered as plain text. `error` is capped with `ipc::cap`. `operation_events` records every state transition, remote unit ids and recovery decisions, bounded to 200 events per operation. `plan_json` contains no secrets by construction. A `meta` row holds a random installation id (the desktop identity in the lock record). On startup the engine marks every Pending/Running/Verifying operation `UnknownOutcome`, because the app died and the remote unit may still be running. Progress reaches the UI through the existing polling and `operation-updated` event; there is no streaming infrastructure.

### Profile protection and identity

- `ServerService` (shared by Doctor and Deploy, no feature needed because the tables are always present) refuses `remove_server`, a change of host, port, username or authentication configuration in `update_server`, and `replace_host_key` while the server has an operation in Pending, Running, Verifying or UnknownOutcome. The error is `plan { serverInUse }`. A display-name change is allowed.
- Removing a profile with only terminal operations keeps those operations with a null `server_id` and the snapshot intact.
- An approved plan or operation can never be redirected by a profile change: approval binds to the snapshot, and the engine compares the live connection identity with the operation's snapshot before every step launch, poll batch, reconcile and resolution.

## Configuration safety primitive: deferred to S05

S04 changes no config files: NGINX, PHP-FPM, MySQL and Redis run with distro defaults, and `ossiframe-*` managed files do not exist yet. The primitive (backup, write through stdin with fixed argv such as `sudo -n install -m 0644 -o root -g root /dev/stdin <path>`, validate with `nginx -t` or `php-fpm8.3 -t`, reload, verify, restore on failure) has no S04 caller, so it is not built now. S05 (first vhost) builds it together with the stdin field on `CommandSpec`, with the unit tests listed in ADR-0005: managed prefix, never overwrite unmanaged files, rollback on failed validation. This keeps S04's attack surface at argv-only commands. S04 also verifies, and does not edit, that MySQL binds to loopback.

## Post-install verification

Run in the `Verifying` state through `Provisioner::verify`. Each check yields `CheckResult { component, id, title, essential, status: Pass|Fail|Warn|Skipped, detail }`. Details are capped at 200 characters and rendered as text. A component is **verified** only if every essential check is `Pass`. An essential check that is `Fail`, `Skipped` or unavailable makes the component not verified. An optional check that does not pass gives `Warn` or `Skipped` and a recorded limitation, and does not fail the component. There are no scores. The operation is Succeeded only if every selected component is verified, otherwise Failed. Results and verdicts are stored in `operations.verification_json`.

| Component | Essential checks | Optional checks |
|---|---|---|
| all | every package `ii` through `dpkg-query` | |
| git | `/usr/bin/git --version` runs | |
| composer | `/usr/bin/composer --version --no-ansi` starts with `Composer version 2.` | |
| nginx | `systemctl is-active nginx`; `sudo -n /usr/sbin/nginx -t`; bounded local request when `/usr/bin/curl` exists: `curl -sS --max-time 5 -o /dev/null -w '%{http_code}' http://127.0.0.1/` returns any HTTP status (recorded; the default site is expected to return 200), while a connection failure or timeout is `Fail` | `/usr/sbin/nginx -v`. If curl is absent the HTTP check is `Skipped` and non-essential with the limitation "serving not checked" |
| php | `/usr/bin/php -v` is 8.3; `php -m` contains ctype, curl, dom, fileinfo, mbstring, openssl, pdo, pdo_mysql, pdo_sqlite, tokenizer, xml, zip, bcmath, intl, gd | |
| php-fpm | unit `php8.3-fpm` active; `/usr/bin/test -S /run/php/php8.3-fpm.sock`; `sudo -n /usr/sbin/php-fpm8.3 -t`; `php-fpm8.3 -m` contains the same modules (FPM and CLI can load different modules, so the FPM SAPI is asked) | `php-fpm8.3 -v` |
| mysql | unit `mysql` active; local connection `sudo -n /usr/bin/mysql --no-defaults -N -B -e 'SELECT 1'` returns 1 over root `auth_socket`; inventory sockets show 3306 and 33060 bound only to loopback (an unreadable listing is `Fail`) | `/usr/sbin/mysqld --version` |
| redis | unit active; 6379 bound to loopback only; `redis-cli ping` returns `PONG` | `redis-server --version` |

The mysql local connection uses `mysql -e 'SELECT 1'` instead of `mysqladmin ping` because `mysqladmin ping` can exit 0 when access is denied, so it would not prove an authenticated local connection. It requires the root `auth_socket` default and is checked live. If that default differs on the verified image, the check fails and the finding is recorded rather than worked around by editing MySQL.

## IPC surface and desktop changes

`PROVISION_COMMAND_NAMES` (ten commands; feature `tauri` + `provisioning`), all in `ossiframe-desktop::provisioning::commands`:

| Command | Arguments | Returns |
|---|---|---|
| `provision_components` | none | `ComponentInfo[]` |
| `provision_preflight` | `serverId` | `PreflightReport` |
| `provision_create_plan` | `serverId`, `components: string[]` | `PlanDto` (id, digest, expiresAt, plan body) |
| `provision_approve_plan` | `serverId`, `planId`, `planDigest` | `operationId` (runs in the background) |
| `provision_cancel` | `operationId` | `void` |
| `provision_reconcile` | `operationId` | `OperationDto` (includes collected evidence) |
| `provision_mark_resolved` | `operationId` | `OperationDto` (only when backend evidence allows) |
| `provision_release_stale_lock` | `serverId` | `void` (only when backend evidence allows) |
| `provision_get_operation` | `operationId` | `OperationDto` (steps, bounded output, verification) |
| `provision_list_operations` | `serverId`, `limit?` (max 50) | `OperationSummaryDto[]` |

All inputs are validated at the boundary (UUID ids via the existing `is_uuid`, component ids through `ModuleId`-style parsing, digest as 64 lowercase hex). `IpcError` gains one variant, `plan { reason, message }`, with reasons `unknownPlan, superseded, expired, mismatch, wrongServer, busy, serverInUse, blocked, rePlanRequired, notSupported`. The TypeScript `KINDS` set is extended to match. Output sent to the UI is already redacted and capped.

Desktop additions: `provisioning/{plans,engine,store_ops,commands}.rs`, `Store` methods for the two tables (feature-gated), `ServerService::with_connection` unchanged. `tauri_support::setup` and `AppSetup` are not modified (Doctor and the starter still compile as is); apps opt in by calling `provisioning::setup(app, provisioner)` after `setup`.

## Deploy app (`apps/deploy`)

Independent of Doctor; workspace member `apps/deploy/src-tauri`. Identifier `dev.ossiframe.deploy` (separate data directory and keychain service), database `deploy.sqlite3`. Registry: `LinuxInventory`, `UbuntuProvisioning`. It does not depend on `linux-audit` or Doctor.

- `lib.rs`: `Registry::builder().register(LinuxInventory).register(UbuntuProvisioning)`, `setup(...)`, then `provisioning::setup(app, Arc::new(UbuntuProvisioning))`. The handler lists the 11 shared commands plus 10 `provision_*` commands. `build.rs` and `capabilities/main.json` list the same. Unit tests mirror Doctor's: capability and build manifest equal `COMMAND_NAMES + PROVISION_COMMAND_NAMES`; no `dialog:` or `fs:` permissions.
- Frontend: `ServerWorkspace` shell with tabs Overview (inventory UI), Preflight, Software, Plan, Operation, History. These pages are exported from `modules/ubuntu-provisioning/ui/` so generated apps that opt in get them too. Page behaviour:
  - Software shows each component's state from the latest preflight: "Installed <version>", "Not installed", "Partly installed" or a package needing repair, plus "Update available → <candidate>" as information only (candidate versions come from `apt-cache policy` on the server's current package lists, which may be stale; there is no upgrade action). Components that are not fully installed are ticked by default; installed components and Redis are unticked, so the user ticks only what to add. Without a preflight the page asks the user to check the server first and offers the button. Blockers of the latest preflight are listed inline, and plan creation is disabled while any exist. The primary button is "Review install plan". A successful approval clears the cached preflight, since the server changed. The component list carries each component's package names (`ComponentInfo.packages`) and the report carries `candidateVersions`, both typed in `@ossiframe/client`.
  - Plan shows packages, services, steps, resources, privileges, limitations and the expiry countdown. The digest appears in a technical-details section and in history. Approval opens a confirmation dialog that lists the target server (identity snapshot), selected components, packages to install, expected service changes, the root privilege requirement, resource requirements, recovery limitations, and that APT may install additional dependencies and run maintainer scripts. An explicit approve action for that plan is enabled only while it is unexpired. The backend executes only the stored approved plan, and the frontend never submits commands or plan contents. The UI states that the digest does not protect against a compromised frontend.
  - Operation shows step status, bounded log text rendered as plain text, "cancel requested" state, the lock owner when busy, and the per-component verification table (essential and optional checks). For UnknownOutcome it shows Reconcile with the collected evidence and "Mark resolved after inspection" (enabled only when the backend allows it), and states that nothing is retried automatically and that Ossiframe's lock excludes only other Ossiframe instances.
  - No server is bound to a runtime, there is no Laravel functionality, and more components can be installed later through a new plan.
  - Nothing mutates unless a plan was explicitly approved. No mock data.
- `@ossiframe/client` gains `provisioning.ts` with typed wrappers and types, exported separately from `api`, so Doctor does not import it.

## Developer reuse

- `tools/ossiframe-cli`: `CatalogEntry` gains `privileged: bool`. `ubuntu-provisioning` is `true`. `new --modules ubuntu-provisioning` fails with a clear message unless `--enable-provisioning` is also given. Without the flag the starter stays read-only and unchanged. With it, the generated app adds the `provisioning` feature on `ossiframe-desktop`, the `provisioning::setup` call, the ten commands in handler, `build.rs` and capability, and the matching drift tests. Without the flag, the default starter stays read-only. How these blocks are injected follows what `render.rs` already does for module registration and is settled in implementation. License notices are preserved in generated output. The opt-in app is built outside the repository in local mode (path dependencies on this repository) for verification, and against a published revision only after the user publishes.
- Docs updated: `docs/architecture.md` (execution and operations), `docs/security.md` (privilege table, enforcement layers), ADR-0004 and ADR-0005 "open questions" get resolved notes, `docs/compatibility.md` (24.04 components after verification), `docs/developer-guide.md` (opt-in), `docs/roadmap.md`. Evidence goes in `docs/verification/s04-infrastructure-provisioning.md`.

## Error handling

| Scenario | Response |
|---|---|
| Sudo not passwordless | Plan blocked. Message names the exact state and says passwordless `sudo -n` is required. No prompt, no password path |
| Blocking conflict (existing apache, mariadb, custom sites, other listeners) | Plan creation refused, conflicts listed. Doctor can inspect the server |
| Plan expired, superseded, altered or for another server | `plan` error. The plan is gone. User re-plans |
| Drift at recheck | `rePlanRequired` with reasons. Nothing executed |
| Apt lock held (unattended-upgrades) | Warning at plan time. The install waits up to 300 s through `DPkg::Lock::Timeout`. Beyond the step deadline: UnknownOutcome, reconcile. Apt is never terminated |
| Step exits non-zero | Step and operation Failed with capped journal tail. No rollback, no package removal, no automatic `dpkg --configure -a`. The message tells the user the manual recovery options and that a fresh preflight and plan follow |
| Disconnect or crash mid-step | UnknownOutcome. No retry. Reconcile after reconnect, then mark resolved if reconcile stays inconclusive |
| Unknown package source, or candidate or dependency from a non-allowlisted origin | Blocker at preflight, or gate failure before `apt-install`. Sources are never modified. The message names the source and says to fix it manually and re-plan |
| Remote lock held by another operation | `busy` with the parsed owner. Stale locks need an explicit user action backed by evidence |
| Profile removal or material edit during an unresolved operation | Refused with `serverInUse`. Display-name change allowed |
| Verification failure | Operation Failed with the failing essential checks. Installed packages stay |
| Server profile removed after the operation ended | History stays with a null server reference and the identity snapshot |

## Security considerations

- Privilege: `sudo -n` argv only, absolute program paths, no shell strings, no stdin and no secrets in S04. The connection user's passwordless sudo is root-equivalent and the UI and docs say so.
- No credential or secret is stored, sent over IPC or logged. Plan JSON, outputs and errors contain none. Remote output is untrusted: capped, passed through best-effort redaction of known patterns (no claim of universal redaction), stored and rendered as plain text.
- No SSH, firewall or user changes. Packages come only from the Ubuntu archive. No PPAs and no network-fetched scripts.
- The host key pin is part of plan binding. A changed key blocks connect (S01) and invalidates plans.
- Never run commands against servers from hooks. Development testing uses only the disposable server the user names.

## Correctness Properties

*A property is a characteristic or behavior that should hold true across all valid executions of the system. Per project rules, these are checked with small plain unit tests over representative and hostile inputs. No property-testing framework is added.*

### Property 1: Privileged rendering keeps words intact

For any privileged `CommandSpec` with an absolute program and any argument strings, the rendered command line starts with `'/usr/bin/sudo' '-n' '--'` and each argument is exactly one quoted word. A relative program is rejected.

**Validates: Requirements 1.2, 1.3, 1.4, 1.6**

### Property 2: Read-only guard rejects privilege

For any `CommandSpec` with Privilege `Root` submitted through `ReadOnly`, the result is `PrivilegeDenied` and the wrapped executor records zero runs. For any `User` spec, it is passed through.

**Validates: Requirements 2.6, 3.1, 3.2, 3.3**

### Property 3: Sudo classification follows the exit code

For any sudo probe result, a non-zero exit never classifies as Passwordless, and any classification other than Passwordless yields a Blocker.

**Validates: Requirements 2.1, 2.2, 2.3**

### Property 4: Blockers prevent plans

For any Preflight_Report with at least one Blocker, `canProvision` is false and plan building is refused. Unavailable, incomplete or truncated evidence yields a Blocker, an unknown listener owner is never treated as owned by itself (it is accepted only for an installed, active component of a server that is OssiframeManaged), and a server is classed OssiframeManaged only with a matching Succeeded operation record.

**Validates: Requirements 6.4, 6.7, 6.8, 7.2, 7.3, 7.4, 7.6, 7.7, 7.8, 7.9, 7.10, 7.11, 7.14, 9.7**

### Property 5: Selection resolution is closed and ordered

For any selection of known component ids, the resolved list contains every required component before its dependents. For any selection with an unknown id, resolution fails with `UnknownComponent`.

**Validates: Requirements 8.7, 8.8, 8.9**

### Property 6: Plans are deterministic and complete

For any equal report and selection, the Plan_Digest is equal. For any change to the package list, the digest changes. Installed packages never appear in install steps, and a plan with every package installed has zero steps.

**Validates: Requirements 9.2, 9.3, 9.6, 9.10, 9.11**

### Property 7: Approval invalidation

For any stored plan, approval succeeds only when the plan id matches, the plan is unexpired, the digest matches, and the server and pinned host key are unchanged. Each failing condition yields its own reason and nothing executes. A plan approves at most once, and a newer plan for the same server supersedes it.

**Validates: Requirements 10.1, 10.2, 10.3, 10.4, 10.5**

### Property 8: Drift forces re-planning

For any change to OS version, planned package states, blockers, sudo mode, the NGINX site list or package sources between plan and approval, or total RAM or free disk falling below the plan's minimum, recheck returns drift and no step runs. Changes in uptime, apt activity or memory above the minimum return no drift.

**Validates: Requirements 10.6, 10.7, 10.8**

### Property 9: State transitions stay within the diagram

For any pair of Operation_State and event, `next` either returns a state allowed by the state diagram or an `IllegalTransition`. Terminal states accept no event. Startup marks every non-terminal in-flight Operation UnknownOutcome.

**Validates: Requirements 11.3, 11.4, 11.5**

### Property 10: Interruption never relaunches

For any scripted disconnect, deadline or exhausted poll retry during a step, the Operation ends in UnknownOutcome and the fake executor records no second launch of that step. A cancel request before a later step yields Cancelled with that step never launched.

**Validates: Requirements 12.7, 12.8, 12.9, 14.3**

### Property 11: Reconcile is read-only and total

For any recorded evidence (unit active, finished success, finished failure, unit missing with a launched or never-launched step, evidence unavailable), reconcile maps to exactly one outcome from the decision table and issues no mutating command. A missing unit alone never yields Failed, inconclusive evidence stays UnknownOutcome, and a completed step never implies plan success. Mark-resolved is allowed only when no apt or dpkg process and no held lock file exist, and it never retries or resumes.

**Validates: Requirements 15.1, 15.2, 15.3, 15.4, 15.5, 15.6, 15.7, 15.8, 29.2, 29.3, 29.4**

### Property 12: Stored output is bounded and redacted

For any remote output, the stored step output is valid UTF-8, at most 64 KiB, and has the known secret patterns redacted (best effort, no claim of universal redaction). Events per operation are bounded.

**Validates: Requirements 13.1, 13.3, 13.4**

### Property 13: Success requires every essential check to pass

For any set of CheckResults, a component is verified if and only if every essential check has status Pass (Fail, Skipped and unavailable all count as not passing), and the Operation is Succeeded if and only if every selected component is verified. Optional checks never change the outcome.

**Validates: Requirements 17.3, 17.14, 17.15**

### Property 14: Command surfaces match exactly

For Doctor, the handler, `build.rs` and capability lists equal the expected command names and contain no `provision` name. For Deploy, they equal the shared plus provisioning commands. For a generated project without `--enable-provisioning`, no provisioning command appears.

**Validates: Requirements 5.1, 5.3, 5.4, 19.3, 20.4**

### Property 15: Remote lock exclusion and ownership

For any acquisition result, exactly one operation holds the lock, and a failed start returns `busy` with the parsed owner. For any ownership record naming another operation, release is refused. The lock is kept at UnknownOutcome and released at Succeeded, Failed and Cancelled. A stale holder is released only through the explicit action with clear evidence. Malformed or hostile description records are rejected.

**Validates: Requirements 26.2, 26.3, 26.5, 26.6, 26.7, 26.8**

### Property 16: Profile changes cannot redirect or erase history

For any server with an operation in Pending, Running, Verifying or UnknownOutcome, removal, host, port, username, authentication and host-key changes are refused, and a display-name change passes. Removing a profile with only terminal operations keeps them with a null reference and the snapshot. For any connection whose identity differs from the operation snapshot, the engine sends no further command.

**Validates: Requirements 27.2, 27.3, 27.4, 27.5**

### Property 17: Package sources stay inside the allowlist

For any set of enabled sources, planning is allowed only when every host is on the allowlist and every source file was read and parsed. For any candidate or install set with a non-allowlisted origin or a missing candidate, the gate fails before `apt-install`.

**Validates: Requirements 28.2, 28.3, 28.4, 28.5**

## Testing strategy

Small plain unit tests only (no property framework, no fixture or e2e suites):

- `command.rs`: privileged rendering (`'/usr/bin/sudo' '-n' '--' ...`), hostile args stay one word (extends the real-shell round trip), relative program rejected for privileged.
- `ReadOnly`: rejects `Root`, passes `User`; `dispatch::run_operation` with a probe module that attempts a privileged command is rejected and the fake connection records zero runs.
- `PlanStore`: expiry, supersede, digest mismatch, wrong server (endpoint or pinned key changed), single use, unknown plan.
- Plan determinism: same report and selection give the same digest; changing any package changes it.
- Plan building: dependency resolution (php-fpm pulls php), blocker refusal, mysql on 512 MB refusal, already-present skipped, zero-step plan.
- Preflight parsing: sudo classification, dpkg-query lines, nginx site classification (`default` and `ossiframe-*` allowed), listener conflicts.
- Preflight classification and sources: each `ServerClass`, package presence never implying OssiframeManaged, unknown listener owner and truncated evidence blocking, source allowlist for one-line and deb822 files.
- `next()` transition table and startup recovery marking in-flight operations UnknownOutcome. Reconcile decision table with scripted evidence (missing unit is not failure, inconclusive stays UnknownOutcome) and mark-resolved gating.
- Engine with a scripted fake executor: step Failed on exit 1; disconnect during poll gives UnknownOutcome with no relaunch and no stop or kill command; cancel before step 2 gives Cancelled; poll timeout retried 3 times then UnknownOutcome; terminal step result persisted before cleanup; identity mismatch stops commands.
- Remote lock logic: acquisition classification, ownership record parsing, foreign-owner release refused, stale handling, lock kept at UnknownOutcome.
- Verification outcomes: essential Fail, Skipped or unavailable make a component not verified, optional Warn does not.
- Store: migration idempotent, no secret column, output cap, history kept after profile removal, removal and material edits refused for non-terminal operations.
- Drift tests for Doctor and Deploy as above.

Required checks: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo check`, `pnpm typecheck`, `pnpm lint`, production builds of Doctor and Deploy, `shellcheck` for any scripts, generator build of an opt-in project outside the repo.

### Real-server verification plan (evidence in `docs/verification/s04-infrastructure-provisioning.md`)

| Target | Check |
|---|---|
| Existing Doctor Lightsail server (512 MB) | Read-only preflight only. Expect blockers (RAM for MySQL, existing configuration). It is never provisioned |
| Fresh disposable Ubuntu 24.04, about 2 vCPU and 4 GB, created by the user. Before any mutation the user confirms the host key fingerprint in the provider console and explicitly approves provisioning | Preflight, plan, approve, full default stack. Every essential check passes. Record package versions, candidate origins, `needrestart` observations and unit behaviour of `systemd-run --remain-after-exit` |
| Same server | Remote lock: a second plan or second desktop sees `busy` with the owner; the lock unit survives SSH loss and app exit; stale release needs explicit user action. Profile removal and host, port, user and key edits are refused during an operation |
| Same server, second run | Plan has zero install steps and verifies only (idempotency) |
| Same server | Edit state between plan and approval (install a package), expect `rePlanRequired`; wait out expiry, expect rejection; alter digest, expect rejection |
| Same server | Kill the SSH connection or app during `apt-install`: UnknownOutcome, unit keeps running, reconcile settles it from evidence or mark-resolved applies, no automatic retry |
| Fresh image right after boot | Observe unattended-upgrades holding the lock and the `DPkg::Lock::Timeout` wait; confirm NEEDRESTART settings do not break the run |
| Doctor build | Still exposes no provisioning commands; read-only inspection of the provisioned server |

Results are written only from real runs. If a check cannot be run, the doc says so.

## Performance considerations

Polling is 3 s with two cheap commands, journal refresh every 15 s, and output capped at 64 KiB per step. The engine holds no large buffers and does one SQLite write per state change or refresh. A 2 GB server is the verification target; the 8 GB desktop constraint is unaffected.

## Dependencies

- New Rust: `sha2` (optional, `ossiframe-desktop` feature `provisioning`). `tokio`, `serde`, `uuid`, `time`, `rusqlite` already present. The module crate adds nothing beyond `ossiframe-core`, `ossiframe-mod-linux-inventory`, `async-trait`, `serde`, `serde_json`, `tokio`.
- New workspace members: `modules/ubuntu-provisioning` (covered by `modules/*`), `apps/deploy/src-tauri`.
- Frontend: no new packages. Deploy uses the same Tauri, React, Tailwind and `@ossiframe/ui` set as Doctor.
- Server: Ubuntu 24.04 with systemd, `sudo`, `apt-get`, `dpkg`, `systemctl`, `journalctl` and the Ubuntu archive (`main` and `universe`). Nothing is installed on the server beyond the chosen packages. No agent.

## Material unresolved decisions

| Decision | Recommendation | Alternative | Resolve by |
|---|---|---|---|
| Sudo password support | Not in S04: require passwordless `sudo -n`, report clearly otherwise | `sudo -S -p ''` with the password over stdin, held in memory for one operation, optional keychain opt-in. Needs stdin plumbing in `SshExecutor` and redaction | Before S04 tasks; revisit in S05 with bootstrap |
| Durable execution method | `systemd-run --remain-after-exit` with polling | Foreground only (simpler, loses outcome on disconnect) | Live check of unit semantics on noble |
| `apt-get update` in the plan | Yes, a visible approved step | Run implicitly before install | Settled here |
| Redis in initial set | Optional, unchecked | Drop from S04 to cut one verification path | User |
| Composer source | Archive package 2.7.1-2 (universe) | Official installer with checksum verification (newer Composer, network code execution) | Settled here; revisit if Laravel needs Composer newer than 2.7 |
| Plan digest dependency | `sha2` | Random plan id only (no digest, no tamper check on stored history) | Settled here |
| Remote lock | Transient unit `ossiframe-provision-lock` (atomic by unit name, ownership in the description, survives SSH and app loss), plus native apt and dpkg locks | `flock` file under `/run` (needs file writes or stdin for ownership) | Live check that a second start fails with the same unit name on noble |
| Config write primitive and stdin | Defer to S05 | Build now with no caller | Settled here |
| `needrestart` handling | Pass `DEBIAN_FRONTEND=noninteractive` and `NEEDRESTART_SUSPEND=1` in the unit, make no "no restarts" claim | Leave default (may restart services mid-run) | Live check on fresh image; record what is observed |
| Engine location | Generic engine in `ossiframe-desktop` behind `Provisioner` trait in Core | Engine inside the module crate with a `tauri` feature depending on desktop (module to skeleton edge, S05 would duplicate it) | Settled here |
| Operation history when a profile is removed | Keep history with a null reference and identity snapshot; block removal while unresolved | Cascade delete (loses evidence) | Settled here |
| Provider mirrors on the source allowlist | Ubuntu-run hosts only at first; add provider mirrors after verification | Allow any mirror whose candidate origin passes `apt-cache policy` | User, when a provider mirror is seen in preflight |
