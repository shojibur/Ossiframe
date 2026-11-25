# Implementation Plan: Application Deployment and Management (S05)

## Overview

Order: Core additions (`LinuxUserName`, `RunAs`, `stdin`), desktop store, the `app-deploy` module (including the content-write primitive and `ManagedPath`), typed IPC and client wrappers, Doctor drift confirmation, Deploy UI, a quality gate, then live verification on the existing Disposable_Server. Correctness Properties are checked with small plain `#[test]` unit tests inside the implementation tasks, with no property-testing framework. Only task 7 needs the user's server and is a hard stop before any mutation.

## Tasks

- [ ] 1. Core additions (`crates/ossiframe-core`, `crates/ossiframe-ssh`)
  - [ ] 1.1 Add `LinuxUserName`, `RunAs` and the app identifier types
    - `LinuxUserName` (pattern `^[a-z_][a-z0-9_-]{0,31}$`, rejects `root`) and `Privilege::RunAs(LinuxUserName)` in `command.rs`. `to_command_line()` renders `'/usr/bin/sudo' '-n' '-u' '<u>' '--'` followed by quoted program and args. `Privilege::Root` is unchanged.
    - `ident.rs`: `AppSlug`, `DomainName`, `GitUrl`, `BranchName`, following the `ServerName`/`Hostname` pattern.
    - `ReadOnly` rejects `RunAs` as well as `Root`.
    - Unit tests (Properties 1 and 2): RunAs rendering; `root` and out-of-pattern names rejected; ReadOnly denies both `RunAs` and `Root` with zero recorded runs; identifier boundary cases (empty, too long, traversal, metacharacters, wildcard domain, `file://`).
    - _Requirements: 2.1, 2.2, 2.3, 2.4, 2.5, 3.1, 3.2, 3.3, 3.4_

  - [ ] 1.2 Add `stdin` to `CommandSpec` and the executor
    - Optional `stdin` set only via `with_stdin`, rejected above 1 MiB, `#[serde(skip)]`, absent from `to_command_line()`. In `ossiframe-ssh`, write `stdin` then send EOF before reading output; unchanged without `stdin`.
    - Unit tests (Property 3): at or under 1 MiB accepted and rendering byte-identical; over 1 MiB rejected before any executor call; serialized JSON omits stdin.
    - _Requirements: 4.1, 4.2, 4.3, 4.4, 4.5_

  - [ ] 1.3 Add `PlanTarget.app_id`
    - `pub app_id: Option<String>` with `#[serde(default)]`. Unit test: an S04-shaped `PlanTarget` JSON deserializes with `app_id: None`.
    - _Requirements: 17.1, 17.4_

- [ ] 2. Desktop store (`crates/ossiframe-desktop`)
  - [ ] 2.1 `apps` and `releases` tables and store methods
    - Additive migration with the schema from the design (`server_id ON DELETE RESTRICT`, `entry_file`, `UNIQUE(server_id, slug)`). Insert/get/list apps, update state and current release, insert/list releases. `remove_server` is refused while any app is not Removed.
    - No engine or `remote_lock.rs` change: the S04 per-server lock and unresolved-operation rule are reused as is.
    - Unit tests: no secret column; migration idempotent; `remove_server` refused with a live app and allowed once all are Removed; `UNIQUE(server_id, slug)` enforced.
    - _Requirements: 10.4, 15.1, 16.5_

- [ ] 3. `app-deploy` module (`modules/app-deploy`, `ossiframe-mod-app-deploy`)
  - [ ] 3.1 Crate, `AppType`, `ManagedPath`, `EntryFile` and content-write helper
    - New crate depending on `ossiframe-core`, `async-trait`, `serde`, `serde_json`, `tokio`; registers zero `OperationInfo` entries. `AppType` data returned from `components()`.
    - `ManagedPath::parse` accepting only the three prefixes; `EntryFile` validated relative path; app user name builder (`ossi-<slug>` into `LinuxUserName`).
    - Content-write helper building the fixed `install` (stdin) and `mv -T` steps; `mv` never runs after a failed `install`.
    - Unit tests (Properties 1 and 4): `ManagedPath` accepts the three prefixes and rejects `/etc/passwd`, `..` paths and a missing `ossiframe-` segment; `EntryFile` rejects `..`, leading `/`, metacharacters; scripted fake executor, 4 install/mv outcomes.
    - _Requirements: 1.2, 1.3, 2.6, 2.7, 5.1, 5.2, 5.3, 5.4, 5.5_

  - [ ] 3.2 Preflight
    - Base-stack presence (git, nginx, php8.3-fpm, mysql, composer), free disk, domain collision from an `ossiframe-*` `sites-enabled` listing, existing app user check. For an SSH URL on a non-builtin host, `ssh-keyscan` as the SSH login user, returning fingerprints.
    - Unit tests: missing component blocks with its name; domain collision from a fixture listing; existing user for a different app blocks; keyscan output parsed into fingerprints.
    - _Requirements: 1.3, 6.4, 8.4, 9.9_

  - [ ] 3.3 Create_Plan: user, pool, site, database
    - Linux user step; FPM pool and NGINX site writes via the content-write helper (marker line, `php-fpm8.3 -t` / `nginx -t` before reload, `sites-enabled` symlink, optional default-site disable); MySQL step for Laravel (script on stdin, identifiers from the slug only, `ALTER USER` for idempotent retry); `.env` creation with the generated password (0600, app user).
    - Unit tests (Properties 4 and 5): marker line always present; hostile values cannot inject a directive; SQL names only `ossi_<slug>` and grants only on it; the password appears only in stdin, never in program or args; Static has no pool or database step; Php has no database step.
    - _Requirements: 2.6, 6.1, 6.2, 7.1, 7.2, 7.3, 7.4, 8.1, 8.2, 8.3, 8.5, 8.6, 10.1, 10.2, 10.3, 10.4, 10.5, 10.6_

  - [ ] 3.4 Git access: deploy key, pinned host keys, gate
    - Ship the `known_hosts` data file for github.com, gitlab.com and bitbucket.org. **Copy each host's keys from the provider's official documentation page at implementation time and record the source URL and date next to the data. Do not invent or recall key values.**
    - Steps: create `<app_root>/.ssh` (0700, app user), write `known_hosts` (0600) via the content-write helper (builtin keys, or only the scanned keys matching user-confirmed fingerprints), generate the deploy key as the app user, read back only the public key. `app_create_plan` returns `gitHostKeyUnconfirmed` with fingerprints for an unconfirmed other host. Recheck rescans and returns `rePlanRequired` on a changed key. `GIT_SSH_COMMAND` builder and `deploy-key-check` gate, both with `StrictHostKeyChecking=yes` and the app's `known_hosts`. HTTPS URLs use none of this.
    - Unit tests: builtin hosts yield a `known_hosts` write; no rendered command contains `accept-new`; unconfirmed host returns `gitHostKeyUnconfirmed` and writes nothing; only matching scanned keys are written; `GIT_SSH_COMMAND` is built only from fixed options and validated-root paths; gate parser success vs connection refusal; HTTPS URL has no key or known_hosts step.
    - _Requirements: 6.3, 9.1, 9.2, 9.3, 9.4, 9.5, 9.6, 9.7, 9.8, 9.9, 9.10, 9.11, 9.12, 9.13, 9.14_

  - [ ] 3.5 Build steps, boot check, release switch, Redeploy_Plan and Rollback_Plan
    - Per-type build steps (all `RunAs(app user)`), then the Boot_Check (Laravel `php <release_dir>/artisan --version`; Php `php -l <release_dir>/<entry_file>`; Static `test -f <release_dir>/<entry_file>`), then the atomic `current` switch and retention cleanup. A failing step or Boot_Check stops before the switch and keeps the release directory. `rollback_plan` validates the release against the app's records and on-disk state and contains only the switch.
    - Unit tests (Property 6): every build and boot-check step is `RunAs(app user)`; migrate step only when opted in; composer step for Php only when `composer.json` exists; with a scripted fake executor, a failing boot check runs no switch step; retention keeps at most 5 plus current and never removes current; rollback refused for an unknown or missing release.
    - _Requirements: 6.3, 12.1, 12.2, 12.3, 12.4, 12.5, 12.6, 13.1, 13.2, 13.3, 13.4, 13.5, 13.6, 13.7, 13.8_

  - [ ] 3.6 Remove_Plan and post-switch verification
    - `remove_plan` default (site, pool, mark Removed), `dropDatabase`, `deleteFiles`. `verify()` with the HTTP, FPM and Laravel `APP_KEY` checks; a failed essential check fails the operation and leaves `current` on the new release with no automatic rollback.
    - Unit tests: the 4 `(dropDatabase, deleteFiles)` combinations give the expected step lists, default has neither drop nor userdel; each check parser has a pass and a fail input.
    - _Requirements: 14.2, 14.3, 14.4, 14.5, 16.1, 16.2, 16.3, 16.4_

  - [ ] 3.7 `.env` read, write and redaction
    - Parse and redact secret-like keys for display; write the full file via the content-write helper to `<app_root>/shared/.env`, mode 0600, owner and group the app user.
    - Unit tests: redaction table (`DB_PASSWORD`, `APP_KEY`, `STRIPE_SECRET` redacted; `MY_KEYBOARD_LAYOUT` not); non-secret values round-trip byte-identical.
    - _Requirements: 11.1, 11.2, 11.3_

- [ ] 4. Typed IPC commands and client wrappers
  - [ ] 4.1 `app_*` commands in `ossiframe-desktop`
    - `app_types`, `app_list`, `app_get`, `app_create_plan`, `app_redeploy_plan`, `app_rollback_plan`, `app_remove_plan`, `app_approve_plan`, `app_get_env`, `app_set_env`, `app_list_releases`. `app_approve_plan` reuses the existing `PlanStore`/`ApprovedPlan` checks. New plan reasons `appNotFound`, `domainInUse`, `slugInUse`, `gitHostKeyUnconfirmed`.
    - Unit tests: parameter types contain no program, argv, path, privilege or plan-content field; no command returns the database password; a failed preflight stores no plan.
    - _Requirements: 10.7, 14.1, 14.7, 17.3_

  - [ ] 4.2 Typed client wrappers (`packages/client`)
    - Typed wrappers and DTOs for the commands above and the four new plan reasons. Check with `pnpm typecheck`.
    - _Requirements: 2.1, 17.3_

- [ ] 5. Doctor drift confirmation
  - Extend Doctor's drift tests to assert no `app_*` command name in its handler, `build.rs` manifest or capability lists, and no dependency on `ossiframe-mod-app-deploy`.
  - _Requirements: 18.1, 18.2_

- [ ] 6. Deploy app UI and quality gate
  - [ ] 6.1 App list, create flow and approval
    - Pages in `modules/app-deploy/ui/` using `@ossiframe/ui` and the client wrappers: apps per server; create flow (type, slug, domain, repo URL and branch, entry file, default-site checkbox); the git host fingerprint confirmation dialog, shown like the server host key dialog; the existing approve dialog pattern; the deploy public key shown after generation. The app detail shows "database exists", never a password.
    - _Requirements: 9.3, 9.9, 9.10, 10.7, 17.3_

  - [ ] 6.2 App detail: deploy, rollback, environment, removal
    - Release history, redeploy (branch override, migration opt-in), rollback list, environment tab (`app_get_env`/`app_set_env`, redacted display), removal with two separate off-by-default confirmations. A failed operation shows "verification failed, roll back?" as the primary action. All mutations use the existing plan/approve/operation flow.
    - _Requirements: 11.1, 11.4, 13.6, 14.6, 14.7, 16.4_

  - [ ] 6.3 Consolidated quality gate
    - `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo check` (workspace; Deploy and Doctor individually), `cargo test`, `pnpm typecheck`, `pnpm lint`, production build of Deploy, `shellcheck` for any script touched. Confirm no secret in new fixtures, logs or docs.
    - _Requirements: 18.1, 18.2_

- [ ] 7. Live verification (needs the user's Disposable_Server; does not block tasks 1 to 6)
  - Use only the existing 2 GB Lightsail server. Never touch the Doctor server. No server commands from hooks. Record real, sanitized output only. The user drives the Deploy UI.
  - [ ] 7.1 STOP: request explicit approval before any mutation
    - Ask the user (`user_input`) to confirm the test application (type, repository, domain or Host-header test) and to approve before 7.2. Run no `app_create_plan` or later step until they answer.
    - _Requirements: 19.3_
  - [ ] 7.2 Create and verify a Static or Php app
    - Create from a public HTTPS repo; approve; record steps, the boot check result and the health checks. Confirm the Default_Site is untouched unless the user chose otherwise.
    - _Requirements: 8.3, 8.5, 13.2, 14.2, 14.4_
  - [ ] 7.3 Create and verify a Laravel app with a database
    - Use an SSH repo on a builtin host (record deploy key generation, the pinned host key write and `StrictHostKeyChecking=yes` working) or a public repo if no private repo is available, and state which was used. Record database creation, `.env` mode and owner (0600, app user), a redeploy with migrations opted in, and the health checks.
    - _Requirements: 9.1, 9.3, 9.4, 9.8, 9.12, 10.1, 10.2, 10.4, 11.2, 12.1, 12.2, 14.2, 14.3_
  - [ ] 7.4 Boot check failure, redeploy and rollback
    - Deploy a commit that fails the boot check (for example a PHP syntax error) and confirm `current` is unchanged and the release is kept. Redeploy a good commit, roll back, and confirm the previous release is live. Confirm a second operation on the same server is refused while one is running.
    - _Requirements: 13.3, 13.4, 13.6, 15.2_
  - [ ] 7.5 Removal and Doctor scope check
    - Remove an app with default options and confirm the site and pool are gone while files, user and database remain; confirm the server profile cannot be removed while an app is not Removed. Confirm Doctor exposes no app-deploy command.
    - _Requirements: 16.1, 16.5, 18.2_

- [ ] 8. Verification record and roadmap
  - Write the sanitized `docs/verification/s05-application-deployment.md` from task 7 outputs only, mapping each requirement area to evidence or "not run". Record the source and date of the shipped Git host keys. Update `docs/roadmap.md` with S05 status. Add no claim that was not observed.
  - _Requirements: 19.2, 19.4_

## Notes

- No tasks are optional. Property tests are plain `#[test]` functions inside the implementation tasks.
- Task 7.1 is a hard stop before any mutating command runs against the Disposable_Server.
- Deferred in S05: user-defined deploy steps, TLS, queue workers, scheduler, database password rotation, tunable `pm.max_children`.
- Do not commit, push or publish without explicit user approval.

## Task Dependency Graph

```json
{
  "waves": [
    { "id": 0, "tasks": ["1.1"] },
    { "id": 1, "tasks": ["1.2", "1.3"] },
    { "id": 2, "tasks": ["2.1", "3.1"] },
    { "id": 3, "tasks": ["3.2"] },
    { "id": 4, "tasks": ["3.3"] },
    { "id": 5, "tasks": ["3.4"] },
    { "id": 6, "tasks": ["3.5", "3.7"] },
    { "id": 7, "tasks": ["3.6"] },
    { "id": 8, "tasks": ["4.1"] },
    { "id": 9, "tasks": ["4.2", "5"] },
    { "id": 10, "tasks": ["6.1"] },
    { "id": 11, "tasks": ["6.2"] },
    { "id": 12, "tasks": ["6.3"] },
    { "id": 13, "tasks": ["7.1"] },
    { "id": 14, "tasks": ["7.2"] },
    { "id": 15, "tasks": ["7.3"] },
    { "id": 16, "tasks": ["7.4"] },
    { "id": 17, "tasks": ["7.5"] },
    { "id": 18, "tasks": ["8"] }
  ]
}
```
