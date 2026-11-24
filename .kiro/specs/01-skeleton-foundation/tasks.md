# Implementation Plan: 01-skeleton-foundation (S01)

## Overview

The design uses Rust (backend, contracts) and TypeScript (frontend), so no language question applies. The plan builds the monorepo skeleton first, then runs a russh spike against a real server so transport problems surface before the SSH crate is built on them. After that it implements Core (pure logic, with property tests), the SSH crate, the desktop crate, the starter backend, the frontend, the quality gates, and finally the manual verification on a disposable Ubuntu 24.04 VPS.

Property tests for Properties 1 to 6 are not marked optional: Requirement 14.3 requires them, and engineering.md names these areas as the high-risk logic that gets automated tests. Properties 7 to 9 and 11 are verified by implementation structure plus manual verification (V2, V8, V11), as the design states. No other test suites are planned.

Out of scope for every task: provisioning, Laravel, module registry or generator, `apps/deploy`, `apps/doctor`, release CI, git commit or push.

## Tasks

- [ ] 1. Create the monorepo skeleton
  - [ ] 1.1 Create the Cargo workspace and the three Rust crates
    - Root `Cargo.toml` with members `crates/*` and `templates/starter/src-tauri`; `rust-toolchain.toml` pinned to stable
    - Create `crates/ossiframe-core`, `crates/ossiframe-ssh`, `crates/ossiframe-desktop` as empty libraries with only the dependencies each needs now
    - `ossiframe-core` must not depend on russh, Tauri, SQLite or keyring; `ossiframe-ssh` and `ossiframe-desktop` depend on core
    - Commit `Cargo.lock` (do not git-ignore it)
    - _Requirements: 1.1, 1.3, 1.4, 1.5_

  - [ ] 1.2 Create the pnpm workspace, frontend packages and starter app shell
    - `pnpm-workspace.yaml` with `packages/*` and `templates/starter`; root `package.json` with `typecheck`, `lint`, `build` scripts
    - Create `packages/ui` (`@ossiframe/ui`) and `packages/client` (`@ossiframe/client`) as TypeScript packages with an empty export
    - Create `templates/starter` (Vite + React + TypeScript + Tailwind + shadcn/ui setup) with a fixed name `ossiframe-starter`, and `src-tauri` as a minimal Tauri 2 app that builds in the Cargo workspace
    - ESLint config including `react/no-danger`; commit `pnpm-lock.yaml`
    - _Requirements: 1.2, 1.3, 1.5, 12.7_

- [ ] 2. russh spike against a real server
  - [ ] 2.1 Write the russh spike harness in `crates/ossiframe-ssh`
    - Add a throwaway, `#[ignore]`d example or test (`examples/russh_spike.rs`) that reads host, port, user, key path and optional passphrase from environment variables
    - Exercise, in order: connect with a `check_server_key` handler that records the key and returns `false` (confirm no authentication is attempted, V2); connect again returning `true`; Ed25519 key auth with and without passphrase (V1); open a channel, `exec` a command, and print stdout, stderr, exit status and the order of the exit-status and close messages (V4); keepalive and inactivity `Config` fields and the keepalive send methods (V7); SHA-256 fingerprint of the presented key compared with `ssh-keygen -lf` output
    - Read the API of the russh version pinned in `Cargo.lock`; fix any signature or behavior the design lists under "Assumptions to Confirm"
    - Never print secrets; read them only from the environment
    - _Requirements: 6.6, 6.7, 7.1, 7.9, 4.8_

  - [ ] 2.2 Run the spike against the disposable VPS and record findings
    - If the user has not named a disposable server and supplied the environment variables, ask for them with the user_input tool and wait; do not use any other server
    - Append the observed russh version, what worked, and any blocking limitation to `docs/verification/s01-skeleton-foundation.md` (create it with a short header if it does not exist). Record only what was actually observed
    - If a limitation blocks a requirement, stop and report it to the user so ADR-0003 and the `ssh2` fallback in the design can be reconsidered before more SSH work is built
    - _Requirements: 14.4_

- [ ] 3. Implement `ossiframe-core`
  - [ ] 3.1 Implement validated identifier types and errors
    - `ident` module: `ServerName`, `Hostname`, `Port` (default 22), `SshUser`, `KeyPath` (`~` expansion, absolute, no NUL, no existence check), `ProgramName`, each with a validating constructor returning `ValidationError`
    - Serde deserialization via `try_from = "String"` (or the integer equivalent) so IPC input passes through the same constructors
    - `error` module with `ValidationError` and `CommandError` (`thiserror`)
    - _Requirements: 2.1, 2.2, 2.3, 2.4, 2.5, 2.6, 2.7, 2.8, 2.9, 2.10, 2.11, 2.12_

  - [ ] 3.2 Write property tests for identifier validation
    - **Property 3: Identifier validation**
    - Include a table of known-bad inputs (`-oProxyCommand=x`, whitespace, `;`, empty, overlong labels); minimum 100 iterations
    - **Validates: Requirements 2.1 to 2.12, 14.3**

  - [ ] 3.3 Implement command specification and quoting
    - `command` module: `Privilege`, `CommandSpec` (`read_only` constructor, `mutating`, optional stdin as a secret byte wrapper), `quote_arg`, `to_command_line` including the modeled `sudo -n` and `sudo -n -u` forms
    - _Requirements: 3.1, 3.2, 3.3, 3.4, 3.5, 3.6, 3.7_

  - [ ] 3.4 Write property tests for quoting
    - **Property 1: Quoting round-trip** (round trip through a real `sh -c` on the dev machine, plus argv recovery for `to_command_line`)
    - **Property 2: Quoting rejects NUL**
    - Minimum 100 iterations each
    - **Validates: Requirements 3.1, 3.2, 3.6, 14.3**

  - [ ] 3.5 Implement the executor contract, read-only policy and operation types
    - `executor` module: `CancelToken`, `OutputStream`, `ExitStatus`, `ExecResult`, `ExecError`, `Executor` trait (`async-trait`), `ExecutionPolicy`, `ReadOnlyUserPolicy`
    - `operation` module: `OperationStatus` and `OperationRecord` (types only)
    - _Requirements: 4.1, 4.2, 4.3, 4.4, 4.5_

  - [ ] 3.6 Write a small test for the read-only policy
    - **Property 10: Read-only**
    - Check that every spec with `mutating = true` or `privilege != User` yields `PolicyRejected`, and a read-only user spec passes
    - **Validates: Requirements 4.3, 4.4, 4.5**

  - [ ] 3.7 Implement redaction
    - `redact` module: `Redactor` (`add_secret`, `redact`, zeroize on drop) and `StreamRedactor` (`push`, `finish`, hold back `max_secret_length - 1`)
    - Code comments state the known limits (exact matches of registered secrets only); secrets under 4 characters are still redacted
    - _Requirements: 5.1, 5.2, 5.3, 5.4, 5.5_

  - [ ] 3.8 Write property tests for redaction
    - **Property 4: Redaction completeness** (random chunk splits; fixed cases for a secret at a chunk boundary and for overlapping secrets; idempotence)
    - Minimum 100 iterations
    - **Validates: Requirements 5.1, 5.2, 5.3, 5.4, 5.5, 14.3**

  - [ ] 3.9 Implement the host key decision
    - `host_key` module: `PresentedHostKey`, `PinnedHostKey`, `HostKeyDecision`, `decide` (constant-time byte compare), `fingerprint_sha256` (`sha2` + base64 without padding; use the method confirmed in task 2)
    - _Requirements: 6.1, 6.2, 6.3, 6.4, 6.5_

  - [ ] 3.10 Write property tests for the host key decision and fingerprint
    - **Property 5: Host key decision** (including same bytes with a different key type)
    - **Property 6: Fingerprint format**
    - Minimum 100 iterations
    - **Validates: Requirements 6.1, 6.2, 6.3, 6.4, 6.5, 14.3**

- [ ] 4. Checkpoint - Ensure all tests pass
  - Run `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test` for core. Ensure all tests pass, ask the user if questions arise.

- [ ] 5. Implement `ossiframe-ssh`
  - [ ] 5.1 Implement `HostKeyHandler` and `SshConnector`
    - `HostKeyHandler` implements the russh handler: convert the presented key to `PresentedHostKey` (reject certificates), call `host_key::decide`, record the decision in shared state, return `true` only for `Trusted`
    - `SshConnector::connect` with `ConnectTarget`, `AuthMethod`, `ConnectOptions` (timeouts and keepalive defaults from the design), mapping failures to `ConnectError` using the recorded decision rather than russh error text
    - Key-file loading with optional passphrase; no authentication call before `Trusted`; no russh type in any public signature
    - Reuse and then remove the spike code from task 2
    - _Requirements: 6.6, 6.7, 6.8, 7.1, 7.2, 7.3, 7.4, 7.5, 7.6, 7.7, 7.8_

  - [ ] 5.2 Implement `SshExecutor` run loop and keepalive
    - Implement `Executor::run`: policy check, `to_command_line`, channel open and exec, stdin or EOF, select over channel messages, deadline and `CancelToken`, `StreamRedactor` per stream, 256 KiB caps with `truncated`, `on_output` callback, end on channel close, `ConnectionLost` when the stream ends without an exit status
    - Background keepalive task: after `keepalive_max_misses` consecutive failures mark the session dead, notify the owner (callback or channel) and fail running commands with `ConnectionLost`
    - Install `ReadOnlyUserPolicy` as the S01 policy; never retry on `Timeout`, `Cancelled` or `ConnectionLost`
    - _Requirements: 4.1, 4.2, 4.6, 4.7, 4.8, 4.9, 4.10, 4.11, 7.9_

  - [ ] 5.3 Add the ignored real-server integration harness for V4 to V7
    - `#[ignore]` test in `crates/ossiframe-ssh/tests/` reading host, user and key path from environment variables; never run by default `cargo test`
    - Cases: stdout and stderr split with real exit code (`ls /nonexistent`, exit 2), `sleep 30` with a 2 s timeout, cancellation during `sleep 30`, and a keepalive loss check that the user triggers by blocking the network
    - Replace the spike example with this harness once it covers the same ground
    - _Requirements: 4.6, 4.7, 4.8, 4.10, 7.9, 14.4_

- [ ] 6. Implement `ossiframe-desktop`
  - [ ] 6.1 Implement the SQLite store
    - `store` module with `rusqlite` (bundled): numbered migrations in a transaction (migration 1 creates `schema_version`, `servers`, `host_keys` exactly as in the design), `open`, `add_server`, `list_servers`, `get_server`, `remove_server`, `get_host_key`, `pin_host_key` (fails if a pin exists), `replace_host_key`
    - No secret column; `secret_ref` is an opaque ID
    - _Requirements: 8.1, 8.3, 8.5, 8.6, 6.12, 6.13_

  - [ ] 6.2 Implement the vault and the secret string type
    - `vault` module over `keyring` (`save`, `load`, `delete`); `SecretString` via `secrecy` + `zeroize` with redacted `Debug` and `Serialize` (check license and maintenance when adding)
    - _Requirements: 9.1, 9.3, 9.4, 9.5_

  - [ ] 6.3 Implement IPC helpers and DTOs
    - `ipc` module: DTOs (camelCase serde) with secrets only in inbound types, `IpcError` and mapping from `ValidationError`, `ConnectError`, `ExecError`, `StoreError`, `VaultError` (stderr excerpt redacted and capped at 2 KiB), `ConnectionState`, `SessionRegistry`, `PendingTrust` (token, server ID, key, created time; 5-minute expiry; single use), replace-token handling for host key mismatch
    - Event helpers for `connection-state` and `connection-lost`
    - _Requirements: 6.7, 6.8, 6.9, 6.10, 6.11, 6.14, 11.3, 11.5, 9.5_

- [ ] 7. Checkpoint - Ensure all tests pass
  - Run the Rust quality gates across the workspace. Ensure all tests pass, ask the user if questions arise.

- [ ] 8. Implement the starter app backend
  - [ ] 8.1 Implement the server info provider
    - `templates/starter/src-tauri/src/server_info/`: probe list (hostname, os-release, uname -r, uname -m, /proc/uptime, nproc, lscpu, /proc/meminfo, `df -P -B1 /`), each a `CommandSpec::read_only` with 10 s timeout; pure parsers that bound input size and return `None` on malformed text; `collect(&dyn Executor, &CancelToken) -> ServerInfo` that records `observed_at` at first probe start, runs probes sequentially and turns failures into `probe_errors`
    - Uses nothing app-specific so it can later move into a module
    - _Requirements: 10.1, 10.2, 10.3, 10.4, 10.5_

  - [ ] 8.2 Write parser tests for the server info provider
    - Small fixture tests for os-release, meminfo, df and lscpu using sample text in the standard Ubuntu 24.04 format (replace with output captured in task 12 when available), plus malformed and empty input returning `None` without panic
    - _Requirements: 10.4_

  - [ ] 8.3 Implement the Tauri commands, state wiring and security configuration
    - `commands.rs`: `add_server` (typed validation, keychain write with row rollback), `list_servers`, `remove_server` (disconnect, delete vault entry, then row), `connect_server` (load pin and secret, connect, return `HostKeyUnknown` with token or `IpcError`), `trust_host_key`, `replace_host_key` (token plus typed host confirmation), `disconnect_server`, `get_server_info`, `cancel_operation`; no command takes a command line, file path or URL
    - `main.rs`: open store in the app data dir, build `Vault` and `SessionRegistry`, register exactly these commands in `generate_handler!` and attempt the app-manifest command allow-list if the Tauri 2 docs support it
    - `tauri.conf.json`: strict CSP without `unsafe-eval`, devtools off in release; `capabilities/default.json` for window `main` with core event defaults only and no `fs`, `shell`, `http`, `opener` or `dialog`
    - Add a build-time check script that fails the production build if the CSP contains `unsafe-eval`; run `shellcheck` on it if it is a shell script
    - _Requirements: 6.7, 6.8, 6.9, 6.10, 6.13, 6.14, 7.8, 8.1, 8.2, 8.3, 8.4, 9.1, 9.2, 9.3, 9.4, 11.1, 11.2, 11.3, 11.5, 13.1, 13.2, 13.3, 13.4, 14.2_

- [ ] 9. Implement the frontend
  - [ ] 9.1 Implement `@ossiframe/client`
    - DTO and `IpcError` types mirroring the Rust types; `api` object with one typed function per IPC command; normalize thrown values into `IpcError`; the only module that imports `invoke`; subscribe helpers for `connection-state` and `connection-lost`
    - _Requirements: 11.1, 11.4, 11.5_

  - [ ] 9.2 Implement `@ossiframe/ui` components used by the starter
    - Only components the starter uses: `StatusBadge`, `ErrorPanel` (message per error kind, stderr excerpt in `<pre>` as text), `KeyValueList`, `Fingerprint`, dialog wrappers
    - _Requirements: 12.1, 12.6, 12.7_

  - [ ] 9.3 Implement the starter pages and dialogs
    - `ServerListPage` (status badge, fingerprint prefix, add and remove), `AddServerDialog` (field-level validation messages, key file or password switch, "Save secret in OS keychain" off by default, secret cleared on submit), `TrustHostKeyDialog` (key type, full fingerprint, Trust and Cancel), host key mismatch panel (both fingerprints, no continue button, "Replace pinned key" with typed host confirmation), `ServerOverviewPage` (all info fields, "unavailable" with probe error on request, observed-at time and age, Refresh and Cancel)
    - Wire the connect flow: `connectServer`, on `hostKeyUnknown` show the dialog, `trustHostKey`, reconnect, then `getServerInfo`
    - Keep the last result in memory only
    - _Requirements: 12.1, 12.2, 12.3, 12.4, 12.5, 12.6, 12.7, 12.8, 9.2, 10.2, 10.5_

- [ ] 10. Checkpoint - Ensure all tests pass
  - Ensure the Rust workspace and frontend packages build and typecheck. Ensure all tests pass, ask the user if questions arise.

- [ ] 11. Run the quality gates
  - [ ] 11.1 Make every required gate pass
    - Run `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo check`, `cargo test`, `pnpm typecheck`, `pnpm lint`, and the production build of the starter app (including the CSP check from task 8.3); fix every failure at its cause
    - Add the root script entries needed to run them in one command if they are missing
    - Run `cargo audit` once locally if available and keep its result for the verification record
    - _Requirements: 1.6, 14.1, 14.2, 13.1_

- [ ] 12. Manual verification on a real disposable VPS
  - [ ] 12.1 Run V1 to V12 and record the evidence
    - Use only the disposable Ubuntu 24.04 VPS the user names; ask with the user_input tool if none is named, and wait. This task needs the user to run the desktop app and perform the manual steps (blocking the network, changing the host key or editing a scratch pin) that an agent cannot do alone
    - Build the checklist in `docs/verification/s01-skeleton-foundation.md` with date, app commit, server image, and a short result per verification point V1 to V12 (V10: compare displayed values with `cat /etc/os-release`, `uname -a`, `free -b`, `df /` typed manually; V11: search the SQLite file, app log and IPC traces for the test password and passphrase)
    - Record only points that were actually run; mark any others "not run". Include the spike findings from task 2.2 and the `cargo audit` result
    - If any secret is found in the V11 search, treat S01 as not done and report it
    - _Requirements: 14.4, 14.5, 6.6, 6.7, 6.8, 7.1, 7.2, 7.7, 7.9, 9.5, 10.2_

  - [ ] 12.2 Update `docs/compatibility.md` with observed versions
    - Add the versions actually observed during verification (Ubuntu image and kernel, OpenSSH server version, russh and Tauri versions from `Cargo.lock`, macOS version, Rust and Node toolchain versions). Do not add values that were not observed
    - _Requirements: 14.4_

## Notes

- Property tests are required (not optional) because Requirement 14.3 mandates them. Each uses at least 100 iterations.
- Task 2 gates the SSH work. If it finds a blocking russh limitation, stop and review ADR-0003 before continuing.
- Tasks 12.1 and 12.2 depend on a real server and manual steps by the user; they must never contain invented results.
- No task commits, pushes or publishes.

## Task Dependency Graph

```json
{
  "waves": [
    { "id": 0, "tasks": ["1.1"] },
    { "id": 1, "tasks": ["1.2", "2.1"] },
    { "id": 2, "tasks": ["2.2"] },
    { "id": 3, "tasks": ["3.1"] },
    { "id": 4, "tasks": ["3.2", "3.3"] },
    { "id": 5, "tasks": ["3.4", "3.5", "3.7", "3.9"] },
    { "id": 6, "tasks": ["3.6", "3.8", "3.10"] },
    { "id": 7, "tasks": ["4"] },
    { "id": 8, "tasks": ["5.1", "6.1", "6.2"] },
    { "id": 9, "tasks": ["5.2", "6.3"] },
    { "id": 10, "tasks": ["5.3", "8.1"] },
    { "id": 11, "tasks": ["7"] },
    { "id": 12, "tasks": ["8.2", "8.3", "9.1", "9.2"] },
    { "id": 13, "tasks": ["9.3"] },
    { "id": 14, "tasks": ["10"] },
    { "id": 15, "tasks": ["11.1"] },
    { "id": 16, "tasks": ["12.1"] },
    { "id": 17, "tasks": ["12.2"] }
  ]
}
```
