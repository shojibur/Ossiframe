# Requirements Document

## Introduction

S02 turns the S01 foundation into a reusable framework. It adds a minimal module contract and registry to Core, a generic `run_operation` dispatch, a `linux-inventory` module extracted from Doctor, shared Tauri plumbing and React connection UI, a starter template, a repository-local generator, a custom `ports` example module, and a developer quick-start. A third-party developer generates an app outside the Ossiframe repository, selects modules, adds their own module, and connects to an authorized server without editing Core.

Testing is limited to small plain unit tests for high-risk logic, plus manual verification against the named Lightsail server. No property-based testing and no e2e suites.

## Glossary

- **Core**: The `ossiframe-core` crate.
- **Desktop_Crate**: The `ossiframe-desktop` crate, including its optional `tauri` feature.
- **Module**: A trusted compile-time Rust crate implementing the `Module` trait.
- **Module_Manifest**: Metadata of a Module (id, name, version, description, dependencies). It is not a sandbox.
- **Module_Id**: A validated identifier matching `[a-z][a-z0-9-]{0,62}`.
- **Operation_Id**: A validated identifier with the same rules as Module_Id.
- **Registry**: The `Registry` built by `RegistryBuilder` from explicitly registered Modules.
- **Resolver**: `RegistryBuilder::resolve`, the pure function ordering manifests by dependency.
- **Dispatcher**: The `run_operation` Tauri command and `Registry::run`.
- **Linux_Inventory**: The `ossiframe-mod-linux-inventory` Module with operation `server-info`.
- **Doctor**: The Ossiframe Doctor application in `apps/doctor`.
- **Shared_Commands**: The eleven Tauri commands named in `COMMAND_NAMES`, defined in Desktop_Crate.
- **Client_Package**: `@ossiframe/client`.
- **UI_Package**: `@ossiframe/ui`.
- **Server_Workspace**: The `ServerWorkspace` React component in UI_Package.
- **Module_UI**: Optional `.tsx` files in a Module's `ui/` folder, exporting a `moduleUi` object. Backend-only Modules have none.
- **Starter_Template**: The directory `templates/starter/`.
- **Generator**: The `ossiframe-cli` tool, command `new`.
- **Generated_App**: An application produced by Generator.
- **Ports_Module**: The `ossiframe-mod-ports-example` Module (id `ports`) in `examples/custom-module/`.
- **Lightsail_Server**: The disposable Ubuntu 24.04 server the user named for verification (`<server-ip>`, user `ubuntu`).
- **Published_Rev**: A Git commit that exists on the public remote `https://github.com/shojibur/Ossiframe`. A dirty or unpushed local checkout is not a Published_Rev.
- **Local_Mode**: Generated_App built with `--ossiframe-local` (plain Cargo `path` dependencies on a local checkout, no git source), for development only.
- **Standalone_Mode**: Generated_App built from a Published_Rev with no monorepo checkout.
- **Quality_Gates**: The checks listed in Requirement 14.

## Non-Goals

- Provisioning, Laravel support, and the Deploy application.
- Auditing rules (S03).
- Publishing to npm or crates.io.
- Dynamic or runtime-loaded plugins, and a marketplace.
- Mutating operations, privileged execution, and a mutating-operation marker in the Module contract (S04).
- New crates beyond `ossiframe-mod-linux-inventory`, `ossiframe-cli`, and the Ports_Module crate.
- Restyling the UI or adding UI libraries.
- Checking the public remote for the pinned commit from the Generator.
- Property-based testing, fixture suites, and e2e suites.
- A permission framework. S04 must add explicit authorization and approval boundaries before any mutating or privileged operation.
- Closing S01 open checks (independent fingerprint comparison through the Lightsail console, password authentication). Password authentication stays unverified and disabled. These items stay documented as open and are not part of S02 completion.

## Requirements

### Requirement 1: Module contract and identifiers

**User Story:** As a module author, I want a small public contract in Core, so that I can add a module without changing Core.

#### Acceptance Criteria

1. THE Core SHALL expose the `Module` trait, Module_Manifest, `OperationInfo`, `OperationContext`, `OperationError`, Module_Id and Operation_Id as public types.
2. WHEN a Module_Id or Operation_Id string is constructed, THE Core SHALL accept only strings matching `[a-z][a-z0-9-]{0,62}`.
3. IF a Module_Id or Operation_Id string is empty, contains uppercase letters, spaces or `..`, or exceeds 63 characters, THEN THE Core SHALL reject it with a typed error.
4. THE Core SHALL provide `OperationContext` with exactly an executor reference, a cancel token and an RFC 3339 `observed_at` value.
5. THE Core SHALL contain no knowledge of Laravel, PHP, NGINX or MySQL in the module contract.

### Requirement 2: Registry and dependency resolution

**User Story:** As an app developer, I want modules registered explicitly and ordered by dependency, so that startup fails clearly on a bad module set.

#### Acceptance Criteria

1. THE Registry SHALL be built only from Modules passed to `RegistryBuilder::register`.
2. IF two registered Modules share a Module_Id, THEN THE Registry build SHALL fail with `RegistryError::DuplicateId`.
3. IF a Module declares a dependency that is not registered, THEN THE Registry build SHALL fail with `RegistryError::MissingDependency`.
4. IF the dependency graph contains a cycle, THEN THE Registry build SHALL fail with `RegistryError::Cycle` listing the unresolved Module_Ids.
5. IF a Module lists the same operation id twice, THEN THE Registry build SHALL fail with `RegistryError::DuplicateOperation`.
6. WHEN a valid set of Modules is built, THE Registry SHALL return manifests with dependencies before dependents and ties ordered by Module_Id.
7. THE Generator and THE Registry SHALL use the same Resolver implementation over manifests.
8. IF the Registry build fails at app startup, THEN THE app SHALL stop with the readable error text.

### Requirement 3: Generic `run_operation` dispatch

**User Story:** As a frontend developer, I want one typed command to call module operations, so that adding a module needs no ACL or command edits.

#### Acceptance Criteria

1. THE Desktop_Crate SHALL expose `run_operation(serverId, moduleId, operationId, args)` returning a JSON value.
2. THE Desktop_Crate SHALL expose `list_modules` returning each Module's id, name, version, description, dependencies and operations.
3. IF `serverId` is not a UUID, THEN THE Dispatcher SHALL return `NotFound` without calling `ServerService`.
4. IF `moduleId` or `operationId` fails Module_Id or Operation_Id validation, THEN THE Dispatcher SHALL return a validation error without calling `ServerService`.
5. IF the serialized `args` exceed 16 KiB, THEN THE Dispatcher SHALL return `Validation` with field `args` without calling `ServerService`.
6. IF the named Module or operation is not registered, THEN THE Dispatcher SHALL return `NotFound` and SHALL NOT call `Module::run`.
7. WHEN the Dispatcher runs an operation, THE Dispatcher SHALL execute it inside `ServerService::with_connection` so that per-server serialization, cancel and lost-connection handling apply.
8. IF a result exceeds 1 MiB, THEN THE Dispatcher SHALL return `Failed("result too large")`.
9. WHEN an operation returns `ExecError::Timeout`, `Cancelled` or `ConnectionLost`, THE Dispatcher SHALL map it to the existing `commandTimeout`, `cancelled` or `connectionLost` error kinds.
10. IF an operation returns `OperationError::Failed`, THEN THE Dispatcher SHALL return a short message that excludes raw command output.
11. THE Dispatcher SHALL accept no program, argv, shell string, executable, filesystem path, URL or privilege level from the frontend.
12. THE Dispatcher SHALL resolve operations only from the app's compiled Registry, SHALL leave `args` validation to the Module in Rust, and SHALL NOT expose Executor functionality directly through IPC.
13. ALL S02 operations SHALL be read-only by design, and no privileged or mutating operation SHALL exist in S02. Module metadata is not a sandbox.
14. THE design and developer guide SHALL state that S04 must add explicit authorization and approval boundaries before any mutating or privileged operation. S02 SHALL add no permission framework.

### Requirement 4: Linux inventory module extraction

**User Story:** As a Doctor user, I want the same overview as before, so that the refactor does not change behavior.

#### Acceptance Criteria

1. THE Linux_Inventory crate SHALL contain the inventory probe table, parsers, bounds and tests moved from `apps/doctor/src-tauri/src/inventory/`.
2. THE Linux_Inventory crate SHALL depend only on `ossiframe-core`, `serde` and `async-trait` at runtime.
3. THE Linux_Inventory Module SHALL declare id `linux-inventory`, no dependencies, and one operation `server-info`.
4. WHEN `server-info` is run with `null` args, THE Linux_Inventory Module SHALL return the existing camelCase `ServerInfo` JSON.
5. IF `server-info` receives non-null args, THEN THE Linux_Inventory Module SHALL return `OperationError::InvalidArguments`.
6. WHEN `server-info` runs, THE Linux_Inventory Module SHALL pass `ctx.exec`, `ctx.cancel` and `ctx.observed_at` to the existing `collect` function.
7. THE Doctor SHALL remove the `get_server_info` command and call `runOperation(id, "linux-inventory", "server-info", null)`.
8. THE Doctor SHALL register only Linux_Inventory.
9. THE Doctor SHALL keep its S01 behavior for connect, host key prompt, changed host key, credential prompt, cancel, stale marking and overview data.
10. THE Doctor SHALL keep `doctor.sqlite3`, the app identifier `dev.ossiframe.doctor`, its CSP and its capability model unchanged.
11. THE Doctor SHALL consume Linux_Inventory only through public Core and Desktop_Crate interfaces, and SHALL NOT change SSH transport, credential storage or inventory parsing behavior.
12. THE Doctor SHALL keep its S01 UI without redesign.

### Requirement 5: Shared Tauri plumbing behind the `tauri` feature

**User Story:** As an app developer, I want connection commands shared, so that Doctor and Generated_Apps do not duplicate them.

#### Acceptance Criteria

1. THE Desktop_Crate SHALL declare `tauri` as an optional dependency enabled only by the `tauri` feature.
2. WHILE the `tauri` feature is disabled, THE Desktop_Crate SHALL build and pass its S01 tests without Tauri.
3. WHERE the `tauri` feature is enabled, THE Desktop_Crate SHALL provide the Shared_Commands: `list_servers`, `add_server`, `update_server`, `remove_server`, `connect_server`, `disconnect_server`, `trust_host_key`, `replace_host_key`, `cancel_operation`, `run_operation`, `list_modules`.
4. WHERE the `tauri` feature is enabled, THE Desktop_Crate SHALL provide `setup` that opens the Store in the app data directory, uses the app identifier as the keychain service name, builds `ServerService` with Tauri events, and manages the Registry as Tauri state.
5. THE Desktop_Crate SHALL export `COMMAND_NAMES` listing the eleven Shared_Commands.
6. THE Desktop_Crate SHALL carry the moved nine S01 command behaviors, `TauriEvents` payloads and the `ids_and_tokens_are_validated` test without behavior change.
7. THE first implementation task SHALL build Doctor with commands defined in Desktop_Crate and record whether cross-crate `generate_handler!` works.
8. IF cross-crate `generate_handler!` fails, THEN THE Desktop_Crate SHALL fall back first to an exported `ossiframe_commands` macro, then to thin one-line wrappers in each app, and the verification record SHALL state which fallback was used.
9. THE Doctor and THE Starter_Template SHALL each include a test asserting that `capabilities/main.json` has an `allow-` permission for every `COMMAND_NAMES` entry and no other `allow-` entries beyond the app's own.

### Requirement 6: Shared React connection code and UI

**User Story:** As an app developer, I want the connection flow in shared packages, so that a Generated_App gets it without copying Doctor.

#### Acceptance Criteria

1. THE Client_Package SHALL expose `runOperation` and `listModules` and SHALL NOT expose `getServerInfo`.
2. THE Client_Package SHALL provide a `useServerConnections` hook holding server list refresh, connection events, connect flow, in-memory pending credential handling, dialog state and per-server busy, error and warning maps.
3. WHEN a connection flow ends or is superseded, THE `useServerConnections` hook SHALL clear the pending credential.
4. THE UI_Package SHALL provide the server form, credential dialog, host key dialog, changed host key dialog, remove confirmation dialog, formatters, state badge and Server_Workspace.
5. THE Server_Workspace SHALL render a children slot receiving `{ server, connected }`.
6. THE UI_Package SHALL render remote data as text and SHALL keep `react/no-danger` enforced.
7. THE Doctor `App.tsx` SHALL consist of Server_Workspace and module pages, with its UI behavior unchanged from S01.
8. WHERE a Module provides Module_UI, THE Module_UI SHALL export `{ id, title, Page }` where `id` equals the Rust Module_Id. Module_UI is optional and backend-only Modules are supported.
9. THE app SHALL list module pages explicitly in `src/modules.ts`. An app MAY build its own UI from operation results typed through the Client_Package `runOperation` wrapper.
10. THE Doctor SHALL import the Linux_Inventory page directly from `modules/linux-inventory/ui/`.

### Requirement 7: Starter template

**User Story:** As a third-party developer, I want a starter template, so that my app begins with the same secure shell as Doctor.

#### Acceptance Criteria

1. THE Starter_Template SHALL contain the frontend shell, `src-tauri` crate, `tauri.conf.json`, capability file, icons copied from Doctor, and a README.
2. THE Starter_Template `tauri.conf.json` SHALL use the same CSP, `freezePrototype` and `withGlobalTauri: false` values as Doctor.
3. THE Starter_Template capability file SHALL grant `core:event:default` and `allow-` permissions for `COMMAND_NAMES` only, with no fs, shell, http, opener or dialog plugins.
4. THE Starter_Template SHALL use only the placeholders `{{product_name}}`, `{{identifier}}`, `{{package_name}}`, `{{lib_name}}`, `{{ossiframe_deps}}`, `{{module_deps}}`, `{{module_registrations}}`, `{{module_ui_imports}}` and `{{module_ui_list}}`. `{{ossiframe_deps}}` is a whole-line token that expands to the `ossiframe-core` and `ossiframe-desktop` dependency lines.
5. THE Starter_Template SHALL use `app.sqlite3` as its database file.
6. THE Starter_Template SHALL include `.kiro/steering/{product,tech,structure}.md` stating that apps use structured commands only, keep secrets only in the keychain, expose typed operations only, and do not edit vendored `packages/`.
7. THE Starter_Template SHALL include `.kiro/skills/ossiframe-module/SKILL.md` of at most 120 lines covering the Module trait, manifest rules, `CommandSpec` use, output bounding and parsing, arg validation, fake-executor tests, registration and adding a UI page, with the Ports_Module as the worked example.
8. THE Starter_Template SHALL generate, build and run with zero optional Modules. Without Linux_Inventory, THE app SHALL show basic server connection info (profile, endpoint, connection state, pinned fingerprint) and no inventory page.
9. THE root `Cargo.toml` SHALL exclude `templates`.
10. THE Starter_Template SHALL include Linux_Inventory, its registration and its page only when the Generator selects it.

### Requirement 8: Generator input validation

**User Story:** As a developer, I want the generator to reject bad input before writing anything, so that it cannot damage my filesystem or build a broken app.

#### Acceptance Criteria

1. WHEN the user runs `ossiframe-cli new <dir> --name <name> --identifier <id>`, THE Generator SHALL create a Generated_App in `<dir>`.
2. IF `--name` is empty, exceeds 64 characters, does not start with a letter, or contains characters outside `[A-Za-z0-9 _-]`, THEN THE Generator SHALL exit non-zero with a one-line reason and write nothing.
3. IF `--identifier` has fewer than two dot-separated segments, has a segment not matching `[a-z][a-z0-9-]*`, exceeds 128 characters, or ends with `.app`, THEN THE Generator SHALL exit non-zero with a one-line reason and write nothing.
4. IF `<dir>` exists and is not an empty directory, THEN THE Generator SHALL refuse and write nothing.
5. IF `--identifier` starts with `dev.ossiframe.` and `--allow-ossiframe-namespace` is not given, THEN THE Generator SHALL refuse.
6. IF `--modules` names an unknown module, THEN THE Generator SHALL exit non-zero and write nothing.
7. IF a required dependency is not a known Module or the dependency graph has a cycle, THEN THE Generator SHALL exit non-zero and write nothing.
8. THE Generator SHALL include only explicitly selected Modules plus their required dependencies. WHEN neither `--modules` nor `--with-example-module` is given, THE Generator SHALL generate an app with zero Modules.
9. IF `--rev` is not given and `git` is unavailable, THEN THE Generator SHALL exit non-zero with a message asking for `--rev`.
10. IF the repository working tree has uncommitted changes and neither `--rev` nor `--ossiframe-local` is given, THEN THE Generator SHALL refuse with a one-line reason and write nothing.
11. THE Generator SHALL build output paths only from validated components.
12. THE Generator SHALL render into a temporary sibling directory and rename it to `<dir>` only after rendering succeeds, and SHALL remove the temporary directory on failure.
13. THE Generator SHALL replace `{{key}}` tokens with a plain string function and fail if an unknown token or a leftover `{{` remains in any text file.
14. THE Generator SHALL read the Starter_Template from disk at run time.

### Requirement 9: Generated app contents and module selection

**User Story:** As a developer, I want the generated app to include exactly the modules I selected, so that registration and UI match.

#### Acceptance Criteria

1. WHEN modules are selected, THE Generator SHALL write for each module in resolved order a Cargo dependency and a `.register(...)` line, and, only when the module has a `ui/` folder, a TS import and a `pages` entry.
2. WHEN `--with-example-module` is given, THE Generator SHALL copy `examples/custom-module` to `<dir>/modules/ports/` and rewrite its `ossiframe-core` dependency to the same source as the app.
3. THE Generator SHALL copy `packages/ui` and `packages/client` into `<dir>/packages/`, list `packages/*` in `pnpm-workspace.yaml`, and copy a selected module's `ui/` files into `<dir>/src/modules/<id>/` only when that module is selected.
4. THE Generated_App SHALL depend on vendored frontend packages through `workspace:*`.
5. THE Generated_App SHALL expose only the eleven Shared_Commands in its capability file.
6. THE Generated_App SHALL use its validated identifier as the keychain service name and app data directory identity.

### Requirement 10: Distribution

**User Story:** As a developer, I want a generated app to build outside the Ossiframe repository, so that I can ship it independently.

#### Acceptance Criteria

1. WHERE `--ossiframe-local` is not given (Standalone_Mode), THE Generator SHALL write each Ossiframe Rust dependency as a Git dependency on `https://github.com/shojibur/Ossiframe` with the same `rev` value.
2. WHEN `--rev` is not given, THE Generator SHALL use the local `git rev-parse HEAD`.
3. WHEN `--ossiframe-local <path>` is given, THE Generator SHALL write each Ossiframe crate dependency as a plain `path = "<canonical local checkout>/crates/<crate>"` dependency (module crates via their local paths), with no git source and no `[patch]` section.
4. IF `<path>/crates/ossiframe-core/Cargo.toml` does not exist, THEN THE Generator SHALL reject `--ossiframe-local` and write nothing.
5. WHEN `--ossiframe-local` is used, THE Generator SHALL print a warning and THE Generated_App README SHALL mark it "local testing only; remove before sharing".
6. THE Generator output and THE Generated_App README SHALL state that Standalone_Mode requires the pinned commit to be a Published_Rev, and that Local_Mode is for development only.
7. WHERE `--ossiframe-local` is not given, THE Generated_App SHALL contain no manifest `path =` entry pointing outside its project directory.
8. THE Generator SHALL perform no network access.
9. THE Generated_App SHALL have its own name, identifier, data directory, keychain service namespace derived from the identifier, selected Modules, build configuration, README and `.kiro` guidance.
10. THE Generator SHALL copy frontend packages into the app and SHALL NOT require npm or crates.io publishing.
11. IN Standalone_Mode, ALL Ossiframe dependencies, including selected built-in Modules, SHALL resolve from the Published_Rev without a monorepo checkout.

### Requirement 11: Custom `ports` example module

**User Story:** As a module author, I want a working example using only public APIs, so that I can copy its pattern.

#### Acceptance Criteria

1. THE Ports_Module SHALL declare id `ports`, an empty dependency list, and one operation `listening-tcp`.
2. THE Ports_Module crate SHALL depend only on `ossiframe-core`, `serde` and `async-trait` at runtime.
3. WHEN `listening-tcp` runs, THE Ports_Module SHALL execute `ss -Htln` as a fixed program and argv with a 10 second timeout, without the `-p` flag.
4. THE `listening-tcp` operation SHALL accept `null` or an object `{ "family": "ipv4" | "ipv6" | "all" }` (default `all`). IF args have any other shape, unknown keys or an unknown family value, THEN THE Ports_Module SHALL validate in Rust and return `OperationError::InvalidArguments`. THE family value SHALL select which parsed sockets are returned and SHALL NOT reach argv.
5. THE ss parser SHALL cut input at 64 KiB, return at most 512 entries, limit address fields to 128 characters, strip control characters, skip unparsable lines and count them in `skipped`.
6. THE ss parser SHALL parse IPv4 (`127.0.0.1:22`), IPv6 (`[::]:22`) and wildcard (`*:80`) local addresses.
7. THE Ports_Module SHALL return `{ observedAt, sockets, skipped, truncated }`, where each socket is `{ localAddress, port, process: null }`.
8. THE Ports_Module SHALL provide an optional custom page in `examples/custom-module/ui/` with a family selector and a table of addresses and ports rendered as text using UI_Package components.
9. THE Ports_Module SHALL remain a normal workspace member so it is compiled and tested with the workspace.
10. THE Ports_Module SHALL be defined outside Core, use only the public SDK, be registered explicitly in the app, and not import Doctor internals. Adding it SHALL require no Core change.

### Requirement 12: Quick-start documentation

**User Story:** As a new developer, I want one short guide, so that I can generate and extend an app on my own.

#### Acceptance Criteria

1. THE repository SHALL contain `docs/developer-guide.md` of at most 150 lines.
2. THE developer guide SHALL cover generating an app, pinning Ossiframe or using a local checkout, running `pnpm install` and `pnpm tauri dev`, writing a module, registering it, adding its page, testing with a fake executor, and building.
3. THE `README.md` SHALL link to the developer guide in at most two lines.
4. THE developer guide and `SKILL.md` SHALL state that modules are trusted code and the manifest is not a sandbox.

### Requirement 13: Security invariants preserved

**User Story:** As a security-conscious user, I want S02 to keep S01's guarantees, so that extensibility does not weaken them.

#### Acceptance Criteria

1. THE Registry SHALL dispatch only to compiled-in Modules.
2. THE Modules SHALL depend only on `ossiframe-core` and SHALL NOT depend on `ossiframe-desktop` or `ossiframe-ssh`.
3. THE Modules SHALL send remote commands only through the Core executor using `CommandSpec`.
4. THE app SHALL keep secrets only in the OS keychain and SHALL keep host key pinning behavior unchanged.
5. THE Doctor and THE Generated_App SHALL include only read-only Modules and SHALL expose no mutating or privileged operation.
6. THE new code SHALL NOT log raw SSH output.

### Requirement 14: Quality gates and unit tests

**User Story:** As a maintainer, I want focused tests and standard gates, so that high-risk logic is covered without heavy test infrastructure.

#### Acceptance Criteria

1. THE Core tests SHALL cover duplicate id, missing dependency, cycle, dependency-first deterministic order, and Module_Id rejection of uppercase, empty, `..`, spaces and over-length input.
2. THE Dispatcher tests SHALL cover rejection of a non-UUID server id, bad module or operation ids, oversized args, and an unknown operation before `ServerService` or `Module::run` is reached.
3. THE Generator tests SHALL cover generation with zero modules, automatic inclusion of required dependencies, refusal on a dirty tree without `--rev` or `--ossiframe-local`, hostile name and identifier inputs (`../x`, quotes, `\`, `"; rm`, empty, control characters), refusal of a non-empty directory, rejection of unknown and leftover tokens, equality of the generated CSP with Doctor's CSP, and absence of external `path =` entries without `--ossiframe-local`.
4. THE Ports_Module tests SHALL cover the ss parser on IPv4, IPv6, wildcard, malformed lines and oversized input, args validation (valid and invalid `family`), plus one module test with a fake `Executor`.
5. THE Linux_Inventory tests from S01 SHALL pass unchanged after the move.
6. THE tests SHALL use plain unit tests only, with no property-testing framework and no e2e suite.
7. THE change SHALL pass `cargo fmt --all --check`, `cargo check --workspace --all-targets`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `pnpm -r typecheck`, `pnpm -r lint`, `cargo audit`, `pnpm audit`, production builds of Doctor and of a Generated_App, and `shellcheck` for any added script.
8. THE new dependencies (`serde_json` in Core, optional `tauri` in Desktop_Crate, `clap` in the CLI) SHALL be maintained and permissively licensed.

### Requirement 15: Manual verification on a real server

**User Story:** As a maintainer, I want evidence from a real server, so that S02 claims only what was verified.

#### Acceptance Criteria

1. WHEN S02 verification is performed, THE verification SHALL run only against the Lightsail_Server and SHALL be read-only.
2. THE verification record SHALL be written to `docs/verification/s02-modules-and-generator.md` with real output only.
3. THE verification SHALL record the result of the cross-crate `generate_handler!` spike.
4. THE verification SHALL confirm Doctor connect, host key prompt and overview with real data against the Lightsail_Server.
5. THE verification SHALL generate outside the repository a minimal app (zero modules) and an app with `--modules linux-inventory --with-example-module`, run `pnpm install` and `pnpm tauri build` for each, and confirm that no path dependency leaves the project in Standalone_Mode.
6. THE verification SHALL confirm that the minimal app shows basic connection info, and that the app with modules shows the `linux-inventory` overview and the `ports` page with real listening sockets, compared by the user against `ss -tln` run on the server.
7. THE verification SHALL confirm that Doctor registers only `linux-inventory` and that the Generated_App capability file lists only the eleven Shared_Commands.
8. THE verification of Standalone_Mode SHALL use a Published_Rev, in a directory without the monorepo checkout. Results from `--ossiframe-local` SHALL be recorded as Local_Mode only and SHALL NOT count as Standalone_Mode proof.
9. THE verification record SHALL list every item that was not verified, including the open S01 checks (independent fingerprint comparison through the Lightsail console pending; password authentication unverified and disabled).

### Requirement 16: S02 completion criteria

**User Story:** As a maintainer, I want an explicit completion list, so that S02 is closed only on verified evidence.

#### Acceptance Criteria

1. S02 SHALL be complete only WHEN all of the following are verified and recorded in `docs/verification/s02-modules-and-generator.md`:
   1. Doctor still connects and shows real inventory.
   2. Doctor consumes the `linux-inventory` module through the Registry.
   3. The Registry and Resolver work (unit tests pass, Doctor starts from the Registry).
   4. A minimal app is generated without optional modules.
   5. An app is generated with selected modules.
   6. A generated app builds outside the monorepo.
   7. A generated app connects to the Lightsail_Server.
   8. The Ports_Module retrieves real listening ports.
   9. A custom module is added without changing Core.
   10. Standalone_Mode with a Published_Rev is independently verified.
   11. A concise quick-start guide exists.
   12. Actual build and verification results are recorded.
2. THE S01 open checks SHALL remain documented and SHALL NOT block S02 completion.
