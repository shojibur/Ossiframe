# Requirements Document

## Introduction

S04 adds the first server mutations to Ossiframe: installing a base stack (Git, NGINX, PHP 8.3 CLI and FPM, Composer, MySQL, optional Redis) on a fresh Ubuntu 24.04 server. The flow is preflight, plan, explicit approval, execution, verification. A privilege on `CommandSpec`, a generic plan and operation engine in `ossiframe-desktop` (feature `provisioning`), a module crate `ubuntu-provisioning`, and a new app `apps/deploy` deliver it. Doctor stays read-only. These requirements are derived from `design.md`.

## Glossary

- **CommandSpec**: Core structured command (program, argv, timeout, privilege). The only way to run a remote command.
- **Privilege**: Core enum with values `User` and `Root`.
- **Executor**: Core trait that runs a CommandSpec on a server.
- **ReadOnly_Guard**: Core wrapper `ReadOnly` around an Executor that rejects any CommandSpec whose Privilege is not `User`.
- **Operation_Dispatcher**: Existing `dispatch::run_operation`, which runs module operations behind the ReadOnly_Guard.
- **Provisioner**: Core trait implemented by a module. Provides components, preflight, plan, recheck and verify.
- **Ubuntu_Provisioning_Module**: Crate `modules/ubuntu-provisioning` (`ossiframe-mod-ubuntu-provisioning`).
- **Component**: A provisioning unit (`git`, `nginx`, `php`, `php-fpm`, `composer`, `mysql`, `redis`) defined by packages, dependencies and expected units.
- **Preflight**: Read-only inspection of a server (apart from the sudo probe) that yields a Preflight_Report.
- **Preflight_Report**: Facts, Conflicts and a derived `canProvision` flag, with an observation time.
- **Conflict**: A finding with severity Blocker or Warning.
- **Plan**: Deterministic description of every change: target, OS, components, packages, files, services, resources, privileges, limitations, steps and preconditions.
- **Plan_Digest**: SHA-256 hex over the canonical JSON of the Plan.
- **Plan_Store**: In-memory desktop store holding at most one Plan per server.
- **Approved_Plan**: Type whose only constructor is the Plan_Store approval path.
- **Engine**: The generic plan, approval and operation engine in `ossiframe-desktop` behind the `provisioning` feature.
- **Operation**: A persisted execution of an Approved_Plan, with steps.
- **Operation_State**: One of Pending, Running, Verifying, Succeeded, Failed, Cancelled, UnknownOutcome.
- **Durable_Step**: A step run as a transient systemd unit named `ossiframe-op-<8 hex>-<idx>` through `systemd-run`.
- **Reconcile**: Read-only resolution of an UnknownOutcome Operation.
- **Provisioning_Commands**: The ten `provision_*` IPC commands.
- **Remote_Lock**: The transient systemd unit `ossiframe-provision-lock`, held on the server for the whole mutation lifecycle of one Operation. Its description is the ownership record.
- **Identity_Snapshot**: Server name, host, port, user and pinned host key SHA-256 captured in a Plan and in an Operation.
- **Server_Class**: Preflight classification of a server as Fresh, ExistingUnmanaged, OssiframeManaged or Partial.
- **Essential_Check**: A verification check whose Pass is required for its component to count as verified.
- **Source_Allowlist**: The explicit list of official Ubuntu archive hosts permitted as APT sources.
- **Doctor**: `apps/doctor`, the read-only application.
- **Deploy**: `apps/deploy`, the new provisioning application.
- **Generator**: `tools/ossiframe-cli`.
- **Starter**: The template that Generator renders.
- **Disposable_Server**: A fresh Ubuntu 24.04 instance (about 2 vCPU, 4 GB RAM) that the user creates and names.
- **Verification_Record**: `docs/verification/s04-infrastructure-provisioning.md`.

## Requirements

### Requirement 1: Privilege on CommandSpec

**User Story:** As a framework developer, I want privilege expressed in the structured command, so that root execution is explicit and never built from strings.

#### Acceptance Criteria

1. THE CommandSpec SHALL carry a Privilege, and `CommandSpec::new` SHALL produce Privilege `User`.
2. WHEN `CommandSpec::privileged` is called with an absolute program path, THE CommandSpec SHALL be created with Privilege `Root`.
3. IF `CommandSpec::privileged` is called with a program path that is not absolute, THEN THE CommandSpec SHALL return a ValidationError.
4. WHEN a CommandSpec with Privilege `Root` is rendered, THE CommandSpec SHALL produce a command line that starts with `'/usr/bin/sudo' '-n' '--'` (fixed sudo path) followed by individually quoted program and arguments.
5. WHEN a CommandSpec with Privilege `User` is rendered, THE CommandSpec SHALL produce a command line without a `sudo` prefix.
6. WHEN a hostile argument is passed to a privileged CommandSpec, THE CommandSpec SHALL render it as one quoted word.

### Requirement 2: Passwordless sudo only

**User Story:** As a server owner, I want Ossiframe to use only non-interactive sudo, so that no password prompt can hang or capture a credential.

#### Acceptance Criteria

1. THE Preflight SHALL run the fixed probe `/usr/bin/sudo -n -- /usr/bin/true` and classify the result as Passwordless, PasswordRequired, NotPermitted or Unavailable.
2. THE Preflight SHALL decide the classification from the exit code, and use English stderr text only as a hint for the message.
3. IF the sudo classification is not Passwordless, THEN THE Preflight_Report SHALL contain a Blocker that states that passwordless `sudo -n` is required.
4. WHEN `sudo -n` fails, THE Executor SHALL return without prompting for a password.
5. THE Engine SHALL send no sudo password and no stdin to any remote command in S04.
6. IF a privileged command is rejected, THEN THE Executor SHALL return `ExecError::PrivilegeDenied` and SHALL NOT run the command.
7. WHEN `ExecError::PrivilegeDenied` crosses IPC, THE IpcError mapping SHALL return `operationFailed` with a fixed message.

### Requirement 3: Read-only guard on module operations

**User Story:** As a Doctor user, I want read-only operations to be unable to mutate a server, so that inspection stays safe regardless of module metadata.

#### Acceptance Criteria

1. THE Operation_Dispatcher SHALL pass every operation an Executor wrapped in the ReadOnly_Guard.
2. IF a CommandSpec with Privilege `Root` is submitted to the ReadOnly_Guard, THEN THE ReadOnly_Guard SHALL return `ExecError::PrivilegeDenied` and the wrapped Executor SHALL record zero runs.
3. WHEN a CommandSpec with Privilege `User` is submitted to the ReadOnly_Guard, THE ReadOnly_Guard SHALL pass it to the wrapped Executor.
4. THE Ubuntu_Provisioning_Module SHALL register with zero `OperationInfo` entries.
5. THE S04 change SHALL leave the `Module` and `OperationInfo` types unchanged.
6. THE ReadOnly_Guard SHALL check only the Privilege field and SHALL NOT classify commands. S04 documentation SHALL NOT claim that the guard prevents all user-level mutation or prevents trusted Rust code from invoking sudo through its own executor.
7. THE Doctor read-only behavior SHALL derive from its registered modules with predefined read-only commands, the restricted typed IPC surface, and the absence of provisioning registration and setup, and SHALL be verified by command-list drift tests.

### Requirement 4: Privileged path only through approved plans

**User Story:** As a security reviewer, I want privileged execution reachable only after approval, so that no frontend or module can run root commands directly.

#### Acceptance Criteria

1. THE Engine SHALL execute privileged steps only from an Approved_Plan.
2. THE Plan_Store SHALL be the only code path that constructs an Approved_Plan.
3. THE Provisioning_Commands SHALL accept only server ids, component ids, plan ids, operation ids, a limit and a Plan_Digest, and SHALL accept no program, argv, path, privilege value or plan contents.
4. WHEN a Provisioning_Command receives an input, THE Engine SHALL validate ids as UUIDs, component ids as module-style identifiers and the Plan_Digest as 64 lowercase hex characters before use.
5. THE Provisioning_Commands SHALL exist only when `ossiframe-desktop` is built with features `tauri` and `provisioning`.
6. THE Provisioning_Commands SHALL have state only in an app that calls `provisioning::setup` with a Provisioner.
7. THE `tauri_support::setup` function and `AppSetup` SHALL remain unchanged so that Doctor and the Starter compile without the feature.
8. THE preflight sudo probe SHALL be the only privileged command executed before approval. Privileged commands after approval (steps, Remote_Lock, journal reads, reconcile and resolution evidence reads, verification) SHALL be tied to an Operation created from an Approved_Plan and SHALL use fixed argv.

### Requirement 5: Doctor remains read-only

**User Story:** As a Doctor user, I want proof that Doctor cannot provision, so that it remains safe on servers Deploy never touched.

#### Acceptance Criteria

1. THE Doctor handler list, `build.rs` manifest and capability file SHALL contain none of the Provisioning_Commands.
2. THE Doctor `Cargo.toml` SHALL name neither `ossiframe-mod-ubuntu-provisioning` nor the `provisioning` feature.
3. THE Doctor drift tests SHALL assert that the capability, `build.rs` and handler lists equal `COMMAND_NAMES` plus the report commands exactly.
4. THE Doctor drift tests SHALL assert that no permission name contains `provision`.
5. THE Doctor SHALL depend on neither the Ubuntu_Provisioning_Module nor Deploy.

### Requirement 6: Preflight facts

**User Story:** As an operator, I want a read-only preflight of the server, so that I see its state before any plan exists.

#### Acceptance Criteria

1. WHEN Preflight runs, THE Provisioner SHALL collect OS and architecture, memory, swap, free disk, connectivity and uid, sudo mode, package states, package candidates, dpkg health, apt and dpkg activity, reboot-required state, web server configuration, existing database and web infrastructure, listening ports and foreign `ossiframe-op-*` units.
2. THE Preflight SHALL run every command through the Executor with an absolute program path.
3. THE Preflight_Report SHALL contain the observation time, the facts, the list of Conflicts and `canProvision`.
4. WHEN a Conflict has severity Blocker, THE Preflight_Report SHALL set `canProvision` to false.
5. THE Preflight SHALL run no privileged command other than the sudo probe.
6. IF the preflight is cancelled or the connection drops, THEN THE Engine SHALL return an OperationError and SHALL create no Plan.
7. THE Preflight SHALL classify the server as Fresh, ExistingUnmanaged, OssiframeManaged (only when the Store holds a Succeeded Operation with no failed Essential_Check for the same Identity_Snapshot endpoint and pinned host key) or Partial (an earlier Failed, Cancelled or closed Operation for that identity). Package presence, file names or unit names SHALL NOT establish OssiframeManaged.
8. IF evidence needed for a Blocker rule is unavailable, incomplete (timeout or parse failure) or truncated at a cap, THEN THE Preflight_Report SHALL contain a Blocker for that rule.

### Requirement 7: Preflight blocking rules

**User Story:** As a server owner, I want provisioning blocked on unsupported or already-configured servers, so that Ossiframe never overwrites or blindly changes an existing setup.

#### Acceptance Criteria

1. IF the OS is Ubuntu 24.04, THEN THE Preflight SHALL treat the OS as supported on `x86_64` and `aarch64`.
2. IF the OS is Ubuntu 26.04, THEN THE Preflight_Report SHALL contain a Blocker that states the version is pending verification.
3. IF the OS is neither Ubuntu 24.04 nor Ubuntu 26.04, THEN THE Preflight_Report SHALL contain a Blocker for an unsupported OS.
4. IF RAM is below 900 MB and `mysql` is selected, THEN THE Preflight_Report SHALL contain a Blocker.
5. IF RAM is below 1.8 GB and `mysql` is selected, THEN THE Preflight_Report SHALL contain a Warning.
6. IF free disk on `/` is below the estimated need plus 1 GB, THEN THE Preflight_Report SHALL contain a Blocker.
7. IF any package on the server is half-installed, unpacked, half-configured, triggers-awaited, triggers-pending or reinstall-required in the unprivileged `dpkg-query` listing (installed, not-installed and config-files states are fine), or a planned package has a dpkg state other than absent or `ii`, THEN THE Preflight_Report SHALL contain a Blocker.
8. IF `apache2`, `mariadb-server` or `postgresql` is installed or active, or a PHP version other than 8.3 is installed, THEN THE Preflight_Report SHALL contain a Blocker.
9. IF an Apache site exists, or an NGINX entry other than `default` and names starting with `ossiframe-` exists, THEN THE Preflight_Report SHALL contain a Blocker.
10. IF ports 80, 443, 3306, 33060 or 6379 are held by a process that does not belong to a selected component, or by a process whose owner cannot be determined, THEN THE Preflight_Report SHALL contain a Blocker. Unknown ownership SHALL NOT count as ownership by the selected component.
11. IF an `ossiframe-op-*` unit or the Remote_Lock unit exists, THEN THE Preflight_Report SHALL contain a Blocker with the owner details when readable. The unit scan SHALL be advisory, and the Remote_Lock SHALL be authoritative.
12. IF apt or dpkg activity, a pending reboot, or a missing package candidate before `apt-get update` is observed, THEN THE Preflight_Report SHALL contain a Warning.
13. WHEN the Server_Class is OssiframeManaged or Partial, THE Preflight SHALL list planned packages already installed as already present and SHALL NOT report a Conflict for them. Partial SHALL yield a Warning and allow planning only when dpkg is healthy.
14. IF the Server_Class is ExistingUnmanaged (a service-bearing package, a non-default site or a listener on a managed port without an Ossiframe record), THEN THE Preflight_Report SHALL contain a Blocker, and THE Preflight SHALL provide no adoption path. Components without services or configuration (`git`, `composer`) MAY be preinstalled and SHALL NOT alone make a server ExistingUnmanaged.

### Requirement 8: Ubuntu provisioning module and components

**User Story:** As a framework developer, I want provisioning as a reusable module over generic engine contracts, so that Core stays free of stack knowledge.

#### Acceptance Criteria

1. THE Ubuntu_Provisioning_Module SHALL depend on `ossiframe-core` and `ossiframe-mod-linux-inventory` and SHALL declare `linux-inventory` in its manifest.
2. THE Provisioner trait and Plan types in `ossiframe-core` SHALL contain no NGINX, PHP, MySQL or Laravel knowledge.
3. THE Ubuntu_Provisioning_Module SHALL define the components `git`, `nginx`, `php`, `php-fpm`, `composer`, `mysql` and `redis` with their packages, requirements and expected units as in the design.
4. THE Ubuntu_Provisioning_Module SHALL use packages from the Ubuntu 24.04 archive only, with no PPA and no network-fetched script.
5. THE Ubuntu_Provisioning_Module SHALL mark `redis` as optional and unselected by default.
6. THE Ubuntu_Provisioning_Module SHALL use `git nginx php php-fpm composer mysql` as the default selection.
7. WHEN a selection is resolved, THE Ubuntu_Provisioning_Module SHALL add the required components in dependency order.
8. IF a selection contains an unknown component id, THEN THE Ubuntu_Provisioning_Module SHALL return `PlanError::UnknownComponent`.
9. THE Ubuntu_Provisioning_Module SHALL allow any subset of components, including NGINX without PHP and PHP without MySQL.
10. THE Ubuntu_Provisioning_Module SHALL attach no runtime-typed server profile to a server.
11. THE Ubuntu_Provisioning_Module SHALL implement PostgreSQL, Node.js, Supervisor and Certbot components in no S04 deliverable.

### Requirement 9: Plan contents

**User Story:** As a user, I want a complete plan before approving, so that I know exactly what will change and what cannot be undone.

#### Acceptance Criteria

1. THE Plan SHALL contain the target as an Identity_Snapshot (server id, name, host, port, user, pinned host key SHA-256), OS, resolved components, already-present components, packages with installed versions, files, services, resources, privileges, limitations, steps and preconditions.
2. WHEN a Plan is built, THE Ubuntu_Provisioning_Module SHALL omit packages already installed and sort the remaining packages.
3. WHEN at least one package is missing, THE Ubuntu_Provisioning_Module SHALL emit the durable step `apt-update`, the gate step `candidate-check` and the durable step `apt-install`, in that order.
4. THE `apt-install` step SHALL run `/usr/bin/apt-get install -y --no-install-recommends -o DPkg::Lock::Timeout=300` with the missing packages.
5. THE Plan SHALL list `apt-get update` as a visible step that the user approves.
6. IF every selected package is installed, THEN THE Ubuntu_Provisioning_Module SHALL produce a Plan with zero steps that results in a verify-only Operation.
7. IF the Preflight_Report contains a Blocker, THEN THE Ubuntu_Provisioning_Module SHALL refuse to build a Plan.
8. THE Plan SHALL contain an empty files list in S04.
9. THE Plan limitations SHALL state that no rollback exists, that a failed install can leave partial packages, that services start automatically and package scripts may restart services, that APT may install additional dependencies and run maintainer scripts, that no firewall change is made, that Ossiframe never runs `dpkg --configure -a`, removes packages or terminates apt or dpkg, and that the default NGINX page is reachable when port 80 is open.
10. THE Plan SHALL serialize to deterministic JSON with only ordered collections.
11. FOR ALL equal Preflight_Reports and selections, THE Ubuntu_Provisioning_Module SHALL produce Plans with equal Plan_Digests.

### Requirement 10: Approval binding

**User Story:** As a user, I want approval tied to exactly what I reviewed, so that a changed, stale or foreign plan cannot run.

#### Acceptance Criteria

1. THE Plan_Store SHALL hold at most one Plan per server, and a new Plan for a server SHALL supersede the earlier one.
2. THE Plan_Store SHALL expire a Plan 10 minutes after creation.
3. WHEN a Plan is approved, THE Plan_Store SHALL consume it so that it cannot be approved a second time.
4. WHEN `provision_approve_plan` is called, THE Engine SHALL check, in order, the plan exists, the plan id matches, the plan is not expired, the submitted Plan_Digest equals the stored and the recomputed Plan_Digest, the server record and pinned host key equal the Plan target, the server is connected, and no other Operation is active or UnknownOutcome for the server.
5. IF a check in criterion 4 fails, THEN THE Engine SHALL return a `plan` error with the matching reason (`unknownPlan`, `superseded`, `expired`, `mismatch`, `wrongServer`, `busy`) and SHALL execute nothing.
6. WHEN approval checks pass, THE Engine SHALL call `Provisioner::recheck` before creating the Operation.
7. IF `Provisioner::recheck` reports drift in OS version, planned package states, blockers, sudo mode, the NGINX site list or package sources, THEN THE Engine SHALL return `rePlanRequired` with the reasons and SHALL execute nothing.
8. THE recheck SHALL re-evaluate total RAM and free disk against the Plan resource needs and report drift when either is below its minimum, and SHALL NOT treat uptime, apt activity or memory above the minimum as drift.
9. WHEN a plan is read back from SQLite, THE Engine SHALL verify its Plan_Digest.
10. THE Engine SHALL execute only the stored Plan, never a Plan or commands supplied by the frontend.
11. WHEN recheck reports no drift, THE Engine SHALL acquire the Remote_Lock before creating the Operation, and IF acquisition fails, THEN THE Engine SHALL return `busy` with the owner details and execute nothing.

### Requirement 11: Operation records and states

**User Story:** As a user, I want every provisioning run recorded with a clear state, so that I can see what happened and never lose an unknown outcome.

#### Acceptance Criteria

1. WHEN an Operation is approved, THE Engine SHALL persist it in the `operations` and `operation_steps` tables through a migration shared by Doctor and Deploy.
2. THE `operations` and `operation_steps` tables SHALL contain no secret column.
3. THE Engine SHALL implement Operation_State transitions as a pure function that rejects transitions outside the design state diagram.
4. THE Engine SHALL reject a new Plan for a server that has an Operation in Pending, Running, Verifying or UnknownOutcome.
5. WHEN the application starts, THE Engine SHALL mark every Operation in Pending, Running or Verifying as UnknownOutcome.
6. WHEN an Operation changes state or refreshes output, THE Engine SHALL write the change to SQLite and emit `operation-updated` through the `OperationEvents` trait.
7. WHEN a server profile is deleted, THE Store SHALL keep its Operations, steps and events, set the server reference to null and retain the Identity_Snapshot.
8. THE Engine SHALL persist every state transition and every remote unit id as an operation event, bounded to 200 events per Operation.
9. THE Store SHALL keep a random installation id, used as the desktop identity in the Remote_Lock ownership record.

### Requirement 12: Durable execution

**User Story:** As a user, I want long apt steps to survive an SSH disconnect, so that a lost connection does not leave an unobserved half-run.

#### Acceptance Criteria

1. WHEN a Durable_Step starts, THE Engine SHALL persist the step with its unit name (`ossiframe-op-<full operation UUID>-<step index>`) and status Running before launching it.
2. THE Engine SHALL launch a Durable_Step with `/usr/bin/systemd-run --unit=<name> --service-type=exec --remain-after-exit --quiet`, `DEBIAN_FRONTEND=noninteractive` and `NEEDRESTART_SUSPEND=1`.
3. WHILE a Durable_Step runs, THE Engine SHALL poll `systemctl show` every 3 seconds for LoadState, ActiveState, SubState, Result and ExecMainStatus.
4. WHEN `Result=success` and `ExecMainStatus=0`, THE Engine SHALL mark the step succeeded.
5. WHEN the unit result is a failure, THE Engine SHALL mark the step and the Operation Failed.
6. THE Engine SHALL enforce a per-step deadline of 5 minutes for `apt-update` and 20 minutes for `apt-install`.
7. IF a poll times out, THEN THE Engine SHALL retry the poll up to 3 consecutive times and then set the Operation to UnknownOutcome.
8. IF the connection drops, a step deadline passes, a launch times out or the application crashes during a step, THEN THE Engine SHALL set the Operation to UnknownOutcome and SHALL NOT relaunch the step.
9. THE Engine SHALL NOT retry any mutating command automatically.
10. THE Engine SHALL NOT stop, kill or signal apt, dpkg or any unit that may still run them.
11. WHEN a step reaches a terminal state, THE Engine SHALL first persist the terminal step result (exit status, end time, bounded log), and only then clean up a unit confirmed finished (`reset-failed` for a failed unit, `systemctl stop` for an `active/exited` unit with no running process). Cleanup failure SHALL be non-fatal.
12. IF a foreground step exits non-zero, THEN THE Engine SHALL mark the step and the Operation Failed.
13. IF a client-side deadline passes or the user cancels while a step runs, THEN THE Engine SHALL stop waiting, report that the step may still be running, and SHALL NOT terminate apt, dpkg or the unit.
14. WHEN a gate step runs, THE Engine SHALL call `Provisioner::gate`, and IF it fails, THEN THE Engine SHALL fail the Operation before the next step.

### Requirement 13: Output handling

**User Story:** As a user, I want step logs that are safe to display, so that remote output cannot leak secrets or overload the app.

#### Acceptance Criteria

1. THE Engine SHALL store step output as the last 200 journal lines, redacted, valid UTF-8 and capped at 64 KiB per step.
2. WHILE a Durable_Step runs, THE Engine SHALL refresh the stored journal output every 15 seconds and once at completion.
3. THE Engine SHALL cap the Operation `error` text with `ipc::cap`.
4. THE Engine SHALL apply best-effort redaction of known secret patterns and a cap to output sent over IPC. S04 sends no secret to a server, and the documentation SHALL NOT claim universal redaction.
5. THE Engine SHALL write no raw SSH output to logs.
6. THE Deploy UI SHALL render remote output as plain text.
7. THE Engine SHALL reuse the existing polling and event mechanism and SHALL add no streaming infrastructure.
8. THE Engine SHALL bound stored output and events so that a long or noisy operation cannot grow the database without limit.

### Requirement 14: Cancellation

**User Story:** As a user, I want to cancel an operation safely, so that cancelling never corrupts the package database.

#### Acceptance Criteria

1. WHEN `provision_cancel` is called, THE Engine SHALL record the cancel request and signal the Engine task.
2. WHILE a step runs, THE Engine SHALL let the step finish and SHALL report "cancel requested, waiting for current step".
3. WHEN a step ends and a cancel is requested, THE Engine SHALL mark the remaining steps skipped and set the Operation to Cancelled before launching another step.
4. WHEN the shared `cancel_operation` command trips the session cancel token, THE Engine SHALL treat it as a cancel request and SHALL NOT pass the token to polling commands.

### Requirement 15: Reconcile

**User Story:** As a user, I want to resolve an unknown outcome from observed server state, so that I decide the next step without a blind retry.

#### Acceptance Criteria

1. WHEN `provision_reconcile` is called for an UnknownOutcome Operation, THE Engine SHALL collect evidence with read-only queries only: the recorded step history, the state of the recorded unit when present, dpkg package-state health from the unprivileged `dpkg-query` listing, apt and dpkg processes and lock holders, `dpkg-query` on the plan packages, service state, and verification checks.
2. WHILE the recorded unit is still active, THE Engine SHALL keep the Operation in UnknownOutcome and report that the step is still running.
3. WHEN the unit finished successfully, THE Engine SHALL record that step as completed, and WHEN all steps are completed, THE Engine SHALL set the Operation to Verifying. A completed step SHALL NOT imply plan success, and a completed `apt-update` SHALL NOT imply that `apt-install` ran.
4. WHEN the unit finished with a failure, THE Engine SHALL set the Operation to Failed with the capped journal tail.
5. IF the recorded unit is missing, THEN THE Engine SHALL NOT conclude that the step never ran and SHALL NOT set Failed on that fact alone. For a launched `apt-install` step it SHALL use the other evidence, and WHEN dpkg is healthy, no apt or dpkg process exists and every plan package is installed, it SHALL set Verifying.
6. IF the evidence is inconclusive or cannot be collected, THEN THE Engine SHALL keep the Operation in UnknownOutcome, list the evidence and show actionable recovery text.
7. IF the local step record shows that a step was never launched and all earlier steps are terminal, THEN THE Engine SHALL set Failed with the message "interrupted before step N".
8. THE Engine SHALL NOT launch, resume, stop, kill or signal anything during Reconcile.
9. THE Engine SHALL NOT resume a half-finished Plan, and recovery SHALL require a fresh preflight and a new Plan.

### Requirement 16: No configuration writes in S04

**User Story:** As a maintainer, I want S04 to change no configuration files, so that its attack surface stays at argv-only commands.

#### Acceptance Criteria

1. THE Engine SHALL write no file on the server in S04. The Remote_Lock is a transient systemd unit, not a file.
2. THE `CommandSpec` SHALL gain no stdin field in S04.
3. THE S04 change SHALL leave NGINX, PHP-FPM, MySQL and Redis on distro default configuration.
4. THE S04 deliverables SHALL include no backup, validate, reload or restore primitive for configuration files.
5. THE `docs/roadmap.md` SHALL record the configuration-write primitive and the stdin field as S05 work.

### Requirement 17: Post-install verification

**User Story:** As a user, I want success reported only after checking each installed component, so that a finished apt run is not mistaken for a working stack.

#### Acceptance Criteria

1. WHEN all steps finish, THE Engine SHALL set the Operation to Verifying and call `Provisioner::verify`.
2. THE Provisioner SHALL return a CheckResult with an `essential` flag and status Pass, Fail, Warn or Skipped for each check, with detail capped at 200 characters, and SHALL NOT compute scores.
3. THE Provisioner SHALL treat a component as verified only when all its Essential_Checks have status Pass. A Fail, Skipped or unavailable Essential_Check SHALL make the component not verified. A non-passing optional check SHALL yield Warn or Skipped and a recorded limitation without failing the component.
4. THE Provisioner SHALL treat, for every selected component, dpkg status `ii` of each package as an Essential_Check.
5. WHERE `nginx` is selected, THE Provisioner SHALL treat the active unit and `nginx -t` as Essential_Checks. WHEN `curl` exists, it SHALL run `curl -sS --max-time 5 -o /dev/null -w '%{http_code}' http://127.0.0.1/` as an Essential_Check that passes on any returned HTTP status (recorded) and fails on a connection failure or timeout. WHEN `curl` is absent, it SHALL mark that check Skipped and non-essential with a recorded limitation.
6. WHERE `php` is selected, THE Provisioner SHALL treat `php -v` reporting 8.3 and the required modules in `php -m` as Essential_Checks.
7. WHERE `php-fpm` is selected, THE Provisioner SHALL treat the active unit, the socket `/run/php/php8.3-fpm.sock`, `php-fpm8.3 -t` and the required modules in `php-fpm8.3 -m` as Essential_Checks.
8. WHERE `composer` or `git` is selected, THE Provisioner SHALL treat a successful version command as an Essential_Check, and for Composer the output SHALL start with `Composer version 2.`.
9. WHERE `mysql` is selected, THE Provisioner SHALL treat the active unit, a local connection through `sudo -n /usr/bin/mysql --no-defaults -N -B -e 'SELECT 1'` over `auth_socket`, and loopback-only binding of ports 3306 and 33060 as Essential_Checks. An unreadable socket listing SHALL be a Fail.
10. WHERE `redis` is selected, THE Provisioner SHALL treat the active unit, loopback-only binding of port 6379 and a `PONG` reply as Essential_Checks.
11. THE Provisioner SHALL treat service version output of nginx, mysqld and redis as optional checks.
12. THE Provisioner SHALL run only `nginx -t`, `php-fpm8.3 -t` and `mysql` privileged, and only inside the approved Operation.
13. THE Engine SHALL persist the CheckResults and component verdicts in `operations.verification_json`.
14. IF any selected component is not verified, THEN THE Engine SHALL set the Operation to Failed.
15. WHEN every selected component is verified, THE Engine SHALL set the Operation to Succeeded.

### Requirement 18: IPC surface

**User Story:** As a frontend developer, I want typed intent-level commands, so that the UI cannot reach a shell or arbitrary files.

#### Acceptance Criteria

1. THE Provisioning_Commands SHALL be `provision_components`, `provision_preflight`, `provision_create_plan`, `provision_approve_plan`, `provision_cancel`, `provision_reconcile`, `provision_mark_resolved`, `provision_release_stale_lock`, `provision_get_operation` and `provision_list_operations`.
2. WHEN `provision_list_operations` is called, THE Engine SHALL return at most 50 summaries.
3. THE IpcError SHALL contain a `plan` variant with reasons `unknownPlan`, `superseded`, `expired`, `mismatch`, `wrongServer`, `busy`, `serverInUse`, `blocked`, `rePlanRequired` and `notSupported`.
4. THE TypeScript error `KINDS` set in `@ossiframe/client` SHALL include the `plan` kind.
5. THE `@ossiframe/client` package SHALL export the provisioning wrappers from `provisioning.ts` separately from `api`.
6. THE Provisioning_Commands SHALL return typed DTOs and SHALL expose no generic shell, file or command-output channel.

### Requirement 19: Deploy app

**User Story:** As a user, I want a Deploy app that walks me through preflight, plan, approval and operation, so that provisioning is a deliberate flow.

#### Acceptance Criteria

1. THE Deploy SHALL be a workspace member at `apps/deploy/src-tauri` with identifier `dev.ossiframe.deploy`, a separate data directory, keychain service and database `deploy.sqlite3`.
2. THE Deploy SHALL register `LinuxInventory` and `UbuntuProvisioning` and SHALL depend on neither Doctor nor `linux-audit`.
3. THE Deploy handler list, `build.rs` manifest and capability file SHALL each equal the shared commands plus the Provisioning_Commands.
4. THE Deploy capability file SHALL contain no `dialog:` or `fs:` permissions.
5. THE Deploy UI SHALL provide the tabs Overview, Preflight, Software, Plan, Operation and History through `ServerWorkspace`.
6. THE Software tab SHALL list components with installed state and checkboxes, apply the default selection and leave `redis` unchecked.
7. THE Plan tab SHALL show packages, services, steps, resources, privileges, limitations and the expiry countdown, and SHALL show the Plan_Digest in a technical-details section and in history.
8. WHEN the user starts approval of an unexpired Plan, THE Deploy UI SHALL show a confirmation dialog that lists the target server (Identity_Snapshot), selected components, packages to install, expected service changes, the root privilege requirement, resource requirements, recovery limitations, and that APT may install additional dependencies and run maintainer scripts, and SHALL require an explicit approve action for that Plan. THE Deploy UI SHALL NOT require typing any part of the Plan_Digest.
9. WHILE an Operation is in UnknownOutcome, THE Operation tab SHALL show Reconcile and "Mark resolved after inspection" actions, the collected evidence, and a statement that nothing is retried automatically.
10. THE Deploy UI SHALL show the sudo requirement and that passwordless sudo is root-equivalent.
11. THE Deploy UI SHALL show no mock data.
12. THE Deploy UI SHALL mutate nothing until a Plan is explicitly approved.
13. THE page components SHALL be exported from `modules/ubuntu-provisioning/ui/`.
14. THE Deploy UI SHALL state that the Plan_Digest does not protect against a compromised frontend. THE Deploy frontend SHALL submit only ids and the Plan_Digest and SHALL NOT submit commands or Plan contents.
15. THE Deploy SHALL bind no server to a runtime, SHALL contain no Laravel functionality, and SHALL allow further components to be installed later through a new Plan.

### Requirement 20: Generator opt-in

**User Story:** As a third-party developer, I want provisioning only when I opt in, so that a generated app stays read-only by default.

#### Acceptance Criteria

1. THE Generator catalog SHALL mark `ubuntu-provisioning` as `privileged`.
2. IF `new --modules ubuntu-provisioning` is used without `--enable-provisioning`, THEN THE Generator SHALL fail with a message that names the missing flag.
3. WHEN `--enable-provisioning` is given, THE Generator SHALL add the `provisioning` feature, the `provisioning::setup` call, the ten commands in the handler, `build.rs` and capability, and the matching drift tests.
4. WHEN `--enable-provisioning` is not given, THE Generator SHALL render the Starter without any Provisioning_Command, `provisioning` feature or provisioning module.
5. THE verification SHALL build an opt-in project generated outside the repository in local mode (path dependencies on the repository). Verification against a published revision SHALL wait until the user publishes.
6. THE Generator SHALL preserve license notices in generated output.

### Requirement 21: Documentation

**User Story:** As a contributor, I want S04 documented where decisions live, so that later specs build on recorded facts.

#### Acceptance Criteria

1. THE S04 change SHALL update `docs/architecture.md` with execution and operations.
2. THE S04 change SHALL update `docs/security.md` with the privilege table and the enforcement layers.
3. THE S04 change SHALL add resolved notes to ADR-0004 and ADR-0005 open questions.
4. THE S04 change SHALL update `docs/developer-guide.md` with the opt-in steps and `docs/roadmap.md` with S04 status and the S05 deferrals.
5. THE S04 change SHALL update `docs/compatibility.md` with Ubuntu 24.04 component versions only from verified runs.

### Requirement 22: Real-server verification

**User Story:** As the project owner, I want S04 claims backed by real runs, so that the docs never state unverified behavior.

#### Acceptance Criteria

1. THE Verification_Record SHALL contain only output from real runs, and SHALL state any check that could not be run.
2. THE verification SHALL use only a Disposable_Server that the user creates and names, and the existing small Doctor server for read-only preflight only.
3. THE verification SHALL create no cloud resource automatically.
4. THE verification on the Doctor server SHALL run the preflight only and SHALL NOT provision or mutate it.
5. BEFORE any mutation on the Disposable_Server, THE user SHALL confirm the host key fingerprint in the provider console and give explicit approval to provision that server. THE verification SHALL then run preflight, plan, approval, the default stack and every verification check, and SHALL record package versions.
6. THE verification SHALL record the behavior of `systemd-run --remain-after-exit` on Ubuntu 24.04.
7. THE verification SHALL run a second plan on the same server and record that it has zero install steps and only verifies.
8. THE verification SHALL change server state between plan and approval and record `rePlanRequired`, wait out expiry and record the rejection, and alter the digest and record the rejection.
9. THE verification SHALL interrupt the SSH connection or application during `apt-install` and record UnknownOutcome, the unit continuing, Reconcile resolution and no automatic retry.
10. THE verification SHALL observe a fresh boot with unattended-upgrades holding the dpkg lock and record the `DPkg::Lock::Timeout` wait and the effect of `NEEDRESTART_SUSPEND=1`.
11. THE verification SHALL run Doctor against the provisioned server and record that Doctor exposes no provisioning command.
12. THE verification SHALL record the Remote_Lock behavior: a second acquisition attempt returns `busy` with owner details, the lock unit survives SSH loss and application exit, and stale handling requires an explicit user action.
13. THE implementation tasks other than live verification SHALL NOT depend on the Disposable_Server, and S04 SHALL be reported as implemented with live verification pending until the live checks are done.

### Requirement 23: Quality gates and unit tests

**User Story:** As a maintainer, I want the standard checks and targeted tests, so that high-risk logic is covered without heavy test suites.

#### Acceptance Criteria

1. THE S04 change SHALL pass `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo check`, `pnpm typecheck`, `pnpm lint`, production builds of Doctor and Deploy, and `shellcheck` for any script.
2. THE S04 change SHALL include unit tests for privileged rendering and hostile arguments, including a relative program path rejection.
3. THE S04 change SHALL include unit tests for the ReadOnly_Guard, including a probe module that attempts a privileged command through the Operation_Dispatcher with zero runs recorded.
4. THE S04 change SHALL include unit tests for Plan_Store expiry, supersede, digest mismatch, wrong server, single use and unknown plan.
5. THE S04 change SHALL include unit tests for Plan determinism, dependency resolution, blocker refusal, low-RAM MySQL refusal, already-present skipping and the zero-step Plan.
6. THE S04 change SHALL include unit tests for sudo classification, `dpkg-query` parsing, NGINX site classification and listener conflicts.
7. THE S04 change SHALL include unit tests for the Operation_State transition table, startup marking of in-flight Operations, the Reconcile decision table (missing unit is not failure, inconclusive evidence stays UnknownOutcome) and the gating of manual resolution.
8. THE S04 change SHALL include Engine unit tests with a scripted fake Executor for step failure, disconnect without relaunch, cancel between steps and poll retry.
9. THE S04 change SHALL include Store unit tests for migration idempotence, absence of secret columns and the output cap.
10. THE S04 change SHALL add no property-testing framework and no end-to-end suite.
11. THE S04 change SHALL include unit tests for Remote_Lock acquisition classification, ownership record parsing, foreign-owner release refusal and stale-lock handling.
12. THE S04 change SHALL include unit tests that profile removal and material edits are refused while an Operation is non-terminal and that history survives profile removal.
13. THE S04 change SHALL include unit tests that essential verification outcomes decide component and Operation results.

### Requirement 24: Non-goals

**User Story:** As a maintainer, I want explicit non-goals, so that S04 stays within scope.

#### Acceptance Criteria

1. THE S04 change SHALL implement none of: Laravel and application lifecycle, vhosts and FPM pools for applications, database users and databases, SSL and Certbot, firewall or SSH changes, multiple PHP versions and PPAs, Node, Python or container adapters, adoption of existing servers, monitoring, automatic cloud server creation.
2. THE S04 change SHALL add no sudo password support.
3. THE S04 change SHALL add no automatic repair (`dpkg --configure -a`, package removal), no modification of APT sources and no adoption of unmanaged servers.
4. THE S04 change SHALL add no dynamic plugin mechanism.

### Requirement 25: S04 completion criteria

**User Story:** As the project owner, I want clear completion criteria, so that I know when S04 is done.

#### Acceptance Criteria

1. THE S04 SHALL be complete only when the provisioning contracts live in Core and the Ubuntu_Provisioning_Module, so that Deploy, the Generator and later specs reuse them.
2. THE S04 SHALL be complete only when every mutation is shown in a Plan, approved by the user, bound to server, host key and Plan_Digest, and executed by the Engine alone.
3. THE S04 SHALL be complete only when Doctor still exposes no mutating or privileged command and its drift tests pass.
4. THE S04 SHALL be complete only when the default stack is provisioned on a Disposable_Server and every component check passes.
5. THE S04 SHALL be complete only when interruption during `apt-install` yields UnknownOutcome and Reconcile or manual resolution handles it from evidence without automatic retry.
6. THE S04 SHALL be complete only when the Deploy shell completes preflight, plan, approval, operation and history on the Disposable_Server.
7. THE S04 SHALL be complete only when a project generated with `--enable-provisioning` builds, and one generated without it exposes no provisioning command.
8. THE S04 SHALL be complete only when all quality gates in Requirement 23 pass.
9. THE S04 SHALL be complete only when the Verification_Record is honest and every documented claim is backed by a real run.
10. THE S04 SHALL be complete only when the Remote_Lock behavior is observed on the Disposable_Server.

### Requirement 26: Remote exclusion lock

**User Story:** As a user, I want a second desktop or a crashed app to be unable to start a conflicting mutation, so that two Ossiframe operations never overlap on one server.

#### Acceptance Criteria

1. BEFORE any mutating step, THE Engine SHALL acquire the Remote_Lock by starting the transient unit `ossiframe-provision-lock` through `sudo -n /usr/bin/systemd-run` with fixed argv.
2. THE acquisition SHALL be atomic, so that a second start of the same unit name fails, and THE Engine SHALL then return `busy` with the owner details read from the unit.
3. THE Remote_Lock unit description SHALL be the ownership record and SHALL contain the operation id, the app id, the desktop installation id and the start time, built only from validated values.
4. THE Remote_Lock SHALL be held by a remote unit and SHALL NOT depend on the SSH session or the desktop process, so that it survives SSH interruption and application crashes.
5. THE Engine SHALL hold the Remote_Lock through all steps and verification and SHALL release it only when the Operation reaches Succeeded, Failed or Cancelled, or through an explicit recovery action.
6. THE Engine SHALL keep the Remote_Lock while an Operation is UnknownOutcome.
7. THE Engine SHALL NOT stop or remove a Remote_Lock whose ownership record names a different operation.
8. WHEN a Remote_Lock owner is not a non-terminal Operation in the local Store, THE Engine SHALL report it as possibly stale with the owner details, and SHALL release it only through `provision_release_stale_lock`, which the Engine SHALL allow only after its own evidence shows no active `ossiframe-op-*` unit, no apt or dpkg process and free lock files, and SHALL record the decision and evidence.
9. THE Engine SHALL keep the native apt and dpkg locks in force, and THE preflight unit scan SHALL remain advisory.
10. THE documentation and the Deploy UI SHALL state that the Remote_Lock is cooperative among Ossiframe instances, that a host reboot clears it, and SHALL NOT claim protection against other tools or administrators.
11. IF the Remote_Lock cannot be acquired reliably because `systemd-run` is unavailable or the result cannot be classified, THEN THE Engine SHALL refuse to run mutating steps.

### Requirement 27: Server identity and operation history

**User Story:** As a user, I want operation history to stay accurate and a profile change to never redirect an approved operation, so that records and execution always refer to the server I approved.

#### Acceptance Criteria

1. THE `operations` table SHALL keep a nullable server reference and an Identity_Snapshot (name, host, port, user, host key SHA-256).
2. WHEN a server profile is removed, THE Store SHALL keep its Operations with a null server reference.
3. WHILE a server has an Operation in Pending, Running, Verifying or UnknownOutcome, THE ServerService SHALL refuse removal, a change of host, port, username or authentication configuration, and a pinned host key replacement with `plan` reason `serverInUse`, and SHALL allow a display-name change.
4. BEFORE each step launch, each poll batch, reconcile and resolution, THE Engine SHALL compare the connection identity (server id, endpoint, pinned host key) with the Operation Identity_Snapshot.
5. IF the identity differs, THEN THE Engine SHALL send no further command and SHALL set UnknownOutcome when a step may be running.
6. THE Engine SHALL scope one explicit connection identity to the whole Operation.
7. THE Preflight classification SHALL use the Identity_Snapshot endpoint and pinned host key of the current server.

### Requirement 28: Package sources and APT behavior

**User Story:** As a server owner, I want packages to come only from official Ubuntu archives, so that provisioning never installs from an unreviewed source.

#### Acceptance Criteria

1. THE Preflight SHALL read `/etc/apt/sources.list`, `/etc/apt/sources.list.d/*.list` and `*.sources` (deb822) without modifying them.
2. THE Source_Allowlist SHALL contain `archive.ubuntu.com`, `*.archive.ubuntu.com` (including cloud mirrors under it), `security.ubuntu.com` and `ports.ubuntu.com`, kept as data in the Ubuntu_Provisioning_Module. Other provider mirrors SHALL be added only after verification.
3. IF an enabled source host is outside the Source_Allowlist, THEN THE Preflight_Report SHALL contain a Blocker with an explanation, and THE Engine SHALL NOT modify sources.
4. IF a source file cannot be read or parsed, THEN THE Preflight_Report SHALL contain a Blocker.
5. AFTER `apt-get update`, THE `candidate-check` gate SHALL verify that every package has a candidate with an allowlisted origin and that `apt-get install -s` lists only packages from allowlisted origins, and IF not, THEN THE Engine SHALL fail the Operation before `apt-install`.
6. THE Plan SHALL list estimated transitive packages when package lists allow a simulation, and otherwise SHALL state that they are unknown until after the update. THE gate SHALL store the exact set in the step output.
7. THE Engine SHALL run APT steps with `DEBIAN_FRONTEND=noninteractive` and `NEEDRESTART_SUSPEND=1`, and THE documentation SHALL state actual `needrestart` behavior only from live verification and SHALL NOT claim that no service restarts.
8. THE Engine SHALL apply bounded deadlines to APT steps and SHALL NOT terminate apt or dpkg on cancel or client deadline.
9. THE Engine SHALL NOT run `dpkg --configure -a` or remove packages automatically.
10. THE failure messages SHALL give actionable recovery text that the user performs manually.

### Requirement 29: Manual recovery from UnknownOutcome

**User Story:** As a user, I want a safe way to close an unresolved operation after I have inspected the server, so that a server is not locked forever and nothing is retried blindly.

#### Acceptance Criteria

1. THE Engine SHALL expose `provision_mark_resolved(operationId)` only for Operations in UnknownOutcome.
2. WHEN `provision_mark_resolved` is called, THE Engine SHALL collect its own fresh evidence and SHALL allow the action only when no apt or dpkg process exists, the apt and dpkg lock files are free and no `ossiframe-op-*` unit is active.
3. WHEN the action is allowed, THE Engine SHALL record the user decision, the time and the evidence in the operation events and `resolution_json`, and SHALL set the Operation to Failed with resolution `markedResolved`.
4. THE Engine SHALL NOT retry, resume, repair or verify as part of the action.
5. WHEN the Remote_Lock is owned by that Operation, THE Engine SHALL release it as part of the action.
6. THE Deploy UI SHALL state that the action records the user decision, verifies nothing, and that the next step is a fresh preflight and a new Plan.
