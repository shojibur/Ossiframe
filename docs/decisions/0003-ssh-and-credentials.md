# ADR-0003: SSH, authentication and credential storage

Status: Accepted (2026-11-24). russh behavior is validated in S01.

## Decision
- SSH library: russh, used only inside `ossiframe-ssh`. No russh types in public Core APIs.
- Authentication: private key (OpenSSH format, optional passphrase) and password. Password is mainly for first bootstrap.
- Host keys: show the SHA-256 fingerprint on first connection; the user confirms; the key is pinned in SQLite. A different key on a later connection fails the connection until the user explicitly replaces the pin.
- Generated management keys: Ed25519.
- Secrets (private keys, passphrases, saved passwords) live in the OS keychain through the `keyring` crate. SQLite stores only a reference to the keychain entry.
- Imported key files may be referenced by path instead of copied.
- Secrets never cross IPC back to the frontend.

## Rationale
russh is pure Rust and async-native, which suits streaming, timeouts and cancellation, and avoids C toolchain issues. The host key check is a callback we control. The OS keychain avoids custom crypto and a master password.

## Unverified assumptions
- russh supports everything S01 needs against Ubuntu 24.04 OpenSSH: key and password auth, host key callback, exec with separate stdout/stderr, exit status, timeouts, cancellation, and detection of dropped connections.
- Keychain access works on macOS with unsigned development builds without a prompt on every rebuild.
- Windows Credential Manager size limits do not affect Ed25519 keys.

## Open questions
- SSH agent support and `~/.ssh/config` parsing. Planned after S01.
- If russh fails a requirement in S01, the fallback candidate is `ssh2` (libssh2) behind the same executor interface.
