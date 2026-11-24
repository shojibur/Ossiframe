# Implementation Plan: S02 Modules and Project Generator

## Overview

Rust (Tokio, Tauri 2) and TypeScript (React). Order: spike, Core module contract, dispatch, linux-inventory extraction, shared Tauri plumbing, Doctor migration, shared React UI, starter template, ports example, generator, docs, quality gate, manual verification, evidence.

Tests are small plain `#[test]` unit tests embedded in the implementation tasks where requirements call for them. No property-based tests, no e2e suites, no commits or pushes. Mutating operations, provisioning, Laravel, Deploy and auditing are out of scope.

## Tasks

- [x] 1. Spike: cross-crate `generate_handler!` with the installed Tauri 2.12
  - [x] 1.1 Build Doctor with commands defined in `ossiframe-desktop`
    - Check the current official Tauri 2 docs and the Tauri version in `Cargo.lock` before writing code.
    - Add the optional `tauri` feature to `crates/ossiframe-desktop/Cargo.toml` (`dep:tauri`, `default-features = false`), and `pub mod tauri_support` behind `#[cfg(feature = "tauri")]`.
    - Move one or two S01 commands (for example `list_servers`, `add_server`) into `tauri_support::commands` as `pub` commands. List them by full path in Doctor's `generate_handler!`, keep Doctor's `build.rs` `AppManifest` and capability entries, and run `cargo check -p` and `cargo build` for Doctor.
    - If it fails, try an exported `macro_rules! ossiframe_commands` that expands to wrapper `#[tauri::command]` fns in the app crate. If that also fails, use thin one-line wrappers in each app.
    - Confirm that `AppManifest::commands` (names only) and `capabilities/main.json` still work.
    - _Requirements: 5.1, 5.2, 5.7, 5.8_
  - [x] 1.2 Record the spike result and chosen path
    - Write the observed outcome (what worked, error text if any, the chosen path: direct, macro or thin wrappers) into a short "Spike" section of `docs/verification/s02-modules-and-generator.md`. Real output only.
    - Tasks 5.1, 6.1 and 10.x follow the recorded path. Direct cross-crate paths are the design default, and the macro or wrappers are the fallback.
    - _Requirements: 5.7, 5.8, 15.2, 15.3_

- [x] 2. Core module contract and registry
  - [x] 2.1 Add identifiers and the `Module` contract to `ossiframe-core`
    - Move `serde_json` from dev-dependencies to runtime dependencies. Add `module.rs` with `ModuleId`, `OperationId` (`[a-z][a-z0-9-]{0,62}`, typed error, no `Deserialize` bypass), `ModuleManifest`, `OperationInfo`, `OperationContext` (exec, cancel, `observed_at`), `OperationError` and the `Module` trait (`async_trait`). Re-export from `lib.rs`.
    - Add unit tests: `ModuleId` rejects empty, uppercase, spaces, `..`, over 63 chars.
    - No Laravel, PHP, NGINX or MySQL knowledge in the contract.
    - _Requirements: 1.1, 1.2, 1.3, 1.4, 1.5, 14.1, 14.8_
  - [x] 2.2 Implement `RegistryBuilder`, `Registry` and the shared resolver
    - Add `registry.rs` with `RegistryBuilder::register/build`, `Registry::builder/manifests/run`, `RegistryError` (`DuplicateId`, `MissingDependency`, `Cycle`, `DuplicateOperation`) and `RunError` (`UnknownModule`, `UnknownOperation`, `Operation`).
    - Add `RegistryBuilder::resolve(&[ModuleManifest])`, a pure Kahn's algorithm with a `BTreeSet` ready set, used by `build` and later by the generator.
    - Add unit tests: duplicate id, missing dependency, cycle (lists the unresolved ids), duplicate operation, dependencies-first deterministic order, unknown operation not dispatched to `Module::run`.
    - _Requirements: 2.1, 2.2, 2.3, 2.4, 2.5, 2.6, 2.7, 13.1, 14.1_

- [x] 3. Generic `run_operation` dispatch
  - [x] 3.1 Implement the dispatcher in `ossiframe-desktop` (no Tauri dependency)
    - Add `dispatch.rs` with `run_operation(service, registry, server_id, module_id, operation_id, args)`: validate UUID (else `NotFound`), parse `ModuleId` and `OperationId` (validation error), cap serialized `args` at 16 KiB (`Validation` field `args`), check the module and operation exist (`NotFound`) before calling `ServerService::with_connection`.
    - Inside `with_connection`, build `OperationContext` with the `observed_at` RFC 3339 timestamp, call `Registry::run`, cap the result at 1 MiB (`Failed("result too large")`), map `ExecError::{Timeout, Cancelled, ConnectionLost}` to the existing `commandTimeout`, `cancelled`, `connectionLost` kinds, and map `OperationError::Failed` to a short message without raw output.
    - Add `list_modules` data (manifest and operations as camelCase DTOs).
    - Add unit tests: non-UUID server id, bad module and operation ids, oversized args and unknown operation are rejected before `ServerService` or `Module::run` is reached. Accept no program, argv, shell string, path, URL or privilege level.
    - Resolve only from the compiled Registry, leave `args` validation to the module in Rust, expose no Executor functionality through IPC, add no permission framework. Note in the code docs that S04 must add explicit authorization and approval boundaries before any mutating or privileged operation.
    - _Requirements: 3.1, 3.2, 3.3, 3.4, 3.5, 3.6, 3.7, 3.8, 3.9, 3.10, 3.11, 3.12, 3.13, 3.14, 14.2_

- [x] 4. Extract the `linux-inventory` module
  - [x] 4.1 Create `modules/linux-inventory` and move the inventory code
    - `git mv` `apps/doctor/src-tauri/src/inventory/{mod.rs,parse.rs}` to `modules/linux-inventory/src/{inventory.rs,parse.rs}` with the probe table, bounds and tests unchanged.
    - Create the crate `ossiframe-mod-linux-inventory` (runtime deps only `ossiframe-core`, `serde`, `async-trait`; dev dep `tokio`). Set root `Cargo.toml` `members` to include `modules/*` and `exclude = ["templates"]`.
    - Do not rewrite inventory parsing; the move relocates code only.
    - _Requirements: 4.1, 4.2, 4.11, 7.9, 13.2_
  - [x] 4.2 Implement `LinuxInventory` as a `Module`
    - Manifest id `linux-inventory`, no dependencies, one operation `server-info`. `null` args return the existing camelCase `ServerInfo`. Non-null args return `OperationError::InvalidArguments`. Pass `ctx.exec`, `ctx.cancel` and `ctx.observed_at` to `collect`. Map `ExecError` to `OperationError::Exec`.
    - Confirm the S01 inventory and parser tests pass unchanged in the new crate. Add one test for non-null args.
    - _Requirements: 4.3, 4.4, 4.5, 4.6, 13.3, 14.5_

- [x] 5. Shared Tauri commands and setup in `ossiframe-desktop` (feature `tauri`)
  - [x] 5.1 Move the nine S01 commands and `TauriEvents` into `tauri_support`
    - Follow the path recorded in task 1.2. Move `list_servers`, `add_server`, `update_server`, `remove_server`, `connect_server`, `disconnect_server`, `trust_host_key`, `replace_host_key`, `cancel_operation`, helpers (`is_uuid`, `server_id`, `token`), `TauriEvents` with `StatePayload` and `LostPayload`, and the `ids_and_tokens_are_validated` test without behavior change.
    - Export `COMMAND_NAMES` with the eleven names.
    - Verify `cargo test -p ossiframe-desktop` passes with and without `--features tauri`, and that the default build has no Tauri dependency.
    - _Requirements: 5.1, 5.2, 5.3, 5.5, 5.6_
  - [x] 5.2 Add `run_operation`, `list_modules` and `setup`
    - Add the two commands calling the dispatcher from task 3.1, with `Registry` managed as Tauri state.
    - Add `setup(app, AppSetup { db_file, registry })`: open the Store in the app data directory, use the app identifier as the keychain service name, build `ServerService` with `TauriEvents`, and manage `ServerService` and `Arc<Registry>`.
    - If the Registry build fails at startup, the app stops with the readable error text.
    - _Requirements: 2.8, 3.1, 3.2, 5.3, 5.4, 13.4_

- [x] 6. Migrate Doctor to the registry and shared plumbing
  - [x] 6.1 Rewire Doctor's Rust side
    - Remove `commands.rs` and the `inventory/` directory and the `get_server_info` command. `lib.rs` builds the Registry with only `LinuxInventory` and calls `setup` with `doctor.sqlite3`. `generate_handler!` (or the fallback from task 1.2) lists the eleven names. `build.rs` lists the same names. Drop `serde_json` and `time` dependencies if unused.
    - Update `capabilities/main.json` to the eleven `allow-` entries plus `core:event:default`. Remove the obsolete `permissions/autogenerated/get_server_info.toml` if the build does not regenerate it.
    - Add a unit test asserting `capabilities/main.json` has an `allow-` permission for every `COMMAND_NAMES` entry and no other `allow-` entries.
    - Keep the identifier `dev.ossiframe.doctor`, CSP and capability model unchanged.
    - _Requirements: 4.7, 4.8, 4.10, 5.9, 13.5_
  - [x] 6.2 Update the client API and Doctor's Overview call
    - In `packages/client`, add `runOperation` and `listModules`, and remove `getServerInfo`. Doctor's Overview loads data with `runOperation(id, "linux-inventory", "server-info", null)`.
    - Run `pnpm -r typecheck` and `pnpm -r lint` for the touched packages.
    - _Requirements: 4.7, 6.1_
  - [x] 6.3 Rebuild Doctor and recheck against the Lightsail server (manual, needs the user)
    - Run the Doctor build. Ask the user to run it at the desktop and confirm against the Lightsail server (`54.179.76.25`, user `ubuntu`; read-only): connect, host key prompt, changed host key behavior as available, credential prompt, cancel, stale marking and overview data, the same as S01.
    - Record only what the user actually observes. Do not fabricate results. Anything not confirmed goes on the not-verified list for task 15.
    - _Requirements: 4.9, 15.1, 15.4_

- [x] 7. Move the shared React connection UI into packages
  - [x] 7.1 Add `useServerConnections` to `@ossiframe/client`
    - Move the connection logic out of `apps/doctor/src/App.tsx`: 15 s server list refresh, `connection-state` and `connection-lost` events, connect flow, in-memory pending credential cleared when the flow ends or is superseded, dialog state (`form | credential | hostKey | changed | remove`), per-server busy, error and warning maps. No JSX.
    - _Requirements: 6.2, 6.3_
  - [x] 7.2 Move dialogs, formatters and `ServerWorkspace` into `@ossiframe/ui`
    - Move `ServerForm`, the credential, host key, changed host key and remove dialogs, `format.ts` and `StateBadge` from Doctor. Add `ServerWorkspace` with a `children({ server, connected })` slot and tabs from a pages list. Add `@ossiframe/client` as a dependency of `@ossiframe/ui`.
    - Render remote data as text. Keep `react/no-danger` enforced. No restyling and no new UI libraries.
    - _Requirements: 6.4, 6.5, 6.6_
  - [x] 7.3 Add the `linux-inventory` optional UI folder and shrink Doctor's `App.tsx`
    - Create `modules/linux-inventory/ui/` (plain `.tsx` files, no pnpm package) exporting `moduleUi = { id: "linux-inventory", title, Page }` with the Overview moved from Doctor.
    - Doctor imports that page directly by relative path and lists it in `src/modules.ts`; `App.tsx` is `ServerWorkspace` plus the pages. Delete the moved files from Doctor. Verify `@ossiframe/ui` and `@ossiframe/client` resolve from the `ui/` location; if not, add a Vite alias and tsconfig `paths` entry in Doctor. No Doctor UI redesign.
    - Run `pnpm -r typecheck`, `pnpm -r lint` and the Doctor frontend production build. UI behavior stays as in S01.
    - _Requirements: 4.9, 4.12, 6.7, 6.8, 6.9, 6.10_

- [x] 8. Starter template
  - [x] 8.1 Create the starter app files
    - Under `templates/starter/` add `package.json`, `pnpm-workspace.yaml` (`packages/*`), `tsconfig.json`, `eslint.config.js`, `.gitignore`, `index.html`, `vite.config.ts`, `src/{main.tsx,App.tsx,modules.ts,index.css}` and `src-tauri/{Cargo.toml,build.rs,tauri.conf.json,capabilities/main.json,src/{main.rs,lib.rs}}`. Copy icons from Doctor.
    - Use only the ten allowed placeholders. Use `app.sqlite3`. `tauri.conf.json` copies Doctor's CSP, `freezePrototype` and `withGlobalTauri: false`. The capability file grants `core:event:default` and `allow-` for `COMMAND_NAMES` only, with no fs, shell, http, opener or dialog plugins. The starter registers zero modules and builds. Its `Connection` tab shows profile, endpoint, connection state and pinned fingerprint from the existing server list data (confirm the fingerprint field is available there). `linux-inventory` and its page are not in the starter; the generator adds them only when selected.
    - Include in the starter `src-tauri` the same capability drift test as Doctor.
    - _Requirements: 5.9, 7.1, 7.2, 7.3, 7.4, 7.5, 7.8, 7.10, 9.5, 9.6_
  - [x] 8.2 Add the starter steering, skill and README
    - Add `.kiro/steering/{product,tech,structure}.md` (structured commands only, secrets only in the keychain, typed operations only, do not edit vendored `packages/`).
    - Add `.kiro/skills/ossiframe-module/SKILL.md` (at most 120 lines) covering the Module trait, manifest rules, `CommandSpec`, output bounding and parsing, arg validation, fake-executor tests, registration, adding a UI page, and the Ports module as the worked example. State that modules are trusted code and the manifest is not a sandbox.
    - Add the template `README.md` with the pinned-commit and `--ossiframe-local` notices (rendered by the generator).
    - _Requirements: 7.1, 7.6, 7.7, 10.5, 10.6, 12.4_

- [x] 9. Custom `ports` example module
  - [x] 9.1 Create the `ossiframe-mod-ports-example` crate
    - In `examples/custom-module/`, id `ports`, no dependencies, one operation `listening-tcp`. Runtime deps only `ossiframe-core`, `serde`, `async-trait`. Add `examples/*` to root workspace members.
    - Run `ss -Htln` (fixed program and argv, 10 s timeout, no `-p`). Args are `null` or `{ "family": "ipv4" | "ipv6" | "all" }`, validated in Rust (typed enum, unknown keys and values return `InvalidArguments`); the filter applies to parsed sockets and never reaches argv. The crate lives outside Core, uses only the public SDK and imports nothing from Doctor. Parser: cut input at 64 KiB, at most 512 entries, address at most 128 chars, strip control characters, skip and count unparsable lines, parse IPv4, `[::]:22` and `*:80`. Result `{ observedAt, sockets: [{ localAddress, port, process: null }], skipped, truncated }`.
    - Add unit tests: parser on IPv4, IPv6, wildcard, malformed lines and oversized input, `family` args validation (valid, unknown value, unknown key, wrong type), plus one module test with a fake `Executor`.
    - _Requirements: 11.1, 11.2, 11.3, 11.4, 11.5, 11.6, 11.7, 11.9, 11.10, 13.2, 13.3, 14.4_
  - [x] 9.2 Create the optional `ports` page
    - Add `examples/custom-module/ui/` (plain `.tsx` files, no pnpm package) exporting `moduleUi` with id `ports`: a family selector and a table that renders addresses and ports as text using `@ossiframe/ui`, calling `runOperation`. No change to Core or Doctor is needed.
    - Run `pnpm -r typecheck` and `pnpm -r lint` (include the folder in a workspace tsconfig or the template check so it is type-checked).
    - _Requirements: 6.8, 6.9, 11.8, 11.10_

- [x] 10. Generator CLI `tools/ossiframe-cli`
  - [x] 10.1 Create the crate with argument parsing and input validation
    - Crate `ossiframe-cli` with `clap` (derive), command `new <dir> --name --identifier [--modules] [--with-example-module] [--rev] [--ossiframe-local] [--allow-ossiframe-namespace]`. Add `tools/*` to root workspace members.
    - Validate name (1 to 64 chars, starts with a letter, `[A-Za-z0-9 _-]`), identifier (two or more segments, each `[a-z][a-z0-9-]*`, at most 128 chars, not ending `.app`, `dev.ossiframe.` refused without the flag), and `<dir>` (absent or empty directory only). One-line error, non-zero exit, nothing written.
    - Add unit tests with hostile inputs (`../x`, quotes, `\`, `"; rm`, empty, control characters) and a non-empty directory.
    - _Requirements: 8.1, 8.2, 8.3, 8.4, 8.5, 8.11, 14.3_
  - [x] 10.2 Add module resolution, rev pinning and token rendering
    - Read manifests from the module crates (`linux-inventory`, ports example). Include only explicitly selected modules plus their required dependencies; no selection means zero modules. Reject unknown modules, unknown dependencies and cycles through `RegistryBuilder::resolve`, before writing.
    - Rev: `--rev`, else `git rev-parse HEAD`. Fail asking for `--rev` if `git` is unavailable. Refuse when the working tree has uncommitted changes and neither `--rev` nor `--ossiframe-local` is given. No network access.
    - Add the plain string `{{key}}` renderer that fails on unknown tokens or a leftover `{{` in any text file. Binary files are copied unchanged. Read the template from disk at run time.
    - Add unit tests: unknown token, leftover `{{`, unknown module, unknown dependency, cycle, zero-module selection, automatic inclusion of required dependencies, dirty-tree refusal.
    - _Requirements: 2.7, 8.6, 8.7, 8.8, 8.9, 8.10, 8.13, 8.14, 10.1, 10.2, 10.8, 14.3_
  - [x] 10.3 Render, vendor and finalize the generated app
    - Render the starter into a temporary sibling directory, then rename to `<dir>` only on success. Remove the temp directory on failure.
    - For each selected module in resolved order, write the Cargo dependency and `.register(...)`, plus the TS import and `pages` entry only if the module has a `ui/` folder. Copy `packages/ui` and `packages/client` into `<dir>/packages/` with `workspace:*` dependencies, and copy each selected module's `ui/` files into `<dir>/src/modules/<id>/`. With zero modules, emit the starter with an empty registry and no module pages. `--with-example-module` copies `examples/custom-module` to `<dir>/modules/ports/` and rewrites its `ossiframe-core` dependency to the app's source.
    - In Standalone_Mode write all Ossiframe crates as Git dependencies on `https://github.com/shojibur/Ossiframe` with one `rev` (`{ git = "https://github.com/shojibur/Ossiframe", rev = "<40-hex>" }`), rendered through the whole-line `{{ossiframe_deps}}` token. The ports example is always `path = "../modules/ports"` inside the app. `--ossiframe-local <path>` requires `<path>/crates/ossiframe-core/Cargo.toml`, renders plain `path = "<canonical local checkout>/crates/<crate>"` dependencies (module crates via their local paths) with no git source and no `[patch]` section, prints a warning, and adds "local testing only; remove before sharing" to the generated README. The output and README state that Standalone_Mode needs the pinned commit to be a Published_Rev and that `--ossiframe-local` is Local_Mode only. The app gets its own name, identifier, data directory, keychain service namespace, README and `.kiro` guidance.
    - Add unit tests: generated CSP equals Doctor's CSP, no manifest `path =` entry outside the project without `--ossiframe-local`, bad `--ossiframe-local` path writes nothing.
    - _Requirements: 8.12, 9.1, 9.2, 9.3, 9.4, 9.5, 9.6, 10.1, 10.3, 10.4, 10.5, 10.6, 10.7, 10.9, 10.10, 14.3_

- [x] 11. Developer quick-start
  - Write `docs/developer-guide.md` (at most 150 lines): generate an app, pin Ossiframe or use a local checkout, `pnpm install` and `pnpm tauri dev`, write a module, register it, add its page, test with a fake executor, build. State that modules are trusted code, the manifest is not a sandbox, all S02 operations are read-only, and S04 must add explicit authorization and approval boundaries before any mutating or privileged operation. Add a link in `README.md` of at most two lines.
  - _Requirements: 12.1, 12.2, 12.3, 12.4_

- [x] 12. Consolidated quality gate
  - Run and fix until clean: `cargo fmt --all --check`, `cargo check --workspace --all-targets`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `pnpm -r typecheck`, `pnpm -r lint`, `cargo audit`, `pnpm audit`, the Doctor production build, and `shellcheck` for any added script.
  - Confirm the new dependencies (`serde_json`, optional `tauri`, `clap`) are maintained and permissively licensed.
  - Report real command results. If a tool is not installed, say so and do not claim the check passed.
  - _Requirements: 13.2, 13.6, 14.1, 14.2, 14.3, 14.4, 14.5, 14.6, 14.7, 14.8_

- [x] 13. Verify generated apps in Local_Mode on the Lightsail server
  - [x] 13.1 Generate and build a minimal app and an app with modules
    - From the repository run `ossiframe-cli new ~/ossiframe-test/min --name "Min Ops" --identifier dev.example.min --ossiframe-local <repo path>` (no modules) and `ossiframe-cli new ~/ossiframe-test/acme --name "Acme Ops" --identifier dev.example.acme --modules linux-inventory --with-example-module --ossiframe-local <repo path>`. Run `pnpm install` and `pnpm tauri build` in each generated directory.
    - Inspect each generated `capabilities/main.json` and confirm only the eleven Shared_Commands. Record this as Local_Mode only. This task must not commit or push.
    - _Requirements: 8.8, 9.1, 9.2, 9.5, 10.3, 10.5, 15.5, 15.7, 16.1_
  - [x] 13.2 Run both generated apps against the Lightsail server (manual, needs the user)
    - The user runs each built app and connects to `54.179.76.25` (user `ubuntu`, read-only, disposable server named by the user). Minimal app: basic connection info (profile, endpoint, state, pinned fingerprint) and no inventory page. Acme app: `linux-inventory` overview and the `ports` page with real listening sockets, compared by the user against `ss -tln` run on the server. Try each `family` value on the ports page.
    - Record only what the user reports. Never fabricate output.
    - _Requirements: 7.8, 11.3, 11.8, 15.1, 15.6, 16.1_

- [x] 14. Verify Standalone_Mode with a Published_Rev
  - [x] 14.1 Stop and request user approval to publish the commit
    - Stop. Ask the user to approve committing and pushing the S02 implementation to the public repository `https://github.com/shojibur/Ossiframe`, and to name the revision (sha or tag) to use. Do not commit or push without that approval, and never push from this task. Continue only after the user confirms the revision is published.
    - _Requirements: 10.6, 15.8, 16.1_
  - [x] 14.2 Generate and build outside the repository from the Published_Rev
    - In a directory without the monorepo checkout, run `ossiframe-cli new` with `--rev <published rev>`, no `--ossiframe-local`, `--modules linux-inventory --with-example-module`. Run `pnpm install` and `pnpm tauri build`. The generator must run from a clean tree (a dirty tree is refused).
    - `grep` the generated manifests to confirm every Ossiframe dependency, including selected modules, resolves from the Git rev and no `path =` entry leaves the project. Connect to the Lightsail server and confirm the overview and ports page work (user observes). A dirty local checkout is not proof.
    - _Requirements: 10.1, 10.7, 10.11, 15.5, 15.8, 16.1_

- [x] 15. Record evidence and confirm completion
  - Complete `docs/verification/s02-modules-and-generator.md` with real output only: spike result, Doctor recheck, Registry and Resolver test results, minimal and module app builds, Lightsail runs, `ss -tln` comparison, Doctor registers only `linux-inventory`, capability files list only the eleven commands, quality gate results, Local_Mode vs Standalone_Mode results, and the custom `ports` module added without a Core change.
  - Check the S02 completion list (Requirement 16) item by item: (1) Doctor connects and shows real inventory; (2) Doctor consumes `linux-inventory`; (3) Registry and Resolver work; (4) minimal app generated; (5) app with selected modules generated; (6) generated app builds outside the monorepo; (7) generated app connects to the Lightsail server; (8) ports module retrieves real listening ports; (9) custom module added without Core changes; (10) Standalone_Mode verified from a Published_Rev; (11) `docs/developer-guide.md` exists; (12) results recorded. Mark any item not met as not complete.
  - List every item not verified. Keep the S01 open checks documented as open and outside S02 completion: independent fingerprint comparison through the Lightsail console pending, password authentication unverified and disabled.
  - Update `docs/roadmap.md` and `docs/compatibility.md` only with facts observed in this verification.
  - _Requirements: 15.2, 15.3, 15.4, 15.7, 15.8, 15.9, 16.1, 16.2_

## Notes

- No task is optional. Unit tests are embedded in the implementation tasks that need them.
- Tasks 6.3, 13.2 and 14.1 need the user. Task 14.1 stops and requests approval to publish; the agent never commits or pushes on its own.
- Tasks 5.1, 6.1 and the starter follow the cross-crate path recorded in task 1.2.
- `linux-inventory` is optional for generated apps. Module UI is an optional `ui/` folder, not a package.
- S02 operations are read-only; S04 must add authorization and approval boundaries before any mutating operation.

## Task Dependency Graph

```json
{
  "waves": [
    { "id": 0, "tasks": ["1.1"] },
    { "id": 1, "tasks": ["1.2"] },
    { "id": 2, "tasks": ["2.1"] },
    { "id": 3, "tasks": ["2.2"] },
    { "id": 4, "tasks": ["3.1", "4.1"] },
    { "id": 5, "tasks": ["4.2"] },
    { "id": 6, "tasks": ["5.1"] },
    { "id": 7, "tasks": ["5.2"] },
    { "id": 8, "tasks": ["6.1"] },
    { "id": 9, "tasks": ["6.2"] },
    { "id": 10, "tasks": ["6.3"] },
    { "id": 11, "tasks": ["7.1"] },
    { "id": 12, "tasks": ["7.2"] },
    { "id": 13, "tasks": ["7.3"] },
    { "id": 14, "tasks": ["8.1"] },
    { "id": 15, "tasks": ["8.2", "9.1"] },
    { "id": 16, "tasks": ["9.2", "10.1"] },
    { "id": 17, "tasks": ["10.2"] },
    { "id": 18, "tasks": ["10.3"] },
    { "id": 19, "tasks": ["11"] },
    { "id": 20, "tasks": ["12"] },
    { "id": 21, "tasks": ["13.1"] },
    { "id": 22, "tasks": ["13.2"] },
    { "id": 23, "tasks": ["14.1"] },
    { "id": 24, "tasks": ["14.2"] },
    { "id": 25, "tasks": ["15"] }
  ]
}
```
