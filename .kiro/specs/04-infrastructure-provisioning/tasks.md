# Implementation Plan: Infrastructure Provisioning (S04)

## Overview

Implementation is Rust (Tokio, Tauri 2) plus TypeScript (React). The order is Core, then desktop engine, then the `ubuntu-provisioning` module, then the Doctor drift tests, then `apps/deploy`, then the generator opt-in and docs, then one quality gate, then live verification. Each task ends wired into the previous ones. The Correctness Properties from the design are checked by small plain `#[test]` unit tests inside the implementation tasks. No property-testing framework, no end-to-end suite, no commit or push tasks.

`ApprovedPlan` is defined in `ossiframe-desktop` (task 2.3) with a private constructor reachable only from the plan store. Core cannot enforce a crate-private constructor for another crate, so Core defines the `Plan` and `Provisioner` contracts and desktop owns the approval type.

## Tasks

- [ ] 1. Core contracts (`crates/ossiframe-core`)
  - [ ] 1.1 Add Privilege to CommandSpec
    - In `command.rs`: add `Privilege { User, Root }`, `CommandSpec::privileged(program, args, timeout)` (absolute program path required, otherwise `ValidationError`) and `privilege()`. `new` yields `User`.
    - `to_command_line()` prefixes `'sudo' '-n' '--'` only for `Root`. Every word stays individually quoted. Add no stdin field.
    - In `executor.rs`: add `ExecError::PrivilegeDenied`.
    - Unit tests (Property 1): privileged line starts with `'sudo' '-n' '--'`; hostile arguments stay one quoted word (extend the existing real-shell round trip); relative program rejected; `User` has no sudo prefix.
    - _Requirements: 1.1, 1.2, 1.3, 1.4, 1.5, 1.6, 2.6, 16.2, 23.2_

  - [ ] 1.2 Add the ReadOnly executor guard
    - In `executor.rs`: add `ReadOnly<'a>(&'a dyn Executor)`. `run` returns `PrivilegeDenied` for any spec whose privilege is not `User` without calling the wrapped executor, and passes `User` specs through. Export from `lib.rs`.
    - Unit tests (Property 2) with a recording fake executor: `Root` gives `PrivilegeDenied` and zero recorded runs; `User` is passed through.
    - _Requirements: 2.4, 2.6, 3.1, 3.2, 3.3, 23.3_

  - [ ] 1.3 Add provisioning contracts and the state machine
    - New `provision.rs`, exported from `lib.rs`: `OperationState` (seven states), pure `next(OperationState, Event) -> Result<_, IllegalTransition>` following the design diagram, `Plan` and its parts (`PlanTarget`, `PackageChange`, `FileChange`, `ServiceEffect`, `Resources`, `PlanStep`, `StepMode`, `Preconditions`), `PreflightReport`, `Conflict`, `Severity`, `SudoMode`, `CheckResult`, `CheckStatus`, `ComponentInfo`, `Drift`, `PlanError` (including `UnknownComponent`), and the `Provisioner` async trait. Use ordered collections only so serialization is deterministic. No NGINX, PHP, MySQL or Laravel knowledge.
    - `PreflightReport::can_provision` is derived from the conflicts: false when any is a Blocker.
    - Leave `Module` and `OperationInfo` unchanged.
    - Unit tests (Property 9): every allowed transition passes, every other (state, event) pair returns `IllegalTransition`, terminal states accept no event; `can_provision` is false with a Blocker; a plan serializes to identical JSON twice.
    - _Requirements: 3.5, 6.3, 6.4, 8.2, 9.10, 11.3_

- [ ] 2. Desktop engine (`crates/ossiframe-desktop`)
  - [ ] 2.1 Wire the guard into dispatch and extend IpcError
    - In `dispatch::run_operation`, build the `OperationContext` executor as `ReadOnly(&*conn)`.
    - Map `ExecError::PrivilegeDenied` to `operationFailed` with a fixed message. Add the `IpcError::plan { reason, message }` variant with reasons `unknownPlan, superseded, expired, mismatch, wrongServer, busy, blocked, rePlanRequired, notSupported`.
    - Unit tests: a probe module that attempts a privileged command through `run_operation` is denied and the fake connection records zero runs; the privilege-denied mapping carries the fixed message; the `plan` variant serializes with a camelCase reason.
    - _Requirements: 2.7, 3.1, 3.2, 18.3, 23.3_

  - [ ] 2.2 Operations migration, store methods and recovery
    - Add migration 2 (unconditional, shared by Doctor and Deploy): `operations`, `operation_steps` and the `operations_server` index exactly as in the design, cascade-delete on server removal, no secret column.
    - Add Store methods behind the `provisioning` feature: insert operation, update status, upsert step, replace step output (redacted, valid UTF-8, capped at 64 KiB), cap `error` with `ipc::cap`, get, list (max 50), startup recovery that marks Pending, Running and Verifying as UnknownOutcome, and "active operation exists" for a server. Add `sha2` as an optional dependency of the feature.
    - Unit tests: migration is idempotent; the schema test shows no secret column; output cap and UTF-8 boundary; deleting a server cascades; startup marking changes only in-flight rows (Property 9); list limit is 50.
    - _Requirements: 11.1, 11.2, 11.4, 11.5, 11.7, 13.1, 13.3, 18.2, 23.7, 23.9_

  - [ ] 2.3 Plan store, digest and ApprovedPlan
    - New `provisioning/plans.rs`: in-memory `PlanStore` with at most one plan per server, 10 minute expiry, `digest = hex(sha256(canonical JSON of Plan))`, and `ApprovedPlan` whose only constructor is the store's approval path. Approval checks in order: unknown plan, plan id (`Superseded`), expiry, submitted digest equals stored and recomputed digest (`Mismatch`), server record and pinned host key equal the target (`WrongServer`), connected, no active or UnknownOutcome operation (`Busy`). Approval consumes the plan.
    - Provide a digest re-verification helper for plans read back from SQLite.
    - Input validation helpers: UUID ids, module-style component ids, 64 lowercase hex digest.
    - Unit tests (Property 7): expiry, supersede, digest mismatch, tampered stored plan, wrong server (endpoint and pinned key changed), single use, unknown plan, busy server, and validators reject malformed input. Use an injectable clock.
    - _Requirements: 4.1, 4.2, 4.4, 10.1, 10.2, 10.3, 10.4, 10.5, 10.9, 10.10, 23.4_

  - [ ] 2.4 Operation engine
    - New `provisioning/engine.rs`: calls `Provisioner::recheck` after approval checks and returns `rePlanRequired` with reasons on drift; creates the Operation and spawns the Tokio task. The task persists each step with its unit name and Running status before launch, then launches a durable step with `/usr/bin/systemd-run --unit=ossiframe-op-<8 hex>-<idx> --service-type=exec --remain-after-exit --quiet` and `DEBIAN_FRONTEND=noninteractive`, `NEEDRESTART_SUSPEND=1`. It polls `systemctl show` every 3 s for LoadState, ActiveState, SubState, Result and ExecMainStatus, retries a timed-out poll up to 3 consecutive times, and enforces the per-step deadline (5 min `apt-update`, 20 min `apt-install`). It refreshes the stored journal tail (last 200 lines, redacted, capped) every 15 s and at completion, then attempts `systemctl stop` and `reset-failed` as non-fatal cleanup.
    - Failure handling: foreground non-zero exit or unit failure gives Failed. Disconnect, deadline, launch timeout or exhausted poll retries give UnknownOutcome with no relaunch and no retry of any mutating command. Never stop, kill or signal apt, dpkg or the unit.
    - Cancel: `cancel` records the request and signals the task. The task finishes the current step ("cancel requested, waiting for current step"), then marks remaining steps skipped and sets Cancelled before launching another step. The session cancel token counts as a cancel request and is never passed to polling commands.
    - After the steps: Verifying, `Provisioner::verify`, persist `verification_json`, then Succeeded if no result is Fail, otherwise Failed. A zero-step plan goes straight to Verifying.
    - Reconcile: read-only `systemctl show` on the recorded unit and `dpkg-query` on the plan packages, resolved per the decision table (still active stays UnknownOutcome; finished success goes to Verifying; finished failure gives Failed with the capped journal tail; unit not found gives Failed "no remote activity found"). Never launches a step or resumes a plan.
    - Define the `OperationEvents` trait (`operation_updated`) and write to SQLite on every state change and output refresh. Stop writing when the server was removed.
    - Unit tests with a scripted fake executor (Properties 10 to 13): step failure gives Failed; disconnect during a step gives UnknownOutcome and the fake records no second launch; deadline gives UnknownOutcome; poll timeout is retried 3 times then UnknownOutcome; cancel before step 2 gives Cancelled with step 2 never launched; Succeeded only without a Fail check; reconcile table for all four observations with zero mutating commands; stored output is bounded and redacted; privileged commands appear only inside approved-plan execution.
    - _Requirements: 4.1, 10.6, 10.7, 11.6, 11.8, 12.1, 12.2, 12.3, 12.4, 12.5, 12.6, 12.7, 12.8, 12.9, 12.10, 12.11, 12.12, 13.1, 13.2, 13.4, 13.5, 14.1, 14.2, 14.3, 14.4, 15.1, 15.2, 15.3, 15.4, 15.5, 15.6, 15.7, 17.1, 17.11, 17.12, 17.13, 23.7, 23.8_

  - [ ] 2.5 `provisioning` feature, explicit setup and provision_* commands
    - Add the `provisioning` Cargo feature (requires `tauri`). New `provisioning/commands.rs` with the eight commands and `PROVISION_COMMAND_NAMES`: `provision_components`, `provision_preflight`, `provision_create_plan`, `provision_approve_plan`, `provision_cancel`, `provision_reconcile`, `provision_get_operation`, `provision_list_operations`. They accept only server ids, component ids, plan ids, operation ids, a limit and a digest, and return typed DTOs (redacted and capped output). `provision_preflight` and `provision_create_plan` run the Provisioner preflight through the connection and refuse planning on blockers (`blocked`). Cancelled or dropped preflight returns an OperationError and creates no plan.
    - `provisioning::setup(app, Arc<dyn Provisioner>)` registers state, runs startup recovery and emits `operation-updated` through the Tauri glue. Leave `tauri_support::setup` and `AppSetup` unchanged.
    - Unit tests: command names are exactly the eight; no command parameter type contains a program, argv, path or privilege field (checked through the DTO/argument structs); `provision_list_operations` clamps to 50; a failed preflight stores no plan; the crate still compiles without the feature.
    - _Requirements: 4.3, 4.4, 4.5, 4.6, 4.7, 4.8, 6.6, 18.1, 18.2, 18.6, 11.6_

  - [ ] 2.6 Typed client wrappers (`packages/client`)
    - Add `provisioning.ts` with typed wrappers and DTO types for the eight commands and the `operation-updated` event, exported separately from `api`. Add the `plan` kind to the error `KINDS` set and its reason type.
    - Check with `pnpm typecheck` and a small vitest-free type-level check already used by the package, or a plain test in the style the package uses today. Add no new test framework.
    - _Requirements: 18.4, 18.5_

- [ ] 3. Module `modules/ubuntu-provisioning` (`ossiframe-mod-ubuntu-provisioning`)
  - [ ] 3.1 Crate, component data and dependency resolution
    - New crate depending only on `ossiframe-core`, `ossiframe-mod-linux-inventory`, `async-trait`, `serde`, `serde_json`, `tokio`. Manifest declares `linux-inventory` and registers zero `OperationInfo` entries. Define `UbuntuProvisioning` implementing `Module` and a first `Provisioner` skeleton (`components()` implemented, other methods completed in 3.2 to 3.4).
    - `components.rs`: `ComponentDef` data for `git`, `nginx`, `php`, `php-fpm`, `composer`, `mysql`, `redis` (optional, unselected) with packages, requires, units, disk and memory needs, conflicting packages. Ubuntu 24.04 archive only, no PPA, no script. Default selection `git nginx php php-fpm composer mysql`. Resolve selections in dependency order; unknown id gives `PlanError::UnknownComponent`; any subset is allowed.
    - Before fixing the PHP package names, confirm them against a noble archive listing with `apt-cache policy` or the Ubuntu package search, and adjust the data. Do not claim versions.
    - Unit tests (Property 5): `php-fpm` pulls `php` first; NGINX alone and PHP without MySQL resolve; unknown id errors; default selection excludes redis; module registers zero operations.
    - _Requirements: 3.4, 8.1, 8.3, 8.4, 8.5, 8.6, 8.7, 8.8, 8.9, 8.10, 8.11_

  - [ ] 3.2 Preflight and blocking rules
    - `preflight.rs`: collect facts through the `Executor` with absolute paths only (inventory server-info, software, services, sockets; `id -u`; the sudo probe `sudo -n -- /usr/bin/true`; `dpkg-query` for planned packages; `apt-cache policy`; `dpkg --audit`; `pgrep` and `systemctl is-active` for apt activity; reboot-required; `find` over nginx and apache sites-enabled; `systemctl list-units 'ossiframe-op-*'`). The sudo probe is the only privileged command.
    - Classify sudo from the exit code (Passwordless, PasswordRequired, NotPermitted, Unavailable) with English stderr only as a message hint. Build the `Conflict` list with the blocker and warning rules of Requirement 7, including 26.04 "pending verification", RAM thresholds (900 MB block, 1.8 GB warn for MySQL), disk need plus 1 GB, dpkg state, apache2/mariadb/postgresql and other PHP minors, nginx site classification (`default` and `ossiframe-*` allowed), listener conflicts for ports 80, 443, 3306, 33060 and 6379, foreign operation units, and warnings for apt activity, reboot pending and missing candidates. Packages from an earlier Ossiframe run are reported as already present, not as conflicts. No adoption path.
    - Unit tests (Properties 3 and 4): sudo classification table including non-zero exit never Passwordless; `dpkg-query` line parsing; nginx site classification; listener conflicts with and without owning component; each blocker makes `can_provision` false; warnings do not; already-present is not a conflict; every command in the preflight list has an absolute program path and none except the probe is privileged.
    - _Requirements: 2.1, 2.2, 2.3, 4.8, 6.1, 6.2, 6.3, 6.4, 6.5, 7.1, 7.2, 7.3, 7.4, 7.5, 7.6, 7.7, 7.8, 7.9, 7.10, 7.11, 7.12, 7.13, 7.14, 23.6_

  - [ ] 3.3 Plan builder and recheck
    - `plan.rs`: pure `plan(report, selection)`. Refuse on any Blocker (`blocked`) or a component rule such as low-RAM MySQL. Omit installed packages, sort the rest, fill target, OS, components, already-present, packages, empty files, services, resources, privileges, limitations (no rollback, partial packages on failure, services start automatically, no firewall change, default NGINX page reachable on port 80) and preconditions. Emit durable steps `apt-update` (`/usr/bin/apt-get update`, 5 min) and `apt-install` (`/usr/bin/apt-get install -y --no-install-recommends -o DPkg::Lock::Timeout=300 <pkgs>`, 20 min) when any package is missing, otherwise zero steps.
    - `recheck`: re-run the material probes and return `Drift` only for OS version, planned package states, blockers (including foreign operation units), free disk, sudo mode or the nginx site list. Memory, uptime and apt activity are not drift.
    - Complete the `Provisioner` impl for `plan`, `preflight` and `recheck`.
    - Unit tests (Properties 4, 6, 8): same report and selection gives the same digest input JSON; changing one package changes it; installed packages never appear in the install argv; zero-step plan when all present; blocker refusal; low-RAM MySQL refusal; drift on each material change and no drift on memory, uptime or apt activity changes.
    - _Requirements: 9.1, 9.2, 9.3, 9.4, 9.5, 9.6, 9.7, 9.8, 9.9, 9.10, 9.11, 10.7, 10.8, 16.1, 16.3, 16.4, 23.5_

  - [ ] 3.4 Post-install verification
    - `verify.rs`: `Provisioner::verify` returns `CheckResult`s (detail capped at 200 characters) for every selected component: package `ii` states, git version, nginx (version, unit active, `nginx -t`, HTTP request when `curl` exists with 200 Pass, other code Warn, connection failure Fail), php (8.3 and module list), php-fpm (version, unit, `php-fpm8.3 -t`, socket `/run/php/php8.3-fpm.sock`, modules), composer (`Composer version 2.`), mysql (unit, version, loopback-only 3306 and 33060, `SELECT 1` via `auth_socket`), redis (unit, version, loopback-only 6379, `PONG`). Only the commands needed for `nginx -t`, `php-fpm8.3 -t` and `mysql` run privileged, and only inside the approved operation.
    - Unit tests (Property 13): parsers for each check with pass and fail fixtures built in the test (not captured server output); loopback-only versus wildcard binding; HTTP 200 vs other code vs connection failure mapping; detail cap; selection of components limits which checks run.
    - _Requirements: 17.2, 17.3, 17.4, 17.5, 17.6, 17.7, 17.8, 17.9, 17.10, 13.4_

- [ ] 4. Doctor drift tests
  - Extend Doctor's Rust drift tests in `apps/doctor/src-tauri`: capability, `build.rs` and handler lists equal `COMMAND_NAMES` plus the report commands exactly; no permission name contains `provision`; `Cargo.toml` names neither `ossiframe-mod-ubuntu-provisioning` nor the `provisioning` feature; no dependency on Deploy. Confirm Doctor's handler list is unchanged. Fix any drift the new desktop feature introduces, without enabling the feature in Doctor.
  - _Requirements: 4.7, 5.1, 5.2, 5.3, 5.4, 5.5, 25.3_

- [ ] 5. Deploy app (`apps/deploy`)
  - [ ] 5.1 Tauri shell and capability drift tests
    - Add `apps/deploy/src-tauri` as a workspace member: identifier `dev.ossiframe.deploy`, own data directory, keychain service and `deploy.sqlite3`. Registry: `LinuxInventory` and `UbuntuProvisioning`. `setup(...)` then `provisioning::setup(app, Arc::new(UbuntuProvisioning))`. Handler list, `build.rs` `AppManifest` and `capabilities/main.json` list the shared commands plus `PROVISION_COMMAND_NAMES` plus the inventory commands. No `dialog:` or `fs:` permission. No dependency on Doctor or `linux-audit`.
    - Unit tests (Property 14): capability and build manifest equal the expected set; no `dialog:` or `fs:` permissions; `Cargo.toml` has no Doctor or `linux-audit` dependency.
    - _Requirements: 19.1, 19.2, 19.3, 19.4, 4.6_

  - [ ] 5.2 Provisioning pages in `modules/ubuntu-provisioning/ui/`
    - React pages exported from the module `ui/` directory using `@ossiframe/ui` and the `provisioning.ts` client: Preflight (conflicts and warnings, sudo requirement, statement that passwordless sudo is root-equivalent), Software (components with installed state and checkboxes, default selection, redis unchecked), Plan (packages, services, steps, resources, privileges, limitations, full digest, expiry countdown, approve enabled only for an unexpired plan after typing the first 8 hex characters of the digest), Operation (step status, bounded log rendered as text, "cancel requested" state, verification table, Reconcile button for UnknownOutcome with the statement that nothing is retried automatically) and History. Subscribe to `operation-updated` and refetch. No mock data. Nothing mutates before explicit approval.
    - _Requirements: 13.6, 19.5, 19.6, 19.7, 19.8, 19.9, 19.10, 19.11, 19.12, 19.13_

  - [ ] 5.3 Deploy frontend shell
    - Add the Deploy frontend (Vite, React, Tailwind, same layout as Doctor): `ServerWorkspace` with tabs Overview (inventory UI), Preflight, Software, Plan, Operation, History using the pages from 5.2, and `pnpm` scripts for typecheck, lint and build. Add the workspace entry.
    - _Requirements: 19.5, 19.11, 19.12, 23.1_

- [ ] 6. Generator opt-in and documentation
  - [ ] 6.1 Generator catalog and `--enable-provisioning`
    - In `tools/ossiframe-cli`: add `privileged: bool` to `CatalogEntry` and set it for `ubuntu-provisioning`. `new --modules ubuntu-provisioning` without `--enable-provisioning` fails with a message that names the flag. With the flag, render the `provisioning` feature, the `provisioning::setup` call, the eight commands in handler, `build.rs` and capability, and the matching drift tests. Without it, the starter has no provisioning command, feature or module. Follow how `render.rs` already injects module registration.
    - Unit tests (Property 14): missing flag fails with the flag name; with the flag, the rendered files contain all eight commands in the three lists and the feature; without it, none of the `provision` names appear in any rendered file.
    - _Requirements: 20.1, 20.2, 20.3, 20.4_

  - [ ] 6.2 Documentation updates
    - Update `docs/architecture.md` (execution and operations), `docs/security.md` (privilege table and the four enforcement layers), `docs/developer-guide.md` (opt-in steps), ADR-0004 and ADR-0005 (resolved notes on the open questions) and `docs/roadmap.md` (S04 status as in progress plus S05 deferrals: configuration-write primitive and the stdin field). State only design facts here. Observed server facts are added in task 9.
    - _Requirements: 16.5, 21.1, 21.2, 21.3, 21.4_

- [ ] 7. Consolidated quality gate
  - Run and fix until clean: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo check` (workspace, and Doctor and Deploy individually to confirm feature isolation), `cargo test`, `pnpm typecheck`, `pnpm lint`, production builds of Doctor and Deploy, and `shellcheck` for any script touched. Confirm no property-testing framework or e2e suite was added and no secret appears in new fixtures, logs or docs.
  - _Requirements: 23.1, 23.10, 25.1, 25.2, 25.8_

- [ ] 8. Live verification
  - Rules for this group: use only the existing Doctor server (read-only) and a disposable server the user names. Create no cloud resource. Run no command against servers from hooks. Record real output only, sanitized (no keys, passwords, tokens or private addresses beyond what the user allows). The user runs the desktop apps (`pnpm tauri dev` in `apps/deploy`) and shares results or screenshots when the agent cannot drive the GUI. State any check that could not be run.
  - [ ] 8.1 Read-only preflight on the Doctor server
    - Using the existing 512 MB Lightsail server (host key already pinned), run only `provision_preflight` through Deploy. Do not create a plan or approve anything and install nothing. Record the report as observed: expected blockers or warnings for RAM, existing configuration or sudo. If the result differs, record it as is. Keep notes for the verification record.
    - _Requirements: 22.1, 22.2, 22.4_

  - [ ] 8.2 STOP: ask the user for a disposable server
    - Stop and ask the user (`user_input`) to create a fresh Ubuntu 24.04 instance (2 vCPU, 2 to 4 GB RAM), confirm its host key fingerprint in the provider console, and provide host, user and key. Do not create any cloud resource. Do not continue to 8.3 until the user answers.
    - _Requirements: 22.2, 22.3_

  - [ ] 8.3 Provisioning checks on the disposable server
    - Run and record: preflight, plan, approval and provisioning of the selected components with every verification check, and package versions observed; the behavior of `systemd-run --remain-after-exit` on noble; a second plan on the same server with zero install steps that only verifies; state change between plan and approval giving `rePlanRequired`; expiry rejection; altered digest rejection; interruption of the SSH connection or app during `apt-install` giving UnknownOutcome with the unit still running, Reconcile resolution and no automatic retry; a fresh boot with unattended-upgrades holding the dpkg lock showing the `DPkg::Lock::Timeout` wait and the effect of `NEEDRESTART_SUSPEND=1` (record "not observed" if the timing cannot be reproduced); Doctor against the provisioned server exposing no provisioning command.
    - _Requirements: 22.1, 22.5, 22.6, 22.7, 22.8, 22.9, 22.10, 22.11, 25.4, 25.5, 25.6_

  - [ ] 8.4 Generated app check
    - Generate an app outside the repository with `--enable-provisioning` and confirm it builds. Generate one without the flag and confirm it builds and exposes no provisioning command in handler, `build.rs` or capability. Record both results.
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
    { "id": 5, "tasks": ["2.3", "3.3"] },
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
