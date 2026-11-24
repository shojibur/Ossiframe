# S02 Verification

Status: **in progress**. Only the spike below is recorded so far.

## Environment (local)

- Date: 2026-11-24
- macOS on Apple Silicon (aarch64)
- tauri 2.12 (see `Cargo.lock`)

## Spike: shared Tauri commands

Question: can Tauri commands live in `ossiframe-desktop` and be listed by full path in an app's `generate_handler!`?

**Chosen path: direct.** The macro and thin-wrapper fallbacks were not needed.

What was done (task 1.1):

- `ossiframe-desktop` got an optional `tauri` dependency (`default-features = false`) behind the feature `tauri`.
- A `tauri_support::commands` module holds `list_servers` and `add_server` as `pub async` `#[tauri::command]` functions.
- Doctor lists them by full path in `generate_handler!`.
- Doctor's `build.rs` `AppManifest` and `capabilities/main.json` were unchanged and worked.

Results:

| Check | Result |
|---|---|
| `cargo check -p ossiframe-doctor` | Pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | Pass |
| `cargo check -p ossiframe-desktop` (without the feature) | Pass |
| `cargo tree -p ossiframe-desktop -e normal` (without the feature) | No `tauri` in the tree |
| `cargo test --workspace` | Pass |
| `pnpm --filter @ossiframe/doctor tauri build --no-bundle` | Pass (release build, including ACL and capability generation) |

Source check: in tauri-macros 2.x, `command/wrapper.rs` adds `#[macro_export]` for `pub` commands, and `generate_handler!` rewrites only the last path segment of each listed command.

Not yet verified: a runtime IPC call from the launched app to a moved command. This is pending and is covered by task 6.3.

## Doctor after migration (task 6.3)

Date: 2026-11-24. Release binary `target/release/ossiframe-doctor`, run by the user against the Lightsail server `54.179.76.25`.

User-confirmed observations:
- The existing profile and pinned fingerprint were kept.
- Connected without a fingerprint prompt.
- Real inventory was shown, loaded through `run_operation` with `linux-inventory` / `server-info`.
- Refresh worked.
