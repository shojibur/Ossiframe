# Implementation Plan: Infrastructure Provisioning (S04)

## Overview

Implementation is Rust (Tokio, Tauri 2) plus TypeScript (React). The order is Core, then desktop engine, then the `ubuntu-provisioning` module, then the Doctor drift tests, then `apps/deploy`, then the generator opt-in and docs, then one quality gate, then live verification. Each task ends wired into the previous ones. The Correctness Properties from the design are invariants checked by small plain `#[test]` unit tests inside the implementation tasks. No property-testing framework, no end-to-end suite, no commit or push tasks.

`ApprovedPlan` is defined in `ossiframe-desktop` (task 2.3) with a private constructor reachable only from the plan store. Core cannot enforce a crate-private constructor for another crate, so Core defines the `Plan` and `Provisioner` contracts and desktop owns the approval type.

Tasks 1 to 7 and the draft of task 9 need no server. Only task 8 (live verification) depends on the user's disposable server. Implementation must not block on it; S04 is reported as "implemented, live verification pending" until task 8 is done.

## Tasks

- [x] 1. Core contracts (`crates/ossiframe-core`)
  - [x] 1.1 Add Privilege to CommandSpec
    - In `command.rs`: add `Privilege { User, Root }`, `CommandSpec::privileged(program, args, timeout)` (absolute program path required, otherwise `ValidationError`) and `privilege()`. `new` yields `User`.
    - `to_command_line()` prefixes `'/usr/bin/sudo' '-n' '--'` (fixed sudo path) only for `Root`. Every word stays individually quoted. Add no stdin field.
    - In `executor.rs`: add `ExecError::PrivilegeDenied`.
    - Unit tests (Property 1): privileged line starts with `'/usr/bin/sudo' '-n' '--'`; hostile arguments stay one quoted word (extend the existing real-shell round trip); relative program rejected; `User` has no sudo prefix.
    - _Requirements: 1.1, 1.2, 1.3, 1.4, 1.5, 1.6, 2.6, 16.2, 23.2_

  - [x] 1.2 Add the ReadOnly executor guard
    - In `executor.rs`: add `ReadOnly<'a>(&'a dyn Executor)`. `run` returns `PrivilegeDenied` for any spec whose privilege is not `User` without calling the wrapped executor, and passes `User` specs through. It checks the `Privilege` field only; no command classifier. Document in the type docs that it does not stop user-level mutation or trusted Rust code that builds its own executor. Export from `lib.rs`.
    - Unit tests (Property 2) with a recording fake executor: `Root` gives `PrivilegeDenied` and zero recorded runs; `User` is passed through.
    - _Requirements: 2.4, 2.6, 3.1, 3.2, 3.3, 3.6, 23.3_

  - [x] 1.3 Add provisioning contracts and the state machine
    - New `provision.rs`, exported from `lib.rs`: `OperationState` (seven states), pure `next(OperationState, Event) -> Result<_, IllegalTransition>` following the design diagram, `Plan` and its parts (`PlanTarget` as identity snapshot, `PackageChange`, `FileChange`, `ServiceEffect`, `Resources`, `PlanStep`, `StepMode { Foreground, Durable, Gate }`, `Preconditions`), `PreflightReport` with `ServerClass { Fresh, ExistingUnmanaged, OssiframeManaged, Partial }`, `Conflict`, `Severity`, `SudoMode`, `CheckResult` (with `essential`), `CheckStatus`, `ComponentVerdict`, `ComponentInfo`, `Drift`, `PlanError` (including `UnknownComponent`), and the `Provisioner` async trait (`components`, `preflight`, `plan`, `recheck`, `gate`, `verify`). Use ordered collections only so serialization is deterministic. No NGINX, PHP, MySQL or Laravel knowledge.
    - `PreflightReport::can_provision` is derived from the conflicts: false when any is a Blocker.
    - Leave `Module` and `OperationInfo` unchanged.
    - Unit tests (Property 9): every allowed transition passes, every other (state, event) pair returns `IllegalTransition`, terminal states accept no event; `can_provision` is false with a Blocker; a plan serializes to identical JSON twice.
    - _Requirements: 3.5, 6.3, 6.4, 6.7, 8.2, 9.10, 11.3, 17.2_

- [x] 2. Desktop engine (`crates/ossiframe-desktop`)
  - [x] 2.1 Wire the guard into dispatch and extend IpcError
    - In `dispatch::run_operation`, build the `OperationContext` executor as `ReadOnly(&*conn)`.
    - Map `ExecError::PrivilegeDenied` to `operationFailed` with a fixed message. Add the `IpcError::plan { reason, message }` variant with reasons `unknownPlan, superseded, expired, mismatch, wrongServer, busy, serverInUse, blocked, rePlanRequired, notSupported`.
    - Unit tests: a probe module that attempts a privileged command through `run_operation` is denied and the fake connection records zero runs; the privilege-denied mapping carries the fixed message; the `plan` variant serializes with a camelCase reason.
    - _Requirements: 2.7, 3.1, 3.2, 18.3, 23.3_

  - [x] 2.2 Operations migration, store methods, profile protection and recovery
    - Add migration 2 (unconditional, shared by Doctor and Deploy): `operations` (nullable `server_id` with `ON DELETE SET NULL`, identity snapshot columns `server_name, server_host, server_port, server_user, host_key_sha256`, `resolution_json`), `operation_steps`, `operation_events` (transitions, unit ids, recovery decisions; max 200 per operation), `operations_server` index, and a `meta` row for a random installation id. No secret column. Operations are never cascade-deleted with the profile.
    - Add Store methods behind the `provisioning` feature: insert operation with snapshot, update status, upsert step, replace step output (redacted, valid UTF-8, capped at 64 KiB), append event, cap `error` with `ipc::cap`, get, list (max 50), startup recovery that marks Pending, Running and Verifying as UnknownOutcome, and `has_unresolved_operation(server_id)` (Pending, Running, Verifying or UnknownOutcome). Add `sha2` as an optional dependency of the feature.
    - Profile protection (compiled unconditionally, so Doctor and Deploy share it): `remove_server`, `update_server` changing host, port, username or auth config, and `replace_host_key` return `plan { serverInUse }` while `has_unresolved_operation` is true. A display-name change is allowed.
    - Unit tests: migration idempotent; schema test shows no secret column; output cap and UTF-8 boundary; removing a server with a terminal operation keeps the operation with a null reference and intact snapshot; removal, host/port/user/auth edit and host-key replace are rejected for each of the four non-terminal states while a display-name edit passes; startup marking changes only in-flight rows; list limit is 50; event cap.
    - _Requirements: 11.1, 11.2, 11.4, 11.5, 11.7, 11.9, 13.1, 13.3, 13.7, 18.2, 23.7, 23.9, 23.12, 27.1, 27.2, 27.3_

  - [x] 2.3 Plan store, digest and ApprovedPlan
    - New `provisioning/plans.rs`: in-memory `PlanStore` with at most one plan per server, 10 minute expiry, `digest = hex(sha256(canonical JSON of Plan))`, and `ApprovedPlan` whose only constructor is the store's approval path. Approval checks in order: unknown plan, plan id (`Superseded`), expiry, submitted digest equals stored and recomputed digest (`Mismatch`), server identity (host, port, user, pinned host key) equals the plan's snapshot (`WrongServer`), connected, no active or UnknownOutcome operation (`Busy`). Approval consumes the plan. The frontend submits only ids and the digest, never plan contents.
    - Provide a digest re-verification helper for plans read back from SQLite.
    - Input validation helpers: UUID ids, module-style component ids, 64 lowercase hex digest.
    - Unit tests (Property 7): expiry, supersede, digest mismatch, tampered stored plan, wrong server (endpoint and pinned key changed), single use, unknown plan, busy server, and validators reject malformed input. Use an injectable clock.
    - _Requirements: 4.1, 4.2, 4.4, 10.1, 10.2, 10.3, 10.4, 10.5, 10.9, 10.10, 23.4_

  - [x] 2.4 Operation engine, reconcile and manual resolution
    - New `provisioning/engine.rs`: after approval checks and `Provisioner::recheck`, acquires the remote lock (task 2.7), creates the Operation with the identity snapshot and spawns the Tokio task. Before each step launch, each poll batch, reconcile and resolution, it compares the connection identity (server id, endpoint, pinned host key) with the operation snapshot. A mismatch stops all further commands (UnknownOutcome if a step may be running).
    - Durable steps: persist the step with unit name `ossiframe-op-<full operation UUID>-<idx>` and Running status before launch. Launch with `/usr/bin/systemd-run --unit=<name> --service-type=exec --remain-after-exit --quiet` and `DEBIAN_FRONTEND=noninteractive`, `NEEDRESTART_SUSPEND=1`. Poll `systemctl show` every 3 s (LoadState, ActiveState, SubState, Result, ExecMainStatus), retry a timed-out poll up to 3 consecutive times, enforce the per-step deadline (5 min `apt-update`, 20 min `apt-install`), refresh the stored journal tail (last 200 lines, bounded, plain text) every 15 s and at completion.
    - When a step reaches a terminal state: persist exit status, end time and bounded log first. Only then clean up, and only a unit confirmed finished (`reset-failed` for a failed unit, `systemctl stop` for `active/exited` with no running process). Never stop, kill or signal a unit that may still run apt or dpkg. Cleanup failure is non-fatal.
    - Gate steps run through `Provisioner::gate`; a failing gate fails the Operation before the next step.
    - Failure handling: foreground non-zero exit or unit failure gives Failed. Disconnect, deadline, launch timeout, identity mismatch or exhausted poll retries give UnknownOutcome with no relaunch and no retry of any mutating command. A client-side deadline means stop waiting and report "may still be running"; nothing terminates apt, dpkg or the unit.
    - Cancel: record the request and signal the task. The task finishes the current step ("cancel requested, waiting for current step"), then marks remaining steps skipped and sets Cancelled before launching another step. The session cancel token counts as a cancel request and is never passed to polling commands.
    - After the steps: Verifying, `Provisioner::verify`, persist `verification_json`, then Succeeded only if every selected component is verified (all essential checks Pass), otherwise Failed. A zero-step plan goes straight to Verifying. Release the remote lock on Succeeded, Failed or Cancelled; keep it at UnknownOutcome.
    - Reconcile (`provision_reconcile`): read-only evidence collection (recorded step history, state of the recorded unit if present, dpkg health from the unprivileged `dpkg-query` listing, apt/dpkg processes and lock holders, `dpkg-query` on plan packages, service state), resolved per the design decision table. A missing unit never alone moves the operation to Failed. Inconclusive or unavailable evidence stays UnknownOutcome. `apt-update` completion never implies `apt-install` completion, and a successful step never implies plan success. Reconcile never launches, resumes, stops or kills anything.
    - Mark resolved (`provision_mark_resolved`): allowed only for UnknownOutcome and only when the backend's own fresh evidence shows no apt/dpkg process, free lock files and no active `ossiframe-op-*` unit. It stores the user decision and the evidence in `operation_events` and `resolution_json`, sets Failed with resolution `markedResolved`, releases the lock if owned by this operation, and never retries or repairs.
    - Define the `OperationEvents` trait (`operation_updated`), write to SQLite on every state change and output refresh, persist transitions and unit ids as events, and stop writing when the server was removed. Use existing polling and events; no streaming infrastructure.
    - Unit tests with a scripted fake executor (Properties 10 to 13, 15, 16): step failure gives Failed; disconnect during a step gives UnknownOutcome and the fake records no second launch; deadline gives UnknownOutcome without any stop or kill command; poll timeout is retried 3 times then UnknownOutcome; cancel before step 2 gives Cancelled with step 2 never launched; identity mismatch stops further commands; terminal step result is persisted before any cleanup command; cleanup is skipped for an unfinished unit; reconcile table for every observation, including missing unit with launched step staying UnknownOutcome and missing unit with a never-launched step; mark-resolved is refused while an apt process or lock holder exists and allowed when clear; Succeeded only when every essential check passed; stored output is bounded; privileged commands appear only inside approved-plan execution.
    - _Requirements: 4.1, 10.6, 10.7, 11.6, 11.8, 12.1, 12.2, 12.3, 12.4, 12.5, 12.6, 12.7, 12.8, 12.9, 12.10, 12.11, 12.12, 12.13, 12.14, 13.1, 13.2, 13.4, 13.5, 13.7, 13.8, 14.1, 14.2, 14.3, 14.4, 15.1, 15.2, 15.3, 15.4, 15.5, 15.6, 15.7, 15.8, 15.9, 17.1, 17.14, 17.15, 23.7, 23.8, 27.4, 27.5, 29.1, 29.2, 29.3, 29.4, 29.5_

  - [x] 2.5 `provisioning` feature, explicit setup and provision_* commands
    - Add the `provisioning` Cargo feature (requires `tauri`). New `provisioning/commands.rs` with the ten commands and `PROVISION_COMMAND_NAMES`: `provision_components`, `provision_preflight`, `provision_create_plan`, `provision_approve_plan`, `provision_cancel`, `provision_reconcile`, `provision_mark_resolved`, `provision_release_stale_lock`, `provision_get_operation`, `provision_list_operations`. They accept only server ids, component ids, plan ids, operation ids, a limit and a digest, and return typed DTOs (capped output, best-effort redaction). `provision_preflight` and `provision_create_plan` run the Provisioner preflight through the connection and refuse planning on blockers (`blocked`). Cancelled or dropped preflight returns an OperationError and creates no plan.
    - `provisioning::setup(app, Arc<dyn Provisioner>)` registers state, runs startup recovery and emits `operation-updated` through the Tauri glue. Leave `tauri_support::setup` and `AppSetup` unchanged.
    - Unit tests: command names are exactly the ten; no command parameter type contains a program, argv, path, privilege or plan-content field; `provision_list_operations` clamps to 50; a failed preflight stores no plan; the crate still compiles without the feature.
    - _Requirements: 4.3, 4.4, 4.5, 4.6, 4.7, 4.8, 6.6, 18.1, 18.2, 18.6, 11.6_

  - [x] 2.6 Typed client wrappers (`packages/client`)
    - Add `provisioning.ts` with typed wrappers and DTO types for the ten commands and the `operation-updated` event, exported separately from `api`. Add the `plan` kind to the error `KINDS` set and its reason type (including `serverInUse`).
    - Check with `pnpm typecheck` and a plain test in the style the package uses today. Add no new test framework.
    - _Requirements: 18.4, 18.5_

  - [x] 2.7 Remote exclusion lock
    - New `provisioning/lock.rs`. Acquire by starting the transient unit `ossiframe-provision-lock` with `sudo -n /usr/bin/systemd-run --unit=ossiframe-provision-lock --service-type=exec --quiet --description=<ownership record> -- /usr/bin/sleep infinity` (fixed argv; the description is built only from a validated operation UUID, the fixed app id, the Store's installation id and an RFC 3339 start time). A second start with the same unit name fails atomically; the engine then reads the owner with `systemctl show -p Description,ActiveState,SubState` and returns `plan { busy }` with owner details.
    - Parse and validate the ownership record from the unit description. Release only when the record names this operation (stop and `reset-failed` of the holder, which runs only `sleep`). Never release a lock owned by another operation.
    - Stale handling: a holder whose operation is not a non-terminal operation in the local Store is reported with its owner details. `provision_release_stale_lock(serverId)` is an explicit user action that the backend allows only after its own evidence shows no active `ossiframe-op-*` step unit, no apt/dpkg process and free lock files; it records the decision and evidence in `operation_events`. A holder owned by a local UnknownOutcome operation is released through mark-resolved, not through this command.
    - If `systemd-run` is unavailable or the acquisition result cannot be classified, the engine treats it as a blocker. Native apt/dpkg locks stay in force. The preflight unit scan stays advisory.
    - Unit tests: ownership record round trip and rejection of malformed or hostile records; acquisition result classification (acquired, already exists, unavailable); release refused for a foreign owner; release allowed for own owner; stale detection and the stale-release gating on each evidence input; lock kept at UnknownOutcome and released at each terminal state.
    - _Requirements: 26.1, 26.2, 26.3, 26.4, 26.5, 26.6, 26.7, 26.8, 26.9, 26.11, 23.11_

- [x] 3. Module `modules/ubuntu-provisioning` (`ossiframe-mod-ubuntu-provisioning`)
  - [x] 3.1 Crate, component data and dependency resolution
    - New crate depending only on `ossiframe-core`, `ossiframe-mod-linux-inventory`, `async-trait`, `serde`, `serde_json`, `tokio`. Manifest declares `linux-inventory` and registers zero `OperationInfo` entries. Define `UbuntuProvisioning` implementing `Module` and a first `Provisioner` skeleton (`components()` implemented, other methods completed in 3.2 to 3.4).
    - `components.rs`: `ComponentDef` data for `git`, `nginx`, `php`, `php-fpm`, `composer`, `mysql`, `redis` (optional, unselected) with packages, requires, units, disk and memory needs, conflicting packages. Ubuntu 24.04 archive only, no PPA, no script. Default selection `git nginx php php-fpm composer mysql`. Resolve selections in dependency order; unknown id gives `PlanError::UnknownComponent`; any subset is allowed.
    - Before fixing the PHP package names, confirm them against a noble archive listing with `apt-cache policy` or the Ubuntu package search, and adjust the data. Do not claim versions.
    - Unit tests (Property 5): `php-fpm` pulls `php` first; NGINX alone and PHP without MySQL resolve; unknown id errors; default selection excludes redis; module registers zero operations.
    - _Requirements: 3.4, 8.1, 8.3, 8.4, 8.5, 8.6, 8.7, 8.8, 8.9, 8.10, 8.11_

  - [x] 3.2 Preflight, classification and blocking rules
    - `preflight.rs`: collect facts through the `Executor` with absolute paths only (inventory server-info, software, services, sockets; `id -u`; the sudo probe `/usr/bin/sudo -n -- /usr/bin/true`; `dpkg-query` for planned packages; `apt-cache policy`; an unprivileged whole-system `dpkg-query -W` listing for dpkg health; `pgrep` and `systemctl is-active` for apt activity; reboot-required; `find` over nginx and apache sites-enabled; `systemctl list-units 'ossiframe-op-*'` and the lock unit as advisory facts; read-only reads of `/etc/apt/sources.list`, `/etc/apt/sources.list.d/*.list` and `*.sources` (deb822)). The sudo probe is the only privileged command.
    - Classify sudo from the exit code (Passwordless, PasswordRequired, NotPermitted, Unavailable) with English stderr only as a message hint.
    - Source check: parse enabled sources and allow only the explicit allowlist (`archive.ubuntu.com`, `*.archive.ubuntu.com`, `security.ubuntu.com`, `ports.ubuntu.com`; provider mirrors are added only after verification). Any enabled source outside it, or an unreadable or unparsable source file, is a Blocker with an explanation. Never modify sources.
    - Classify the server: `Fresh`, `ExistingUnmanaged` (service-bearing packages, non-default sites or listeners on managed ports without an Ossiframe record; Blocker, no adoption), `OssiframeManaged` (only when the Store holds a Succeeded operation with no failed essential check for the same identity snapshot; package presence, file names or unit names never establish it) and `Partial` (earlier Failed, Cancelled or closed operation for this identity; Warning, allowed when dpkg is healthy). `git` and `composer` may be preinstalled and do not alone make a server unmanaged.
    - Evidence completeness: a probe that times out, fails to parse or is truncated at a cap yields a Blocker for every rule that depends on it. A listener whose owner cannot be determined never counts as owned by the selected component.
    - Build the rest of the `Conflict` list per Requirement 7 (26.04 "pending verification", RAM thresholds, disk need plus 1 GB, dpkg state, apache2/mariadb/postgresql and other PHP minors, nginx site classification, listener conflicts for ports 80, 443, 3306, 33060 and 6379, warnings for apt activity, reboot pending and missing candidates). The remote lock and foreign units are reported as advisory facts; the lock itself is authoritative.
    - Unit tests (Properties 3, 4 and 17): sudo classification table including non-zero exit never Passwordless; `dpkg-query` line parsing; nginx site classification; source parsing for one-line and deb822 formats, allowlisted and non-allowlisted hosts, unreadable file; each class decision, including "package presence alone is never OssiframeManaged"; unknown listener owner blocks; truncated or timed-out evidence blocks; each blocker makes `can_provision` false; warnings do not; every command in the preflight list has an absolute program path and none except the probe is privileged.
    - _Requirements: 2.1, 2.2, 2.3, 4.8, 6.1, 6.2, 6.3, 6.4, 6.5, 6.7, 6.8, 7.1, 7.2, 7.3, 7.4, 7.5, 7.6, 7.7, 7.8, 7.9, 7.10, 7.11, 7.12, 7.13, 7.14, 23.6, 28.1, 28.2, 28.3, 28.4_

  - [x] 3.3 Plan builder, gate and recheck
    - `plan.rs`: pure `plan(report, selection)`. Refuse on any Blocker (`blocked`) or a component rule such as low-RAM MySQL. Omit installed packages, sort the rest, fill the identity-snapshot target, OS, components, already-present, packages, empty files, services, resources, privileges, limitations and preconditions. Limitations: no rollback; partial packages on failure; services start automatically and package scripts may restart services; APT may install additional dependencies and run maintainer scripts; no firewall change; default NGINX page reachable on port 80; Ossiframe never runs `dpkg --configure -a`, removes packages or terminates apt/dpkg. When package lists allow it, run `apt-get install -s` and list the estimated transitive packages; otherwise state that they are unknown until after the update.
    - Emit steps `apt-update` (durable, `/usr/bin/apt-get update`, 5 min), `candidate-check` (gate) and `apt-install` (durable, `/usr/bin/apt-get install -y --no-install-recommends -o DPkg::Lock::Timeout=300 <pkgs>`, 20 min) when any package is missing, otherwise zero steps.
    - `gate` for `candidate-check` (read-only): `apt-cache policy` shows a candidate for every package with an allowlisted origin, and `apt-get install -s` shows every package to be installed (including dependencies) coming from allowlisted origins. The exact set is stored as step output for history. Failure fails the operation before `apt-install`, with actionable text (for example fix the source, then re-plan).
    - `recheck`: re-run the material probes and return `Drift` for OS version, planned package states, blockers (including foreign operation units and the lock owner), sudo mode, nginx site list, package sources, and the minimums (total RAM and free disk below the plan's needs). Ordinary memory fluctuation above the minimum, uptime and apt activity are not drift.
    - Complete the `Provisioner` impl for `plan`, `preflight`, `recheck` and `gate`.
    - Unit tests (Properties 4, 6, 8, 17): same report and selection gives the same digest input JSON; changing one package changes it; installed packages never appear in the install argv; zero-step plan when all present; blocker refusal; low-RAM MySQL refusal; gate passes for allowlisted origins and fails for an unknown origin or a missing candidate; drift on each material change including RAM or disk dropping below the minimum, and no drift on uptime, apt activity or memory changes above the minimum.
    - _Requirements: 9.1, 9.2, 9.3, 9.4, 9.5, 9.6, 9.7, 9.8, 9.9, 9.10, 9.11, 10.7, 10.8, 16.1, 16.3, 16.4, 23.5, 28.5, 28.6, 28.7, 28.9_

  - [x] 3.4 Post-install verification
    - `verify.rs`: `Provisioner::verify` returns `CheckResult`s (`essential` flag, detail capped at 200 characters) for every selected component and a `ComponentVerdict` per component: verified only if all its essential checks Pass; a Fail, Skipped or unavailable essential check means not verified; optional checks yield Warn or Skipped with the limitation recorded. No scores.
    - Essential checks: every component package `ii`; git `--version` runs; composer `--version` starts with `Composer version 2.`; nginx unit active, `sudo -n nginx -t`, and a bounded local request (`/usr/bin/curl -sS --max-time 5 -o /dev/null -w '%{http_code}' http://127.0.0.1/`) that is essential when `curl` exists (any returned HTTP status passes and is recorded, connection failure or timeout fails) and Skipped with a recorded limitation when `curl` is absent; php `php -v` is 8.3 and `php -m` has the required modules; php-fpm unit active, socket `/run/php/php8.3-fpm.sock` exists, `php-fpm8.3 -t`, required modules via `php-fpm8.3 -m`; mysql unit active, local connection via `sudo -n /usr/bin/mysql --no-defaults -N -B -e 'SELECT 1'` over `auth_socket` (chosen over `mysqladmin ping`, which can exit 0 on access denied), ports 3306 and 33060 loopback-only (an unreadable socket listing is a Fail); redis unit active, 6379 loopback-only, `redis-cli ping` returns `PONG`. Versions of nginx, mysqld and redis are optional.
    - Only `nginx -t`, `php-fpm8.3 -t` and `mysql` run privileged, and only inside the approved operation.
    - Unit tests (Property 13): parsers for each check with pass and fail inputs built in the test (not captured server output); loopback-only versus wildcard binding; HTTP status vs connection failure vs curl missing; essential Fail, Skipped and unavailable all make the component not verified; optional Warn does not; detail cap; selection limits which checks run.
    - _Requirements: 17.2, 17.3, 17.4, 17.5, 17.6, 17.7, 17.8, 17.9, 17.10, 17.11, 17.12, 17.13, 13.4, 23.13_

- [x] 4. Doctor drift tests
  - Extend Doctor's Rust drift tests in `apps/doctor/src-tauri`: capability, `build.rs` and handler lists equal `COMMAND_NAMES` plus the report commands exactly; no permission name contains `provision`; `Cargo.toml` names neither `ossiframe-mod-ubuntu-provisioning` nor the `provisioning` feature; no dependency on Deploy. Confirm Doctor's handler list is unchanged and that it calls no provisioning setup. Doctor's read-only behaviour is asserted through these lists and its registered modules, not through the `ReadOnly` guard alone. Fix any drift the new desktop feature introduces, without enabling the feature in Doctor.
  - _Requirements: 4.7, 5.1, 5.2, 5.3, 5.4, 5.5, 3.7, 25.3_

- [x] 5. Deploy app (`apps/deploy`)
  - [x] 5.1 Tauri shell and capability drift tests
    - Add `apps/deploy/src-tauri` as a workspace member: identifier `dev.ossiframe.deploy`, own data directory, keychain service and `deploy.sqlite3`. Registry: `LinuxInventory` and `UbuntuProvisioning`. `setup(...)` then `provisioning::setup(app, Arc::new(UbuntuProvisioning))`. Handler list, `build.rs` `AppManifest` and `capabilities/main.json` list the shared commands plus `PROVISION_COMMAND_NAMES` plus the inventory commands. No `dialog:` or `fs:` permission. No dependency on Doctor or `linux-audit`.
    - Unit tests (Property 14): capability and build manifest equal the expected set; no `dialog:` or `fs:` permissions; `Cargo.toml` has no Doctor or `linux-audit` dependency.
    - _Requirements: 19.1, 19.2, 19.3, 19.4, 4.6_

  - [x] 5.2 Provisioning pages in `modules/ubuntu-provisioning/ui/`
    - React pages exported from the module `ui/` directory using `@ossiframe/ui` and the `provisioning.ts` client: Preflight (server class, conflicts and warnings, sudo requirement, statement that passwordless sudo is root-equivalent), Software (components with installed state and checkboxes, default selection, redis unchecked), Plan (packages, services, steps, resources, privileges, limitations, expiry countdown, the digest in a technical-details section), an approval confirmation dialog, Operation (step status, bounded log rendered as text, "cancel requested" state, verification table per component with essential and optional checks, lock owner when busy, Reconcile and "Mark resolved after inspection" actions for UnknownOutcome with the statement that nothing is retried automatically) and History (digest and recorded decisions available). No mock data. Nothing mutates before explicit approval.
    - The confirmation dialog lists the target server (identity snapshot), selected components, packages to install, expected service changes, the root privilege requirement, resource requirements, recovery limitations, and that APT may install additional dependencies and run maintainer scripts, with an explicit approve action for that plan. It states that the digest does not protect against a compromised frontend. The frontend submits only ids and the digest. No typed-digest confirmation. The UI makes no claim of concurrent safety beyond the Ossiframe lock.
    - _Requirements: 13.6, 19.5, 19.6, 19.7, 19.8, 19.9, 19.10, 19.11, 19.12, 19.13, 19.14, 26.10_

  - [x] 5.3 Deploy frontend shell
    - Add the Deploy frontend (Vite, React, Tailwind, same layout as Doctor): `ServerWorkspace` with tabs Overview (inventory UI), Preflight, Software, Plan, Operation, History using the pages from 5.2, and `pnpm` scripts for typecheck, lint and build. Add the workspace entry. Nothing ties a server to a runtime, and nothing Laravel-specific is added. Further components can be installed later through a new plan.
    - _Requirements: 19.5, 19.11, 19.12, 19.15, 23.1_

- [x] 6. Generator opt-in and documentation
  - [x] 6.1 Generator catalog and `--enable-provisioning`
    - In `tools/ossiframe-cli`: add `privileged: bool` to `CatalogEntry` and set it for `ubuntu-provisioning`. `new --modules ubuntu-provisioning` without `--enable-provisioning` fails with a message that names the flag. With the flag, render the `provisioning` feature, the `provisioning::setup` call, the ten commands in handler, `build.rs` and capability, and the matching drift tests. Without it, the starter has no provisioning command, feature or module (default stays read-only). Follow how `render.rs` already injects module registration. Preserve license notices in generated output.
    - Unit tests (Property 14): missing flag fails with the flag name; with the flag, the rendered files contain all ten commands in the three lists and the feature; without it, none of the `provision` names appear in any rendered file; license notice files are present in both.
    - _Requirements: 20.1, 20.2, 20.3, 20.4, 20.6_

  - [x] 6.2 Documentation updates
    - Update `docs/architecture.md` (execution, operations, remote lock), `docs/security.md` (privilege table and the enforcement layers, with the accurate limits of the `ReadOnly` guard), `docs/developer-guide.md` (opt-in steps), ADR-0004 and ADR-0005 (resolved notes on the open questions) and `docs/roadmap.md` (S04 status as in progress plus S05 deferrals: configuration-write primitive and the stdin field). State only design facts here. Observed server facts are added in task 9.
    - _Requirements: 16.5, 21.1, 21.2, 21.3, 21.4_

- [x] 7. Consolidated quality gate
  - Run and fix until clean: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo check` (workspace, and Doctor and Deploy individually to confirm feature isolation), `cargo test`, `pnpm typecheck`, `pnpm lint`, production builds of Doctor and Deploy, and `shellcheck` for any script touched. Confirm no property-testing framework or e2e suite was added and no secret appears in new fixtures, logs or docs.
  - _Requirements: 23.1, 23.10, 25.1, 25.2, 25.8_

- [ ] 8. Live verification (depends on the user's disposable server; does not block tasks 1 to 7)
  - Rules for this group: the existing small Lightsail Doctor server is used for read-only preflight only and is never provisioned. Mutation happens only on a fresh disposable Ubuntu 24.04 server (about 2 vCPU, 4 GB) that the user creates and names. Create no cloud resource. Run no command against servers from hooks. Record real output only, sanitized (no keys, passwords, tokens or private addresses beyond what the user allows). The user runs the desktop apps (`pnpm tauri dev` in `apps/deploy`) and shares results or screenshots when the agent cannot drive the GUI. State any check that could not be run.
  - [x] 8.1 Read-only preflight on the Doctor server
    - Using the existing 512 MB Lightsail server (host key already pinned), run only `provision_preflight` through Deploy. Do not create a plan or approve anything and install nothing, now or later. Record the report as observed, including server class, package sources and blockers. If the result differs from expectations, record it as is.
    - _Requirements: 22.1, 22.2, 22.4_

  - [x] 8.2 STOP: ask the user for a disposable server
    - Stop and ask the user (`user_input`) to create a fresh Ubuntu 24.04 instance (about 2 vCPU, 4 GB RAM), confirm its host key fingerprint in the provider console, and provide host, user and key. Do not create any cloud resource. Do not continue to 8.3 until the user answers and gives explicit approval to provision that server.
    - _Requirements: 22.2, 22.3, 22.5_

  - [-] 8.3 Provisioning checks on the disposable server
    - Run and record: preflight, plan, approval and provisioning of the selected components with every verification check, and package versions observed; the behavior of `systemd-run --remain-after-exit` on noble; the actual `needrestart` and noninteractive behavior (record whether any service restart was observed); the candidate-check gate and package origins; a second plan on the same server with zero install steps that only verifies; state change between plan and approval giving `rePlanRequired`; expiry rejection; altered digest rejection; the remote lock (a second desktop or a second plan attempt sees `busy` with owner details, the lock unit survives SSH loss and app exit, and stale handling needs explicit user action); profile removal and host/port/user/key edits refused during an operation; interruption of the SSH connection or app during `apt-install` giving UnknownOutcome with the unit still running, Reconcile and "Mark resolved" behavior and no automatic retry; a fresh boot with unattended-upgrades holding the dpkg lock showing the `DPkg::Lock::Timeout` wait (record "not observed" if the timing cannot be reproduced); Doctor against the provisioned server exposing no provisioning command.
    - _Requirements: 22.1, 22.5, 22.6, 22.7, 22.8, 22.9, 22.10, 22.11, 22.12, 25.4, 25.5, 25.6, 25.10_

  - [ ] 8.4 Generated app check
    - Generate an app outside the repository with `--enable-provisioning` in local mode (path dependencies on this repository) and confirm it builds. Generate one without the flag and confirm it builds and exposes no provisioning command in handler, `build.rs` or capability. Record both results. Verification against a published revision is deferred until the user publishes.
    - _Requirements: 20.5, 25.7_

- [ ] 9. Verification record, roadmap and compatibility
  - Write the sanitized `docs/verification/s04-infrastructure-provisioning.md` with real outputs from task 8 only, a mapping of each S04 completion criterion (Requirement 25) to its evidence or to "not run", and any limitation. Update `docs/roadmap.md` with the S04 final status and `docs/compatibility.md` with Ubuntu 24.04 component versions as actually observed (package versions recorded in 8.3). Add no claim that was not observed.
  - _Requirements: 21.4, 21.5, 22.1, 25.9_

## Notes

- No tasks are optional. Tests for the Correctness Properties live inside the implementation tasks as plain `#[test]` functions.
- Checks that need a real server are in task 8 and depend on the user. Task 8.2 is a hard stop.
- Mutating work is reachable only through the Plan, approval and engine path. Doctor keeps no provisioning command.
- Do not commit, push or publish without explicit user approval.

## Task Dependency Graph

```json
{
  "waves": [
    { "id": 0, "tasks": ["1.1"] },
    { "id": 1, "tasks": ["1.2"] },
    { "id": 2, "tasks": ["1.3"] },
    { "id": 3, "tasks": ["2.1", "3.1"] },
    { "id": 4, "tasks": ["2.2", "3.2"] },
    { "id": 5, "tasks": ["2.3", "2.7", "3.3"] },
    { "id": 6, "tasks": ["2.4", "3.4"] },
    { "id": 7, "tasks": ["2.5"] },
    { "id": 8, "tasks": ["2.6", "4"] },
    { "id": 9, "tasks": ["5.1", "5.2"] },
    { "id": 10, "tasks": ["5.3", "6.1"] },
    { "id": 11, "tasks": ["6.2"] },
    { "id": 12, "tasks": ["7"] },
    { "id": 13, "tasks": ["8.1"] },
    { "id": 14, "tasks": ["8.2"] },
    { "id": 15, "tasks": ["8.3"] },
    { "id": 16, "tasks": ["8.4"] },
    { "id": 17, "tasks": ["9"] }
  ]
}
```
