# S01 Verification

Status: **ready for live verification**. Local checks below were run. Live checks against a real Ubuntu 24.04 VPS have **not** been run yet.

## Environment (local)

- Date: 2026-11-24
- macOS on Apple Silicon (aarch64)
- rustc 1.100.0, Node 24.13.0, pnpm 11.22.0, TypeScript 6.0.3
- russh 0.65.4, rusqlite 0.43.0 (bundled SQLite), keyring-core 1.1.0 + apple-native-keyring-store 1.0.3, tauri 2.12.7

## Local checks run

| Check | Result |
|---|---|
| `cargo fmt --all --check` | Pass |
| `cargo check --workspace --all-targets` | Pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | Pass |
| `cargo test --workspace` | 57 passed, 1 ignored (live test) |
| `pnpm -r typecheck` | Pass |
| `pnpm -r lint` (includes `react/no-danger`) | Pass |
| `pnpm audit` | No known vulnerabilities |
| `cargo audit` (cargo-audit 0.23.0, 542 crates) | No vulnerabilities. 3 warnings: `chrono` unmaintained (RUSTSEC-2026-0543, via russh-util); `proc-macro-error` unmaintained (RUSTSEC-2024-0370) and `glib` unsound (RUSTSEC-2024-0429), both via Tauri's Linux GTK stack and not compiled into the macOS build |
| `pnpm tauri build` | Pass: `Ossiframe Doctor.app` (16.26 MiB), `.dmg` (5.40 MiB) |
| Release binary launch | Process started and created `doctor.sqlite3` in `~/Library/Application Support/dev.ossiframe.doctor/`. UI not inspected visually (screenshot was blocked by OS permissions). |
| Local DB schema | Inspected with `sqlite3 .schema`: no password, passphrase or key column |
| Fingerprint format | Unit test matches local `ssh-keygen -lf` output for an Ed25519 test key |
| Argument quoting | Unit test round-trips hostile arguments through local `/bin/sh` |

## Corrections after review (2026-11-24)

- Editing host, port, username, auth method or key path deletes the saved secret. Regression test: `endpoint_change_never_reuses_saved_secret`.
- Authentication has an overall 30 s deadline; failure or timeout closes the connection with a bounded (2 s) disconnect.
- Command timeout and cancel now cover channel open, exec, output and exit status; channel cleanup is bounded to 2 s.
- Unused `with_redactions` removed: authentication secrets are not retained after login and are never part of a command. Output still goes through bounded capture and whole-output redaction.
- If the keychain write succeeds but the SQLite reference update fails, the new keychain entry is deleted, the session is closed and an error is returned. Failure to delete a replaced keychain entry is reported as a warning.
- Frontend: a credential held during host key confirmation is bound to one server and trust token and is cleared on success, failure, cancel, a new attempt, profile edit or removal.

## Live checks (pending, need a disposable Ubuntu 24.04 VPS)

Run `OSSIFRAME_TEST_HOST=… OSSIFRAME_TEST_USER=… OSSIFRAME_TEST_KEY=… cargo test -p ossiframe-ssh --test live -- --ignored --nocapture`, then the manual UI checks:

- [ ] Unknown host key aborts before authentication (check server `auth.log`)
- [ ] Wrong pin is rejected; correct pin connects
- [ ] Ed25519 key without passphrase; with passphrase (prompt shown); wrong passphrase
- [ ] Password authentication, with and without saving to the keychain
- [ ] Reconnect after app restart with a saved secret; prompt when nothing is saved
- [ ] Exit code and stderr of a failing command
- [ ] Command timeout and cancel; session still usable afterwards
- [ ] Network drop is detected by russh keepalive and shown as disconnected
- [ ] Changed host key blocks connection; replacement needs the typed host name
- [ ] Inventory values match `cat /etc/os-release`, `uname -a`, `nproc`, `free -b`, `df -B1 /`
- [ ] Unreachable host and wrong port errors
- [ ] Editing host or port deletes the saved secret and requires new host key verification
- [ ] Test password and passphrase not present in the SQLite file or app logs
- [ ] Keychain behavior with the unsigned build (prompts, locked keychain)
- [ ] Idle memory of the running app
