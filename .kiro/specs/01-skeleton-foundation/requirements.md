# Requirements Document

## Introduction

S01 (01-skeleton-foundation) delivers the first working vertical slice of Ossiframe: a Tauri 2 desktop app that stores server entries locally, connects to an authorized Ubuntu 24.04 VPS over SSH with a pinned host key, runs read-only commands through a structured executor, and displays real server information with an observation timestamp. This document derives testable requirements from `design.md` for S01 only. It covers the monorepo skeleton, the three Rust crates, the two frontend packages, the starter desktop app, server CRUD (add/list/remove), host key trust and pinning, optional keychain credential storage, the executor contract (quoting, streaming, timeout, cancellation, redaction), the read-only server-info provider, the minimal UI and its error states, typed IPC and Tauri capabilities, quality gates, and manual verification on a real disposable VPS.

## Glossary

- **Ossiframe_Desktop**: The starter Tauri 2 desktop application (`templates/starter/`), comprising its React frontend and its Tauri backend.
- **Core**: The `ossiframe-core` crate (validated types, `CommandSpec`, `Executor` trait, `Redactor`, `host_key::decide`, `OperationRecord` types, errors).
- **SSH_Crate**: The `ossiframe-ssh` crate, providing `SshConnector` and `SshExecutor`.
- **Desktop_Crate**: The `ossiframe-desktop` crate, providing `Store` (SQLite), `Vault` (OS keychain), and IPC helpers (`SessionRegistry`, error mapping, event forwarding).
- **Store**: The SQLite-backed component in Desktop_Crate that persists server records and pinned host keys.
- **Vault**: The OS-keychain-backed component in Desktop_Crate that persists opted-in secrets.
- **Executor**: Any implementation of the Core `Executor` trait that runs a `CommandSpec` and returns an `ExecResult` or `ExecError`.
- **SshExecutor**: The SSH_Crate implementation of Executor that runs commands over an authenticated SSH session.
- **SshConnector**: The SSH_Crate component that establishes a TCP+SSH connection, performs host key verification, and authenticates.
- **HostKeyHandler**: The SSH_Crate component implementing the russh `check_server_key` callback, delegating the trust decision to Core's `host_key::decide`.
- **Redactor**: The Core component that removes registered secret values from text and from streamed output chunks.
- **CommandSpec**: The Core structure describing a program, arguments, privilege level, timeout, optional stdin, and a mutating flag.
- **ReadOnlyUserPolicy**: The Executor policy installed in S01 that rejects any `CommandSpec` with `mutating = true` or a privilege other than `User`.
- **Server_Info_Provider**: The `templates/starter/src-tauri/src/server_info/` module that runs read-only probes over an Executor and parses their output into a `ServerInfo`.
- **Client_Package**: The `@ossiframe/client` frontend package exposing one typed function per IPC command and the shared DTO types.
- **UI_Package**: The `@ossiframe/ui` frontend package of shared UI components used by more than one app.
- **IPC_Layer**: The set of `#[tauri::command]` functions exposed by Ossiframe_Desktop's Tauri backend and their typed DTOs/errors.
- **Pending_Trust_Entry**: An in-memory record `{ token, server_id, key, created_at }` created when a host key is presented with no existing pin, used to authorize a subsequent `trust_host_key` call.
- **Session_Registry**: The in-memory map from server ID to its connection state and (if connected) its SshExecutor handle.
- **Verification_Record**: The document written to `docs/verification/s01-skeleton-foundation.md` recording the manual verification run against a real VPS.

## Requirements

### Requirement 1: Monorepo and crate/package skeleton

**User Story:** As a developer, I want a working monorepo skeleton with the S01 crates and packages wired together, so that the rest of S01 has a buildable foundation that matches the intended architecture.

#### Acceptance Criteria

1. THE repository SHALL contain a Cargo workspace listing `crates/*` and `templates/starter/src-tauri` as members.
2. THE repository SHALL contain a pnpm workspace listing `packages/*` and `templates/starter` as members.
3. THE repository SHALL contain the crates `ossiframe-core`, `ossiframe-ssh`, and `ossiframe-desktop`, and the packages `@ossiframe/ui` and `@ossiframe/client`.
4. THE Core crate SHALL declare no dependency on russh, Tauri, SQLite, or any keychain library.
5. THE repository SHALL commit `Cargo.lock` and `pnpm-lock.yaml`.
6. WHEN the production build is run, THE repository's root scripts SHALL execute `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo check`, `cargo test`, `pnpm typecheck`, `pnpm lint`, and a production build of Ossiframe_Desktop without the model reporting failure.

### Requirement 2: Validated identifier types

**User Story:** As a developer, I want server connection fields validated into typed values at construction, so that malformed or malicious input cannot reach the executor or the shell.

#### Acceptance Criteria

1. WHEN a `ServerName` is constructed from a string of 1 to 64 characters (after trim) containing no control characters, THE Core SHALL return a valid `ServerName`.
2. IF a `ServerName` is constructed from an empty, over-64-character (after trim), or control-character-containing string, THEN THE Core SHALL return a validation error.
3. WHEN a `Hostname` is constructed from a valid IPv4 literal, IPv6 literal, or DNS name meeting the length, label, and character rules in the design, THE Core SHALL return a valid `Hostname`.
4. IF a `Hostname` is constructed from a string containing whitespace, a `/`, a leading `-`, or violating the length or label rules, THEN THE Core SHALL return a validation error.
5. WHEN a `Port` is constructed from an integer between 1 and 65535 inclusive, THE Core SHALL return a valid `Port`.
6. IF a `Port` is constructed from an integer outside 1 to 65535, THEN THE Core SHALL return a validation error.
7. WHEN an `SshUser` is constructed from a string matching `^[a-z_][a-z0-9_-]{0,31}$`, THE Core SHALL return a valid `SshUser`.
8. IF an `SshUser` is constructed from a string not matching that pattern, or beginning with `-`, THEN THE Core SHALL return a validation error.
9. WHEN a `KeyPath` is constructed from a non-empty, NUL-free string that resolves to an absolute path after `~` expansion, THE Core SHALL return a valid `KeyPath` without checking file existence.
10. IF a `KeyPath` is constructed from an empty string, a string containing NUL, or a string that does not resolve to an absolute path, THEN THE Core SHALL return a validation error.
11. WHEN a `ProgramName` is constructed from a non-empty, NUL-free string not starting with `-`, and not containing `/` unless it is an absolute path, THE Core SHALL return a valid `ProgramName`.
12. IF a `ProgramName` is constructed from a string violating any rule in Criterion 11, THEN THE Core SHALL return a validation error.
13. WHEN an IPC command receives a string-typed field corresponding to one of these validated types, THE IPC_Layer SHALL construct the type via its validating constructor before using the value, and SHALL return `IpcError::Validation` on failure.

### Requirement 3: Command quoting and specification

**User Story:** As a developer, I want every remote command built from a structured, quoted specification, so that no argument can be interpreted as shell syntax.

#### Acceptance Criteria

1. WHEN `quote_arg` is given a string containing no NUL byte, THE Core SHALL return a POSIX single-quoted representation of that string that is always quoted.
2. IF `quote_arg` is given a string containing a NUL byte, THEN THE Core SHALL return a `CommandError`.
3. WHEN `to_command_line` is given a `CommandSpec` with `privilege = User`, THE Core SHALL return the program and arguments individually quoted and space-joined, with no `sudo` prefix.
4. WHEN `to_command_line` is given a `CommandSpec` with `privilege = RunAs(user)`, THE Core SHALL return a command line prefixed with `sudo -n -u <quoted user> --`.
5. WHEN `to_command_line` is given a `CommandSpec` with `privilege = Root`, THE Core SHALL return a command line prefixed with `sudo -n --`.
6. THE Core SHALL construct every command line through `quote_arg` such that no unquoted shell metacharacter (`;`, `|`, `&&`, redirects) appears in the output.
7. WHEN `CommandSpec::read_only` is called with a program, argument list, and timeout, THE Core SHALL return a `CommandSpec` with `mutating = false` and `privilege = User`.

### Requirement 4: Executor contract and read-only policy

**User Story:** As a developer, I want a single executor abstraction that enforces the read-only, non-privileged policy of S01, so that no mutating or privileged command can run regardless of caller.

#### Acceptance Criteria

1. WHEN an Executor's `run` is called with a `CommandSpec`, THE Executor SHALL return either an `ExecResult` carrying the exit status, redacted stdout, redacted stderr, a truncation flag, and duration, or an `ExecError`.
2. THE Executor SHALL treat a non-zero exit code as part of a successful `Ok(ExecResult)`, not as an `ExecError`.
3. WHEN `ReadOnlyUserPolicy.check` is given a `CommandSpec` with `mutating = true`, THE ReadOnlyUserPolicy SHALL return `ExecError::PolicyRejected`.
4. WHEN `ReadOnlyUserPolicy.check` is given a `CommandSpec` with `privilege` other than `User`, THE ReadOnlyUserPolicy SHALL return `ExecError::PolicyRejected`.
5. WHEN `ReadOnlyUserPolicy.check` is given a `CommandSpec` with `mutating = false` and `privilege = User`, THE ReadOnlyUserPolicy SHALL allow the command to proceed.
6. IF a running command exceeds its `CommandSpec` timeout, THEN THE SshExecutor SHALL close the channel and return `ExecError::Timeout`.
7. IF a running command's `CancelToken` is cancelled, THEN THE SshExecutor SHALL close the channel and return `ExecError::Cancelled`.
8. IF the underlying connection ends before an exit status is observed, THEN THE SshExecutor SHALL return `ExecError::ConnectionLost`.
9. THE SshExecutor SHALL cap captured stdout and stderr at 256 KiB each and set `truncated = true` when a stream exceeds that cap.
10. WHEN an `on_output` callback is supplied to `run`, THE SshExecutor SHALL invoke it with each redacted output chunk as it arrives, tagged with its stream (stdout or stderr).
11. IF `run` returns `ExecError::Timeout`, `ExecError::Cancelled`, or `ExecError::ConnectionLost`, THEN THE Ossiframe_Desktop SHALL NOT automatically re-run the same command.

### Requirement 5: Output redaction

**User Story:** As a user, I want secrets scrubbed from any command output or log the app produces, so that my passwords and passphrases never appear in logs or the UI.

#### Acceptance Criteria

1. WHEN a secret is registered with a `Redactor` via `add_secret`, THE Redactor SHALL replace every subsequent exact occurrence of that secret in text passed to `redact` with `[REDACTED]`.
2. WHEN text containing one or more registered secrets is split into arbitrary chunks and streamed through a `StreamRedactor` via `push` followed by `finish`, THE Redactor SHALL produce concatenated output containing no registered secret as a substring.
3. THE Redactor SHALL hold back at most `max_secret_length - 1` bytes between `push` calls so that a secret split across a chunk boundary is still redacted.
4. WHEN `redact` is applied twice in sequence to the same text, THE Redactor SHALL produce the same result as applying it once.
5. THE Redactor SHALL redact registered secrets regardless of length, including secrets shorter than 4 characters.

### Requirement 6: Host key verification and pinning

**User Story:** As a user, I want the app to verify and pin the SSH host key of each server, so that a compromised or substituted server cannot intercept my credentials unnoticed.

#### Acceptance Criteria

1. WHEN `host_key::decide` is called with no pinned key, THE Core SHALL return `HostKeyDecision::Unknown` carrying the presented key.
2. WHEN `host_key::decide` is called with a pinned key whose type and public key bytes match the presented key, THE Core SHALL return `HostKeyDecision::Trusted`.
3. IF `host_key::decide` is called with a pinned key whose type or public key bytes differ from the presented key, THEN THE Core SHALL return `HostKeyDecision::Mismatch` carrying both keys.
4. THE Core SHALL NOT return `HostKeyDecision::Trusted` when no key is pinned for that server.
5. WHEN `fingerprint_sha256` is given a wire-format public key, THE Core SHALL return a string starting with `SHA256:`, containing no `=` padding, whose base64 portion decodes to exactly 32 bytes.
6. WHEN HostKeyHandler's `check_server_key` callback is invoked during a handshake, THE SSH_Crate SHALL return `true` to russh only when `host_key::decide` resolves to `Trusted`, and `false` otherwise.
7. IF the SSH handshake's host key decision is `Unknown`, THEN THE SSH_Crate SHALL NOT attempt authentication on that handshake, and `connect_server` SHALL return a `HostKeyUnknown` outcome carrying a Pending_Trust_Entry token.
8. IF the SSH handshake's host key decision is `Mismatch`, THEN THE SSH_Crate SHALL NOT attempt authentication on that handshake, and `connect_server` SHALL return `IpcError::HostKeyMismatch` carrying both fingerprints, the key type, and a replace token.
9. WHEN `trust_host_key` is called with a server ID and a token matching an unexpired Pending_Trust_Entry for that server, THE Store SHALL pin the key recorded in that Pending_Trust_Entry.
10. IF `trust_host_key` is called with an unknown, expired, or mismatched-server token, THEN THE IPC_Layer SHALL return an error and SHALL NOT pin any key.
11. THE Store SHALL expire a Pending_Trust_Entry 5 minutes after creation and remove it after it is used once.
12. WHEN `pin_host_key` is called for a server that already has a pinned key, THE Store SHALL return an error and SHALL NOT modify the existing pin.
13. WHEN `replace_host_key` is called with a valid replace token and a `confirm_host` string equal to the server's host, THE Store SHALL overwrite the existing pin with the new key.
14. IF `replace_host_key` is called with a `confirm_host` string that does not equal the server's host, THEN THE IPC_Layer SHALL return an error and SHALL NOT modify the existing pin.

### Requirement 7: SSH connection and authentication

**User Story:** As a user, I want to connect to my server using a key file or a password, so that I can run read-only inventory probes against it.

#### Acceptance Criteria

1. WHEN `connect_server` is invoked for a server configured with `AuthMethod::KeyFile`, THE SshConnector SHALL attempt key-based authentication only after the host key decision is `Trusted`.
2. WHEN `connect_server` is invoked for a server configured with `AuthMethod::Password`, THE SshConnector SHALL attempt password authentication only after the host key decision is `Trusted`.
3. IF the TCP connection cannot be established (DNS failure, refused, no route), THEN THE SshConnector SHALL return `ConnectError::Unreachable`.
4. IF no SSH banner or key exchange completes within the configured connect timeout, THEN THE SshConnector SHALL return `ConnectError::ConnectTimeout`.
5. IF the configured key file is missing, has wrong permissions, or is not a readable OpenSSH-format key, THEN THE SshConnector SHALL return `ConnectError::KeyFileUnreadable`.
6. IF the configured key file is encrypted and no or an incorrect passphrase is supplied, THEN THE SshConnector SHALL return `ConnectError::PassphraseRequired` or `ConnectError::AuthFailed`.
7. IF the server rejects the supplied key or password, THEN THE SshConnector SHALL return `ConnectError::AuthFailed` without indicating whether the username, key, or password was at fault.
8. WHEN authentication succeeds, THE SshConnector SHALL return an SshExecutor and THE Session_Registry SHALL record the server's connection state as Connected.
9. WHILE a session is connected, THE SshExecutor SHALL send a keepalive at the configured interval, and after the configured number of consecutive missed keepalives SHALL mark the session's state as Failed and emit a `connection-lost` event.

### Requirement 8: Server record storage

**User Story:** As a user, I want to add, list, and remove server entries, so that I can manage which servers the app knows about.

#### Acceptance Criteria

1. WHEN `add_server` is called with a valid `NewServerDto`, THE Store SHALL create a new server record with a generated UUID and return a `ServerDto`.
2. IF `add_server` is called with `save_secret = true` and the Vault write fails, THEN THE IPC_Layer SHALL roll back the created server row and SHALL return `IpcError::Keychain`.
3. WHEN `list_servers` is called, THE Store SHALL return every stored server record as a `ServerDto`, each with a boolean `has_saved_secret` and no secret value.
4. WHEN `remove_server` is called for an existing server ID, THE Desktop_Crate SHALL disconnect any active session for that server, delete its Vault entry if present, and then delete its Store row.
5. THE Store schema SHALL contain no column that stores a secret value; the `secret_ref` column SHALL hold only an opaque identifier.
6. THE Store SHALL apply its schema migrations as a numbered, ordered list executed within a transaction.

### Requirement 9: Secret storage in the OS keychain

**User Story:** As a user, I want to optionally save my server credentials in the OS keychain, so that I don't have to re-enter them on every connection.

#### Acceptance Criteria

1. WHERE a user enables "Save secret in OS keychain" when adding a server, THE Vault SHALL store the secret under an opaque entry ID and `add_server` SHALL set `secret_ref` on the server record.
2. WHERE a user does not enable "Save secret in OS keychain", THE Ossiframe_Desktop SHALL NOT persist the secret anywhere, and the secret SHALL be supplied by the frontend on each `connect_server` call.
3. WHEN a server with a saved secret is connected without an explicit `ConnectSecretDto`, THE Desktop_Crate SHALL load the secret from the Vault for that connection attempt.
4. WHEN a server is removed, THE Desktop_Crate SHALL delete its Vault entry before deleting its Store row.
5. THE Ossiframe_Desktop SHALL NOT write a secret value into SQLite, an IPC response, an event payload, or a log line.

### Requirement 10: Read-only server information

**User Story:** As a user, I want to see real information about a connected server, so that I can confirm what I'm managing.

#### Acceptance Criteria

1. WHEN `get_server_info` is called for a connected server, THE Server_Info_Provider SHALL run the hostname, os-release, kernel, arch, uptime, cpu-count, cpu-model, memory, and root-disk probes as read-only `CommandSpec`s with `Privilege::User`.
2. WHEN all probes for a server succeed, THE Server_Info_Provider SHALL return a `ServerInfoDto` with `hostname`, `os`, `kernel`, `arch`, `uptime_seconds`, `cpu`, `memory`, `root_disk` populated and `observed_at` set to the time the first probe started.
3. IF an individual probe fails or returns unparsable output, THEN THE Server_Info_Provider SHALL set that probe's field to `None` and SHALL append an entry to `probe_errors`, without failing the remainder of `get_server_info`.
4. THE Server_Info_Provider's parsers SHALL return `None` rather than panic when given malformed probe output.
5. WHEN `cancel_operation` is called while probes are running for a server, THE Server_Info_Provider SHALL cancel the in-progress and remaining probes for that server via its `CancelToken`.

### Requirement 11: Typed IPC commands

**User Story:** As a developer, I want all frontend-to-backend calls to go through typed, intent-level commands, so that the frontend cannot issue arbitrary shell, file, or network operations.

#### Acceptance Criteria

1. THE IPC_Layer SHALL expose exactly the commands `add_server`, `list_servers`, `remove_server`, `connect_server`, `trust_host_key`, `replace_host_key`, `disconnect_server`, `get_server_info`, and `cancel_operation`, each accepting a typed DTO and returning a typed result or `IpcError`.
2. THE IPC_Layer SHALL expose no command that accepts a raw shell command line, an arbitrary file path to read, or an arbitrary URL.
3. WHEN any IPC command handler encounters a validation, connection, authentication, policy, storage, or keychain failure, THE IPC_Layer SHALL return the corresponding `IpcError` variant defined in the design, with `stderr_excerpt` redacted and capped at 2 KiB where present.
4. THE Client_Package SHALL expose exactly one typed async function per IPC command and the corresponding DTO and error types, and SHALL be the only code in the frontend that calls `invoke`.
5. THE IPC_Layer SHALL emit `connection-state` and `connection-lost` events carrying the affected server ID.

### Requirement 12: Minimal UI and error states

**User Story:** As a user, I want a minimal interface for managing servers and viewing their status and information, so that I can operate the app without a terminal.

#### Acceptance Criteria

1. WHEN the server list page is displayed, THE Ossiframe_Desktop SHALL show each stored server with a status badge reflecting its connection state (disconnected, connecting, connected, failed) and the prefix of its pinned fingerprint, if any.
2. WHEN the add-server dialog receives an `IpcError::Validation` for a field, THE Ossiframe_Desktop SHALL display the validation message next to that field.
3. WHEN a `connect_server` call returns a `HostKeyUnknown` outcome, THE Ossiframe_Desktop SHALL display the key type and full SHA-256 fingerprint and offer Trust and Cancel actions.
4. WHEN a `connect_server` call returns `IpcError::HostKeyMismatch`, THE Ossiframe_Desktop SHALL display both fingerprints, SHALL NOT offer a way to continue the connection, and SHALL offer a "Replace pinned key" action requiring a typed host confirmation.
5. WHEN the server overview page receives a `ServerInfoDto`, THE Ossiframe_Desktop SHALL display each field or, for fields that are `None`, an "unavailable" indicator with the corresponding probe error on request, alongside the observation time.
6. WHEN an IPC call returns an `IpcError`, THE Ossiframe_Desktop SHALL render a message appropriate to its kind (authentication failed, unreachable, timeout, command failed with exit code and stderr excerpt, cancelled, or connection lost).
7. THE Ossiframe_Desktop SHALL render remote command output and stderr excerpts as plain text, never through `dangerouslySetInnerHTML`.
8. THE add-server dialog's "Save secret in OS keychain" option SHALL default to off.

### Requirement 13: Tauri security configuration

**User Story:** As a user, I want the desktop app configured with a restrictive security policy, so that a compromised webview cannot reach the filesystem, network, or shell beyond what S01 needs.

#### Acceptance Criteria

1. THE Ossiframe_Desktop's content security policy SHALL NOT include `unsafe-eval`, and the production build SHALL fail if it does.
2. THE Ossiframe_Desktop's Tauri capability configuration SHALL grant access only to the `main` window and core event-listening defaults, with no `fs`, `shell`, `http`, `opener`, or `dialog` plugin capability.
3. THE Ossiframe_Desktop SHALL register only the commands listed in Requirement 11.1 for frontend invocation.
4. THE Ossiframe_Desktop SHALL disable devtools in release builds.

### Requirement 14: Quality gates and manual verification

**User Story:** As a developer, I want automated checks and a documented manual verification run against a real server, so that S01 is provably correct before it is considered done.

#### Acceptance Criteria

1. THE repository SHALL pass `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo check`, `cargo test`, `pnpm typecheck`, `pnpm lint`, and a production build of Ossiframe_Desktop before S01 is considered complete.
2. THE repository SHALL run `shellcheck` on every script added during S01, if any script is added.
3. WHEN the quoting, identifier validation, redaction, and host key decision logic are tested, THE test suite SHALL include property-based tests for Properties 1 through 6 defined in `design.md`, each run for a minimum of 100 iterations.
4. WHEN manual verification is performed, THE developer SHALL record the result of verification points V1 through V12 in `docs/verification/s01-skeleton-foundation.md`, including date, app commit, and server image, and SHALL NOT record a verification point that was not actually run.
5. IF the manual verification run finds no secret leakage when searching the SQLite file, application log, and IPC traces for the test password and test passphrase, THEN THE Verification_Record SHALL note this as confirmed; otherwise THE developer SHALL treat S01 as not done.

## Non-Goals

The following are explicitly out of scope for S01, per `design.md`:

- Server provisioning of any kind.
- Any server-mutating operation; only read-only commands run in S01.
- Execution with `sudo`/root privilege; `Privilege::RunAs` and `Privilege::Root` are modeled in Core but rejected by `ReadOnlyUserPolicy`.
- The module registry and generator (planned for S02).
- The `apps/deploy` and `apps/doctor` applications.
- SFTP file transfer.
- An interactive terminal.
- SSH agent support.
- Reading or writing `~/.ssh/config`.
- SSH key generation.
- Verification on any operating system other than macOS.
- Release CI pipelines.
- Persisting `ServerInfo` or `OperationRecord` data in SQLite (both are in-memory or types-only in S01).
- A native file picker for selecting the SSH key file (a typed path field is used instead).
