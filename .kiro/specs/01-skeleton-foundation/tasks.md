# Implementation Plan: 01-skeleton-foundation (S01)

## Overview

Build Ossiframe Doctor (Tauri 2) bottom-up: workspace, then `ossiframe-core`, `ossiframe-ssh`, `ossiframe-desktop`, the Doctor backend, the frontend packages and UI. Everything up to the quality gate is implemented and tested offline. Live VPS verification comes last and does not block implementation. Languages: Rust (backend) and TypeScript (frontend). Small plain `#[test]` unit tests are part of each implementation task. No property-testing framework, fixture suites or end-to-end suites are used.

## Tasks

- [x] 1. Initialize the monorepo workspace
  - Assume stable Rust (rustup) and pnpm are already installed. Install nothing globally.
  - Create root `Cargo.toml` (workspace members `crates/*`, `apps/doctor/src-tauri`), `rust-toolchain.toml`, `pnpm-workspace.yaml` (`packages/*`, `apps/doctor`), root `package.json` with `typecheck` and `lint` scripts.
  - Scaffold empty crates `ossiframe-core`, `ossiframe-ssh`, `ossiframe-desktop`, packages `@ossiframe/ui` and `@ossiframe/client`, and the `apps/doctor` Tauri 2 + Vite + React + TypeScript + Tailwind app.
  - Add dependencies with `cargo add` and `pnpm add` so the current versions are resolved by the tools. Add each dependency in the task that first uses it. Core must not depend on russh, Tauri, rusqlite or keyring.
  - Commit-ready `Cargo.lock` and `pnpm-lock.yaml` (generated, not hand-edited). Confirm `cargo check` and `pnpm install` succeed.
  - _Requirements: 1.1, 1.2, 1.3, 1.4, 1.5, 1.6_

- [x] 2. Implement `ossiframe-core`
  - [x] 2.1 Validated identifier types and `SecretString`
    - Implement `ServerName`, `Hostname`, `Port`, `SshUser`, `KeyPath` with validating constructors and serde via `try_from`. `KeyPath` expands `~`, rejects NUL and relative paths, and does not check existence.
    - Implement `SecretString` (zeroizing, prints and serializes as `<redacted>`).
    - Unit tests: valid and invalid tables per type (empty, overlong, control characters, `-oProxyCommand=x`, whitespace, `;`, `/`, leading `-`, label length, port 0 and 65536, bad user names, relative and NUL key paths); `SecretString` Debug and serialize output.
    - _Requirements: 2.1, 2.2, 2.3, 2.4, 2.5, 2.6, 2.7, 2.8, 2.9, 2.10, 7.12, 14.3_

  - [x] 2.2 `CommandSpec`, quoting and `Executor` contract
    - Implement `CommandSpec::new` (non-empty program, no NUL, no leading `-` on the program, NUL rejected in args) and `to_command_line` (single-quote every word, `'` encoded as `'\''`).
    - Define `ExecResult`, `ExitStatus`, `ExecError`, `CancelToken` (`tokio-util` `CancellationToken`) and the `#[async_trait] Executor` trait. Non-zero exit is `Ok`.
    - Unit tests: quotes, spaces, `;`, `$()`, backticks, newlines, leading `-` in args, NUL rejection; round trip through local `sh -c` with `printf`; a fake `Executor` returning `Timeout`, `Cancelled` and `ConnectionLost`.
    - _Requirements: 3.1, 3.2, 3.3, 3.4, 3.5, 3.6, 3.7, 3.8, 14.3_

  - [x] 2.3 Bounded capture and redaction
    - Implement a bounded output buffer (256 KiB per stream) and `finalize_output(raw, secrets)`: lossy UTF-8, replace registered secrets with `[REDACTED]`, truncate after redaction on a char boundary, set `truncated`. When the raw cap is hit, drop trailing bytes that form a prefix of a registered secret before redaction.
    - Unit tests: secret replaced; multiple secrets; secret split at the raw cap does not leak; truncation on a multibyte boundary sets the flag; invalid UTF-8.
    - _Requirements: 4.1, 4.2, 4.3, 4.4, 14.3_

  - [x] 2.4 Host key types and `decide`
    - Implement `PresentedHostKey`, `PinnedHostKey`, `HostKeyDecision` and `host_key::decide`.
    - Unit tests: no pin gives `Unknown`; same type and bytes gives `Trusted`; different bytes gives `Mismatch` with both keys; same bytes with a different type gives `Mismatch`.
    - _Requirements: 5.1, 5.2, 5.3, 14.3_

- [x] 3. Implement `ossiframe-ssh`
  - [x] 3.1 `SshConnector` and `HostKeyHandler`
    - Add `russh` with `cargo add`. Read the resolved version's API (authentication, key loading with passphrase, `Config` keepalive and inactivity fields, fingerprint helper) and implement against it. No russh type appears in a public signature.
    - Define `AuthMethod` and `ConnectError`. The handler calls `decide`, records the decision in shared state, and returns `true` only for `Trusted`. `connect` maps the recorded decision to `HostKeyUnknown` or `HostKeyMismatch` without sending a credential, rejects host certificates, applies the connect timeout, and configures keepalive and inactivity timeout. Map errors to `Unreachable`, `ConnectTimeout`, `KeyFileUnreadable` (path only), `PassphraseRequired`, `PasswordRequired` and `AuthFailed`.
    - Unit tests (offline): handler returns `false` for `Unknown` and `Mismatch` and records the decision; decision-to-error mapping; unreadable key file error has no file content; `AuthFailed` message does not distinguish user from credential.
    - _Requirements: 5.4, 5.5, 5.6, 5.17, 6.1, 6.2, 6.3, 6.4, 6.5, 9.2, 14.3_

  - [x] 3.2 `SshExecutor`
    - Implement `Executor` for an authenticated session: open a channel, `exec` the quoted line, send EOF, collect stdout and stderr into bounded buffers, record exit status, finish on channel close, enforce timeout and cancel token (close the channel, return `Timeout` or `Cancelled`), return `ConnectionLost` if the connection ends without an exit status. Finalize output with the registered secrets from task 2.3. Expose an `is_open` check for the registry.
    - Unit tests that do not need a server: the select/finalize logic factored into pure helpers (timeout versus cancel versus close ordering, exit-status handling, output finalization wiring).
    - _Requirements: 3.4, 3.5, 3.6, 3.7, 3.8, 4.1, 4.2, 4.3, 9.4, 14.3_

  - [x] 3.3 Ignored live integration test
    - Add an `#[ignore]` test in `ossiframe-ssh` reading host, user and key path from environment variables. It covers exec output and exit code, timeout, cancel and disconnect detection. It must compile offline and never run in the quality gate.
    - _Requirements: 14.4_

- [x] 4. Implement `ossiframe-desktop`
  - [x] 4.1 SQLite store
    - Add `rusqlite` (bundled). Implement numbered migrations in a transaction, `servers` and `host_keys` tables with no secret column (`secret_ref` is an opaque id), CRUD, `pin_host_key` (errors if a pin exists), `replace_host_key`, `clear_pin`. Access is behind a mutex.
    - Unit tests (in-memory DB): migration applies and is idempotent; schema has no secret-bearing column; `pin_host_key` refuses to overwrite and keeps the old pin; replace works; deleting a server cascades the pin.
    - _Requirements: 5.9, 5.13, 7.10, 7.13, 8.1, 8.10, 14.3_

  - [x] 4.2 Keychain vault
    - Add `keyring`. Implement save, load and delete by opaque entry id behind a small trait so tests can use an in-memory fake. Map locked, denied or unavailable to a `keychain` error with no plaintext fallback.
    - Unit tests with the fake: round trip; failure maps to `keychain` and nothing is stored elsewhere.
    - _Requirements: 7.1, 7.6, 7.8, 7.10, 14.3_

  - [x] 4.3 Trust tokens and session registry
    - Implement `TrustToken` store (backend-created, 5 minute expiry, single-use, bound to server ID, host, port and observed key; separate replace token kind). Inject a clock for testing. Implement invalidation by profile.
    - Implement the session registry: map of server ID to `{executor, state}`, per-profile async mutex serializing connect, disconnect, update, remove and inventory, handle-open check before use, removal on lost connection, no entry after a failed or cancelled connect, per-profile cancel tokens, and `connection-state` and `connection-lost` notifications through a callback trait (Tauri-free).
    - Unit tests: token expiry, single use, wrong-server rejection, unknown token, invalidation on host/port edit and pin replace; trusting pins the token's key, not a caller-supplied one; failed connect leaves no entry; guard serializes two concurrent calls; closed handle removes the entry.
    - _Requirements: 5.8, 5.9, 5.10, 5.11, 5.12, 6.6, 6.7, 9.1, 9.3, 9.4, 9.5, 9.6, 11.4, 14.3_

  - [x] 4.4 Profile operations and credential flow
    - Implement add, list, update and remove orchestration over store, vault and registry. Save a secret only after authentication succeeds; roll back the row if the vault write fails on add; replace a saved secret only after the new credential authenticates; delete obsolete secrets on auth kind, key path or credential edits; clear the pin and disconnect on host or port edit; keep pin and session on name-only edits; remove closes the session, invalidates tokens, deletes the vault entry, then the row. `secret_ref` is never left dangling. Ephemeral credentials are held in zeroizing wrappers for one attempt only. Compute `credentialRequired` and `passphraseRequired` outcomes.
    - Unit tests with fakes: add rollback on vault failure; edit matrix (name only, host/port, auth change); remove order and local-only effect; update failure leaves `secret_ref` valid or absent; no secret in any outbound struct (by type).
    - _Requirements: 5.11, 5.12, 7.1, 7.3, 7.4, 7.6, 7.7, 7.8, 7.9, 7.10, 8.1, 8.2, 8.3, 8.4, 8.5, 8.6, 8.7, 8.8, 8.9, 14.3_

- [x] 5. Implement the Doctor backend
  - [x] 5.1 Inventory provider
    - In `apps/doctor/src-tauri/src/inventory/`, implement predefined probes (`hostname`, `cat /etc/os-release`, `uname -r`, `uname -m`, `cat /proc/uptime`, `nproc`, `lscpu`, `cat /proc/meminfo`, `df -P -B1 /`) with 10 second timeouts, run sequentially through `&dyn Executor`, with pure bounded parsers that return `None` for unrecognized lines. A failed, timed-out or unparsable probe sets its fields to `None`, adds a sanitized `probe_errors` entry and continues. Results are in memory only; no CPU utilization.
    - Unit tests: each parser on a sample string, on garbage, and on oversized input without panic; provider with a fake executor covering a failing probe and a timeout.
    - _Requirements: 9.7, 10.1, 10.2, 10.3, 10.4, 10.5, 10.6, 14.3_

  - [x] 5.2 DTOs and `IpcError`
    - Define `ServerDto`, `NewServerDto`, `UpdateServerDto`, `ServerInfoDto`, connect outcomes (`Connected`, `HostKeyUnknown`), and the tagged `IpcError` with all kinds in the design. Inbound DTOs construct validated types via their constructors and return `validation` with the field name on failure. Messages are sanitized and excerpts are capped at 2 KiB; `internal` carries no detail. Map `ConnectError`, `ExecError`, store and vault errors to `IpcError`.
    - Unit tests: invalid input produces `validation` with the field; each source error maps to the expected kind; `internal` has no detail; excerpt cap; no secret fields on outbound DTOs.
    - _Requirements: 2.11, 4.6, 5.7, 5.14, 11.5, 11.6, 14.3_

  - [x] 5.3 Tauri commands and wiring
    - Implement `list_servers`, `add_server`, `update_server`, `remove_server`, `connect_server`, `disconnect_server`, `trust_host_key`, `replace_host_key` (requires `confirm_host` equal to the profile host), `get_server_info` and `cancel_operation`. Register exactly these in `generate_handler!`. Wire the store, vault, registry and event emission in app setup. Never log raw SSH output. Accept no executable, command line, readable path or URL from the frontend. After a `Timeout`, `Cancelled` or `ConnectionLost`, report that the remote command may still run and do not retry.
    - Unit tests: unknown server ID and a token for another server are rejected; wrong `confirm_host` keeps the pin; unknown outcome returns a token, key type and full fingerprint; mismatch returns both fingerprints and a replace token without sending credentials.
    - _Requirements: 3.9, 4.5, 5.7, 5.9, 5.10, 5.14, 5.15, 5.16, 9.5, 9.6, 11.1, 11.2, 11.3, 11.4, 14.3_

  - [x] 5.4 Tauri security configuration
    - Check the installed Tauri 2 schema, then set the production CSP (`default-src 'none'`, scripts and styles from `'self'`, no `unsafe-eval`, no remote origin, IPC allowance as the installed version requires). Capabilities apply to window `main` only with core event defaults and no `fs`, `shell`, `http`, `opener` or `dialog` plugin. Disable devtools in release. Use app manifest command permissions if the installed version supports them.
    - _Requirements: 13.1, 13.2, 13.3, 13.4, 13.5_

- [x] 6. Implement the frontend
  - [x] 6.1 `@ossiframe/client`
    - Hand-written DTO and `IpcError` types and one typed async wrapper per command. Normalize thrown values to `IpcError`. This is the only code that calls `invoke`. Add event subscription helpers for `connection-state` and `connection-lost`.
    - _Requirements: 11.7, 11.8_

  - [x] 6.2 `@ossiframe/ui`
    - Add shadcn/ui-based components Doctor uses and other apps would reuse (button, input, dialog, badge, field with error text). No themes, animations or placeholder navigation.
    - _Requirements: 1.3, 1.6_

  - [x] 6.3 Doctor pages and dialogs
    - Server list (state badge, fingerprint prefix, connect, edit, remove), add/edit dialog (field-level validation errors, "Save secret in OS keychain" off by default, pin-clear warning on host/port edit, typed key path), authentication prompt (clears fields on submit; retries `connect_server` with the dialog-held credential after trust), host fingerprint dialog (key type, full fingerprint in monospace, TOFU wording "Verify this fingerprint", Trust and Cancel), changed host key warning (both fingerprints, no continue, "Replace pinned key" with typed host), connected overview (all inventory fields, observation timestamp, "unavailable" with probe error, stale marking when disconnected, Refresh, loading and error states), and plain-text error messages per `IpcError` kind including the "remote command may still be running" notice. Render remote data as text nodes only.
    - _Requirements: 3.9, 5.18, 7.2, 7.3, 7.4, 7.5, 7.7, 7.11, 12.1, 12.2, 12.3, 12.4, 12.5, 12.6, 12.7, 12.8, 12.9, 12.10, 12.11_

  - [x] 6.4 ESLint and TypeScript configuration
    - Configure ESLint with `eslint-plugin-react` and `react/no-danger` as an error, and strict TypeScript. Ensure `pnpm typecheck` and `pnpm lint` run across the workspace.
    - _Requirements: 12.12, 14.1_

- [x] 7. Consolidated quality gate
  - Run `cargo fmt --check`, `cargo check`, `cargo clippy -- -D warnings`, `cargo test` (ignored live tests excluded), `pnpm typecheck`, `pnpm lint` and a production build of Doctor. Run `cargo audit` if available and `shellcheck` on any script added. Fix failures. Confirm lockfiles are present.
  - _Requirements: 1.5, 14.1, 14.2, 14.3_

- [ ] 8. Live VPS verification (pending: requires the user's disposable Ubuntu 24.04 VPS)
  - Do not start without a VPS the user names and authorizes. If none is available, leave this task unchecked and mark it pending; do not fabricate results.
  - Run from macOS: Ed25519 key with and without passphrase, wrong passphrase, password auth, stdout/stderr/exit code capture, timeout, cancel, disconnect detection, changed host key, keychain behavior on an unsigned build, inventory comparison with `cat /etc/os-release`, `uname -a`, `free -b` and `df /`, unreachable host and wrong port. Run the ignored `ossiframe-ssh` integration test with environment variables.
  - Search the SQLite file, logs and IPC traces for the test password and passphrase. A hit means S01 is not done.
  - _Requirements: 14.4, 14.6, 14.7_

- [ ] 9. Record verification evidence
  - Write `docs/verification/s01-skeleton-foundation.md` with date, commit, server image, and only the checks actually run. If task 8 did not run, state that live verification is pending. Note any russh failure and that ADR-0003 needs revisiting.
  - Update `docs/compatibility.md` with observed versions only (Rust, Tauri, russh, rusqlite, keyring, server OS from the run). Leave unobserved entries unchanged.
  - _Requirements: 14.5, 14.7_

## Notes

- Tests are embedded in the implementation tasks and are not optional. The design has no Correctness Properties section, so no property tests exist.
- Tasks 1 to 7 need no VPS. Task 8 is the only task that needs live infrastructure and may remain pending.
- Out of scope: provisioning, sudo, modules and registry, generator, auditing, Deploy, SFTP, terminal, SSH agent, release CI.

## Task Dependency Graph

```json
{
  "waves": [
    { "id": 0, "tasks": ["1"] },
    { "id": 1, "tasks": ["2.1"] },
    { "id": 2, "tasks": ["2.2"] },
    { "id": 3, "tasks": ["2.3"] },
    { "id": 4, "tasks": ["2.4"] },
    { "id": 5, "tasks": ["3.1"] },
    { "id": 6, "tasks": ["3.2"] },
    { "id": 7, "tasks": ["3.3", "4.1"] },
    { "id": 8, "tasks": ["4.2"] },
    { "id": 9, "tasks": ["4.3"] },
    { "id": 10, "tasks": ["4.4"] },
    { "id": 11, "tasks": ["5.1", "6.1", "6.2"] },
    { "id": 12, "tasks": ["5.2"] },
    { "id": 13, "tasks": ["5.3"] },
    { "id": 14, "tasks": ["5.4", "6.3"] },
    { "id": 15, "tasks": ["6.4"] },
    { "id": 16, "tasks": ["7"] },
    { "id": 17, "tasks": ["8"] },
    { "id": 18, "tasks": ["9"] }
  ]
}
```
