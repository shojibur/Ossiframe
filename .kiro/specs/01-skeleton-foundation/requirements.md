# Requirements Document

## Introduction

S01 (01-skeleton-foundation) delivers Ossiframe Doctor, a Tauri 2 desktop app in `apps/doctor/`. Doctor manages server profiles, connects to an authorized Ubuntu 24.04 VPS over SSH with a pinned host key, and shows read-only inventory with an observation timestamp. Reusable SSH, storage, credential, execution and UI code lives in shared crates and packages. This document derives requirements from `design.md` for S01 only. The starter template, generator, and auditing belong to later specs.

## Glossary

- **Doctor**: The Ossiframe Doctor Tauri 2 desktop app in `apps/doctor/`, including its React frontend and Tauri backend.
- **Core**: The `ossiframe-core` crate (validated types, `CommandSpec`, quoting, `Executor` trait, redaction, `host_key::decide`).
- **SSH_Crate**: The `ossiframe-ssh` crate, providing `SshConnector` and `SshExecutor`.
- **Desktop_Crate**: The `ossiframe-desktop` crate, providing Store, Vault, and the Session_Registry.
- **Store**: The SQLite-backed component in Desktop_Crate that persists server profiles and pinned host keys.
- **Vault**: The OS-keychain-backed component in Desktop_Crate that persists opted-in secrets.
- **Executor**: An implementation of the Core `Executor` trait that runs a `CommandSpec` and returns an `ExecResult` or `ExecError`.
- **SshExecutor**: The SSH_Crate Executor that runs commands over an authenticated SSH session.
- **SshConnector**: The SSH_Crate component that connects, verifies the host key, and authenticates.
- **HostKeyHandler**: The SSH_Crate component implementing the russh `check_server_key` callback using Core's `host_key::decide`.
- **Inventory_Provider**: The `apps/doctor/src-tauri/src/inventory/` module that runs predefined probes through an Executor and parses the output into `ServerInfoDto`.
- **Probe**: One predefined inventory command with trusted arguments and a 10 second timeout.
- **Session_Registry**: The in-memory map from server ID to connection state and SshExecutor, with a per-profile async guard.
- **Trust_Token**: A backend-created, short-lived, single-use token that authorizes pinning or replacing one specific host key.
- **IPC_Layer**: The `#[tauri::command]` functions of Doctor's backend and their typed DTOs and `IpcError`.
- **Client_Package**: The `@ossiframe/client` package with one typed wrapper per IPC command.
- **UI_Package**: The `@ossiframe/ui` package of shared UI components.
- **Ephemeral_Credential**: A password or passphrase supplied for one connection attempt and held only in zeroizing memory.
- **Verification_Record**: The file `docs/verification/s01-skeleton-foundation.md` recording the manual run.

## Requirements

### Requirement 1: Monorepo skeleton

**User Story:** As a developer, I want a buildable monorepo with the S01 crates, packages and app, so that reusable code is separate from Doctor.

#### Acceptance Criteria

1. THE repository SHALL contain a Cargo workspace including `crates/*` and `apps/doctor/src-tauri`.
2. THE repository SHALL contain a pnpm workspace including `packages/*` and `apps/doctor`.
3. THE repository SHALL contain the crates `ossiframe-core`, `ossiframe-ssh` and `ossiframe-desktop`, and the packages `@ossiframe/ui` and `@ossiframe/client`.
4. THE Core crate SHALL declare no dependency on russh, Tauri, SQLite or a keychain library.
5. THE repository SHALL commit `Cargo.lock` and `pnpm-lock.yaml`.
6. THE Doctor app SHALL contain only app composition, IPC commands, the inventory module and UI pages, with reusable SSH, storage, credential, execution and UI code in the crates and packages.

### Requirement 2: Validated input types

**User Story:** As a developer, I want connection fields validated into typed values, so that malformed or malicious input cannot reach the executor.

#### Acceptance Criteria

1. WHEN a `ServerName` is constructed from 1 to 64 characters after trim with no control characters, THE Core SHALL return a valid `ServerName`.
2. IF a `ServerName` is empty, longer than 64 characters after trim, or contains a control character, THEN THE Core SHALL return a validation error.
3. WHEN a `Hostname` is constructed from an IP literal or a DNS name of at most 253 characters with labels of 1 to 63 characters from `[A-Za-z0-9-]` and no leading or trailing `-`, THE Core SHALL return a valid `Hostname`.
4. IF a `Hostname` contains whitespace or `/`, begins with `-`, or violates the length or label rules, THEN THE Core SHALL return a validation error.
5. WHEN a `Port` is constructed from an integer from 1 to 65535, THE Core SHALL return a valid `Port`.
6. IF a `Port` is outside 1 to 65535, THEN THE Core SHALL return a validation error.
7. WHEN an `SshUser` matches `^[a-z_][a-z0-9_-]{0,31}$`, THE Core SHALL return a valid `SshUser`.
8. IF an `SshUser` does not match `^[a-z_][a-z0-9_-]{0,31}$`, THEN THE Core SHALL return a validation error.
9. WHEN a `KeyPath` is non-empty, contains no NUL, and is absolute after `~` expansion, THE Core SHALL return a valid `KeyPath` without checking file existence.
10. IF a `KeyPath` is empty, contains NUL, or is not absolute after `~` expansion, THEN THE Core SHALL return a validation error.
11. WHEN IPC input contains a field of a validated type, THE IPC_Layer SHALL construct the value through its validating constructor and return `IpcError` kind `validation` on failure.

### Requirement 3: Command construction and executor contract

**User Story:** As a developer, I want every remote command built from a quoted structured specification, so that no argument is interpreted as shell syntax.

#### Acceptance Criteria

1. WHEN a `CommandSpec` is created with a program that is non-empty, contains no NUL and does not start with `-`, THE Core SHALL accept it.
2. IF a `CommandSpec` program or any argument contains a NUL byte, or the program is empty or starts with `-`, THEN THE Core SHALL return a validation error.
3. WHEN `to_command_line` is called, THE Core SHALL wrap every word, including the program, in single quotes with each `'` encoded as `'\''`.
4. WHEN an Executor's `run` is called, THE Executor SHALL return an `ExecResult` with exit status, sanitized stdout, sanitized stderr and a truncation flag, or an `ExecError`.
5. THE Executor SHALL treat a non-zero exit code as `Ok(ExecResult)`.
6. IF a running command exceeds its `CommandSpec` timeout, THEN THE SshExecutor SHALL close the channel and return `ExecError::Timeout`.
7. IF the cancel token of a running command is cancelled, THEN THE SshExecutor SHALL close the channel and return `ExecError::Cancelled`.
8. IF the connection ends before an exit status is observed, THEN THE SshExecutor SHALL return `ExecError::ConnectionLost`.
9. IF a command ends with `Timeout`, `Cancelled` or `ConnectionLost`, THEN Doctor SHALL report that the remote command may still be running and SHALL NOT retry the command automatically.

### Requirement 4: Output bounding and redaction

**User Story:** As a user, I want captured output bounded and scrubbed of secrets, so that secrets and oversized output never reach logs or the UI.

#### Acceptance Criteria

1. THE SshExecutor SHALL capture stdout and stderr into separate buffers capped at 256 KiB each.
2. WHEN the channel closes, THE SshExecutor SHALL convert each buffer to lossy UTF-8 and replace every exact occurrence of each registered secret with `[REDACTED]`.
3. WHEN sanitized output exceeds 256 KiB, THE SshExecutor SHALL truncate it after redaction on a character boundary and set `truncated` to true.
4. IF the raw buffer cap is reached while reading, THEN THE SshExecutor SHALL drop trailing bytes that form a prefix of a registered secret before redaction.
5. THE Doctor backend SHALL NOT write raw SSH output to logs.
6. THE IPC_Layer SHALL return parsed inventory fields and sanitized error excerpts of at most 2 KiB, and SHALL NOT return arbitrary command output.

### Requirement 5: Host key verification and trust tokens

**User Story:** As a user, I want each server's host key verified and pinned, so that a substituted server cannot capture my credentials.

#### Acceptance Criteria

1. WHEN `host_key::decide` is called with no pin, THE Core SHALL return `Unknown` carrying the presented key.
2. WHEN `host_key::decide` is called with a pin whose key type and key bytes equal the presented key, THE Core SHALL return `Trusted`.
3. IF `host_key::decide` is called with a pin whose key type or key bytes differ from the presented key, THEN THE Core SHALL return `Mismatch` carrying both keys.
4. THE HostKeyHandler SHALL return `true` to russh only when the decision is `Trusted`.
5. IF the host key decision is `Unknown` or `Mismatch`, THEN THE SshConnector SHALL abort the handshake without sending any credential.
6. IF the server presents a host certificate, THEN THE SshConnector SHALL reject the connection.
7. WHEN the decision is `Unknown`, THE IPC_Layer SHALL return a `HostKeyUnknown` outcome with a Trust_Token, the key type and the full `SHA256:` fingerprint.
8. THE Trust_Token SHALL be created by the backend, expire 5 minutes after creation, be usable once, and be bound to the server ID, host, port and observed key.
9. WHEN `trust_host_key` is called with a valid Trust_Token for that server, THE Store SHALL pin the key recorded in the token and not a key supplied by the frontend.
10. IF `trust_host_key` or `replace_host_key` is called with an unknown, expired, used or other-server token, THEN THE IPC_Layer SHALL return an error and leave pins unchanged.
11. WHEN the host or port of a profile is edited, THE Desktop_Crate SHALL invalidate that profile's pending Trust_Tokens.
12. WHEN a pin is replaced, THE Desktop_Crate SHALL invalidate that profile's sessions and pending Trust_Tokens.
13. IF `pin_host_key` is called for a server that already has a pin, THEN THE Store SHALL return an error and keep the existing pin.
14. WHEN the decision is `Mismatch`, THE IPC_Layer SHALL return `hostKeyMismatch` carrying the pinned and presented fingerprints and a replace token.
15. WHEN `replace_host_key` is called with a valid replace token and `confirm_host` equal to the profile host, THE Store SHALL replace the pin.
16. IF `confirm_host` does not equal the profile host, THEN THE IPC_Layer SHALL return an error and keep the existing pin.
17. WHEN a connection is made, THE SshConnector SHALL verify the presented host key against the pin.
18. THE Doctor UI SHALL describe first-time acceptance as trust on first use, ask the user to verify the fingerprint with a trusted source, and not state that the identity is verified.

### Requirement 6: SSH connection

**User Story:** As a user, I want to connect with a key file or password, so that Doctor can read server inventory.

#### Acceptance Criteria

1. WHEN the host key decision is `Trusted`, THE SshConnector SHALL authenticate with the profile's key file or password.
2. IF the TCP connection cannot be established, THEN THE SshConnector SHALL return `Unreachable`.
3. IF the connection and key exchange do not complete within the connect timeout, THEN THE SshConnector SHALL return `ConnectTimeout`.
4. IF the key file is missing or unreadable, THEN THE SshConnector SHALL return `KeyFileUnreadable` with the path and without file content.
5. IF the server rejects the credential, THEN THE SshConnector SHALL close the connection and return `AuthFailed` without indicating which part was wrong.
6. WHEN authentication succeeds, THE Session_Registry SHALL record the profile as connected with its SshExecutor.
7. IF a connect attempt fails or is cancelled, THEN THE Session_Registry SHALL hold no entry for that profile.

### Requirement 7: Credential handling

**User Story:** As a user, I want control over whether credentials are saved, so that secrets are stored only when I choose.

#### Acceptance Criteria

1. WHERE the user selects "Save secret in OS keychain", THE Vault SHALL store the secret under an opaque entry ID after the connection authenticates successfully.
2. THE Doctor UI SHALL default "Save secret in OS keychain" to off.
3. WHEN a password profile has no saved secret, THE IPC_Layer SHALL return `credentialRequired` from `connect_server` and THE Doctor UI SHALL show an authentication prompt.
4. IF the key file is encrypted and no passphrase is available, THEN THE IPC_Layer SHALL return `passphraseRequired` and THE Doctor UI SHALL show a passphrase prompt.
5. WHEN a host key is trusted after a `HostKeyUnknown` outcome, THE Doctor UI SHALL retry `connect_server` with the Ephemeral_Credential still held in the dialog.
6. WHEN a profile with a saved secret is connected after an application restart, THE Desktop_Crate SHALL load the secret from the Vault.
7. WHEN authentication fails, THE Doctor UI SHALL offer credential re-entry, and THE Desktop_Crate SHALL replace a saved secret only after the new credential authenticates.
8. IF the keychain is locked, denied or unavailable, THEN THE Desktop_Crate SHALL return a `keychain` error and SHALL NOT store the secret in plaintext or treat it as unsaved.
9. THE Desktop_Crate SHALL hold an Ephemeral_Credential only in zeroizing memory for one connection attempt and then drop it.
10. THE Doctor backend SHALL NOT write a secret to SQLite, localStorage, configuration, logs, event payloads or IPC responses.
11. THE Doctor UI SHALL clear credential fields on submit.
12. THE `SecretString` type SHALL print and serialize as `<redacted>`.
13. THE Store schema SHALL contain no column holding a secret value, and `secret_ref` SHALL hold only an opaque identifier.

### Requirement 8: Profile management

**User Story:** As a user, I want to add, list, edit, remove, connect, disconnect and refresh server profiles, so that I can manage my servers locally.

#### Acceptance Criteria

1. WHEN `add_server` is called with valid input, THE Store SHALL create a profile with a generated ID and return a `ServerDto`.
2. IF the Vault write fails during `add_server`, THEN THE Desktop_Crate SHALL roll back the profile row and return a `keychain` error.
3. WHEN `list_servers` is called, THE IPC_Layer SHALL return each profile with a `has_saved_secret` flag, its pinned fingerprint and its connection state, and no secret value.
4. WHEN only the name of a profile is edited, THE Desktop_Crate SHALL keep the pin and the session.
5. WHEN the host or port of a profile is edited, THE Desktop_Crate SHALL disconnect the session, clear the pin and require the new endpoint to be verified again.
6. WHEN the auth kind, key path or credentials of a profile are edited, THE Desktop_Crate SHALL delete obsolete saved secrets, disconnect any active session and not reuse old secrets.
7. IF an update fails, THEN THE Desktop_Crate SHALL leave `secret_ref` either valid or absent.
8. WHEN `remove_server` is called, THE Desktop_Crate SHALL close the session, invalidate Trust_Tokens, delete the Vault entry, then delete the profile row and pin.
9. THE `remove_server` command SHALL change only local state and SHALL NOT contact or modify the remote server.
10. THE Store SHALL apply schema migrations as a numbered ordered list inside a transaction.

### Requirement 9: Connection management

**User Story:** As a user, I want connection state to be accurate, so that I never act on a stale session.

#### Acceptance Criteria

1. THE Session_Registry SHALL serialize connect, disconnect, update, remove and inventory calls for one profile with a per-profile async guard.
2. THE SshConnector SHALL configure russh keepalive and inactivity timeout settings to detect dead connections.
3. IF the connection is lost, THEN THE Session_Registry SHALL remove the entry, mark the profile disconnected and emit a `connection-lost` event.
4. WHEN a session is used, THE Session_Registry SHALL check that the russh handle is open, and remove the entry if the handle is closed.
5. WHEN a connection state changes, THE IPC_Layer SHALL emit a `connection-state` event carrying the server ID.
6. WHEN `cancel_operation` is called for a profile, THE IPC_Layer SHALL cancel its running commands.
7. THE Inventory_Provider SHALL bound each Probe to 10 seconds.

### Requirement 10: Read-only inventory

**User Story:** As a user, I want to see real server information, so that I can confirm what I am managing.

#### Acceptance Criteria

1. WHEN `get_server_info` is called for a connected profile, THE Inventory_Provider SHALL run the probes for hostname, OS release, kernel, architecture, uptime, CPU count, CPU model, memory and root filesystem sequentially, using predefined commands with trusted arguments.
2. THE Inventory_Provider SHALL collect no CPU utilization and run no command that modifies the server.
3. WHEN the probes finish, THE Inventory_Provider SHALL return a `ServerInfoDto` with `observed_at` and the parsed fields.
4. IF a Probe fails, times out or returns unparsable output, THEN THE Inventory_Provider SHALL set its fields to `None` and add a `probe_errors` entry with probe name and a sanitized message, and SHALL continue the remaining probes.
5. THE Inventory_Provider parsers SHALL bound their input size and return `None` for unrecognized lines without panicking.
6. THE Doctor backend SHALL hold inventory results in memory only.

### Requirement 11: Typed IPC and error mapping

**User Story:** As a developer, I want the frontend limited to typed inventory operations, so that it cannot run arbitrary commands.

#### Acceptance Criteria

1. THE IPC_Layer SHALL register only `list_servers`, `add_server`, `update_server`, `remove_server`, `connect_server`, `disconnect_server`, `trust_host_key`, `replace_host_key`, `get_server_info` and `cancel_operation`.
2. THE IPC_Layer SHALL expose only typed profile, connection and inventory operations whose remote commands are predefined in the backend.
3. THE IPC_Layer SHALL accept no executable, shell command line, file path to read, URL or privileged operation from the frontend.
4. IF a command receives an unknown server ID or a token for another server, THEN THE IPC_Layer SHALL return an error.
5. WHEN a command fails, THE IPC_Layer SHALL return an `IpcError` of kind `validation`, `notFound`, `unreachable`, `connectTimeout`, `hostKeyMismatch`, `authFailed`, `credentialRequired`, `passphraseRequired`, `notConnected`, `commandTimeout`, `cancelled`, `connectionLost`, `keychain`, `storage` or `internal` with a sanitized message.
6. THE `internal` error SHALL carry no detail.
7. THE Client_Package SHALL expose one typed async function per command, hand-written DTO types, and SHALL normalize thrown values to `IpcError`.
8. THE Client_Package SHALL be the only frontend code that calls `invoke`.

### Requirement 12: Minimal UI

**User Story:** As a user, I want a minimal interface for profiles and inventory, so that I can use Doctor without a terminal.

#### Acceptance Criteria

1. THE Doctor UI SHALL provide a server list, an add/edit dialog, an authentication prompt, a host fingerprint dialog, a changed host key warning, a connected overview and error states.
2. WHEN the server list is displayed, THE Doctor UI SHALL show each profile with a state badge (disconnected, connecting, connected, error) and the prefix of its pinned fingerprint.
3. WHEN `validation` errors are returned, THE Doctor UI SHALL show each message next to its field.
4. WHEN the host or port of a profile is being edited, THE Doctor UI SHALL warn that the pin will be cleared.
5. WHEN a `HostKeyUnknown` outcome is returned, THE Doctor UI SHALL show the key type, the full SHA-256 fingerprint in monospace, comparison guidance, and Trust and Cancel actions.
6. WHEN `hostKeyMismatch` is returned, THE Doctor UI SHALL show both fingerprints, offer no continue action, and offer "Replace pinned key" requiring the typed host name.
7. WHEN a `ServerInfoDto` is displayed, THE Doctor UI SHALL show hostname, OS, kernel, architecture, uptime, CPU count and model, total and available memory, root filesystem capacity and usage, and the observation timestamp.
8. WHEN a field is `None`, THE Doctor UI SHALL show "unavailable" for it with its probe error available.
9. WHEN the profile is disconnected, THE Doctor UI SHALL mark the displayed information as stale.
10. THE Doctor UI SHALL provide a Refresh action and loading, connected, disconnected and error states.
11. WHEN an `IpcError` is returned, THE Doctor UI SHALL show a plain-text message for its kind.
12. THE Doctor UI SHALL render remote data as text nodes, and ESLint SHALL ban `dangerouslySetInnerHTML` through `react/no-danger`.

### Requirement 13: Tauri security configuration

**User Story:** As a user, I want a restrictive app configuration, so that a compromised webview cannot reach the filesystem, network or shell.

#### Acceptance Criteria

1. THE Doctor production CSP SHALL use `default-src 'none'`, allow scripts and styles from `'self'`, include no `unsafe-eval` and allow no remote origin.
2. THE Doctor capabilities SHALL apply to the `main` window only, with core event defaults and no `fs`, `shell`, `http`, `opener` or `dialog` plugin.
3. THE Doctor SHALL accept the key path as a typed text field.
4. THE Doctor SHALL disable devtools in release builds.
5. WHERE the installed Tauri 2 version supports command permissions in the app manifest, THE Doctor SHALL use them.

### Requirement 14: Quality gates and verification

**User Story:** As a developer, I want automated checks and a documented manual run, so that S01 is verified on a real server.

#### Acceptance Criteria

1. THE repository SHALL pass `cargo fmt --check`, `cargo check`, `cargo clippy -- -D warnings`, `pnpm typecheck`, `pnpm lint` and a production build of Doctor before S01 is complete.
2. WHERE `cargo audit` is available, THE repository SHALL run it, and WHERE a script is added, THE repository SHALL run `shellcheck` on it.
3. THE repository SHALL include focused plain unit tests, with no property-testing framework, for command quoting, malicious input rejection, host key decision, changed-key rejection, credential handling, secret redaction, unauthorized IPC access and execution failure handling.
4. WHEN manual verification is performed, THE developer SHALL run Ed25519 key authentication with and without passphrase, wrong passphrase, password authentication, stdout/stderr/exit code capture, timeout, cancel, disconnect detection, changed host key, keychain behavior on an unsigned build, inventory comparison with `cat /etc/os-release`, `uname -a`, `free -b` and `df /`, and unreachable host and wrong port, on a disposable Ubuntu 24.04 VPS from macOS.
5. THE developer SHALL record in the Verification_Record the date, commit and server image, and only checks that were actually run.
6. IF a search of the SQLite file, logs and IPC traces finds the test password or passphrase, THEN S01 SHALL be treated as not done.
7. IF russh fails a requirement, THEN THE developer SHALL record it in the Verification_Record and revisit ADR-0003.

## Non-Goals

- Provisioning and any server mutation.
- Sudo or privileged execution.
- Module registry, starter template and generator.
- `apps/deploy`.
- SFTP, terminal, SSH agent, `~/.ssh/config` and SSH key generation.
- Streaming output, operation records and journals.
- Non-macOS verification.
- Release CI.
