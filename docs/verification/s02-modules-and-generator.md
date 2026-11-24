# S02 Verification

Status: **complete**. All twelve completion criteria are Met or Met with note. The Standalone_Mode app built from public rev 3ebdfc1 was run against the Lightsail server on 2026-11-25. Notes and not-verified items are listed at the end.

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

## Generated apps (local mode)

**Scope:** these are Local_Mode builds (`--ossiframe-local /Users/shojibur/Desktop/kiro/Ossiframe`, dirty working tree). They are NOT the public-rev independent verification (task 14). Standalone_Mode from a Published_Rev remains unverified. No server was contacted in this step; running the apps against the Lightsail server is task 13.2 (manual).

Fresh copies were generated from the current code (`~/ossiframe-gen-test` deleted first), then `pnpm install` and the full bundle `pnpm tauri build` ran in each:

| App | Command flags | Identifier | Result |
|---|---|---|---|
| minimal (`Ossi Minimal`) | no modules | `com.example.ossiminimal` | build OK, `.app` and `.dmg` bundled |
| full (`Ossi Full`) | `--modules linux-inventory --with-example-module` | `com.example.ossifull` | build OK, `.app` and `.dmg` bundled |

Built bundles:
- `/Users/shojibur/ossiframe-gen-test/minimal/src-tauri/target/release/bundle/macos/Ossi Minimal.app`
- `/Users/shojibur/ossiframe-gen-test/full/src-tauri/target/release/bundle/macos/Ossi Full.app`

Checks (grep and listing of the generated trees):
- **Monorepo references:** in `Cargo.toml`, `package.json`, `vite.config.ts`, `tsconfig*.json`, `pnpm-workspace.yaml`, `packages/*/package.json` and `tauri.conf.json`, the only references to the checkout are the local-mode path dependencies in `src-tauri/Cargo.toml` (`ossiframe-core`, `ossiframe-desktop`, and in full also `ossiframe-mod-linux-inventory`) and `modules/ports/Cargo.toml` (`ossiframe-core`). The full app's `ossiframe-mod-ports-example` is `path = "../modules/ports"` inside the app. `modules/ports/Cargo.toml` also carries the public repository URL as `repository` metadata only. No `[patch]` and no git source in `Cargo.lock` (0 matches). No `dev.ossiframe`, `ossiframe-doctor` or `doctor.sqlite` strings in either tree (excluding `node_modules`, `target`, `dist`, lockfiles).
- **Identity:** `tauri.conf.json` has `productName`/`identifier` `Ossi Minimal`/`com.example.ossiminimal` and `Ossi Full`/`com.example.ossifull`. Both use `db_file: "app.sqlite3"`; `ossiframe_desktop::tauri_support::setup` takes the keychain service name and the data directory from `app.config().identifier` (`tauri_support.rs` lines 42-61), so the namespaces differ per app. Not exercised at run time yet (13.2).
- **Minimal:** no `src/modules`, no `modules/` directory, no `ossiframe-mod-*` dependency in `src-tauri/Cargo.toml`.
- **Full:** `src/modules/linux-inventory`, `src/modules/ports` and `modules/ports` exist.
- **Capabilities:** both `src-tauri/capabilities/main.json` list `core:event:default` plus exactly the eleven shared commands (`list-servers`, `add-server`, `update-server`, `remove-server`, `connect-server`, `disconnect-server`, `trust-host-key`, `replace-host-key`, `cancel-operation`, `run-operation`, `list-modules`).

Not verified here: running either app, connecting to a server, Standalone_Mode.

## Generated apps and Doctor on the Lightsail server (task 13.2)
**Scope:** manual check by the user on 2026-11-24, reported as "All passed" for A, B and C. The generated apps are Local_Mode builds (`--ossiframe-local`, dirty working tree). Public-rev (Standalone_Mode) verification is still pending (task 14).

Server: `54.179.76.25:22`, user `ubuntu`, key `~/.ssh/ossiframe.pem`.

| App | Identifier | Result |
|---|---|---|
| A. Ossi Minimal.app (zero modules) | `com.example.ossiminimal` | Started with no profiles. A new profile was added. The fingerprint dialog appeared on first connect. Connected. Only connection info was shown, with no inventory page. |
| B. Ossi Full.app (`linux-inventory` + ports example) | `com.example.ossifull` | Connected. Overview showed real inventory. On the Ports tab, Load with All, IPv4 and IPv6 showed real listening TCP ports. |
| C. Ossiframe Doctor.app (after the UI move to shared packages) | Doctor identifier | Connected. Inventory shown. Refresh, Disconnect (stale marking) and reconnect worked. |

Notes:
- Each generated app keeps its own data directory and keychain namespace, derived from its identifier. Ossi Minimal started with no profiles, so profiles are not shared between apps.
- Follow-up from the user: the Ports tab showed the expected listening ports. The specific ports were not itemized, and the page output was not compared with `ss -Htln` output.
- Screenshots from the user (2026-11-25, about 07:20 MYT) of Ossi Full and Doctor showed hostname `ip-172-26-3-117`, Ubuntu 24.04.5 LTS, kernel 7.0.0-1014-aws, x86_64, 2 x Intel Xeon Platinum 8175M, 0.4 GiB memory, root filesystem 18.3 GiB (2.5 GiB used, 13%), and pinned key `ssh-ed25519 SHA256:qobMUOP//FiL3rW2mD42syTV5iK5BzW0Dw9Rylibswk`. These are consistent with the OpenSSH reference values recorded in S01. Disk usage and uptime changed with time.

### Reference: listening TCP sockets (`ss -Htln`)
Not collected. The agent's sandbox blocked the `ssh-keyscan` call that was needed to build the pinned `UserKnownHostsFile` (the fingerprint check against `SHA256:qobMUOP//FiL3rW2mD42syTV5iK5BzW0Dw9Rylibswk` was not run). No SSH connection was made and no output is recorded. To add the reference, run `ss -Htln` on the server and append the local addresses and ports here.

## Standalone mode re-check from rev 3ebdfc1 (task 15)

Rev 3ebdfc1e4db310dda643fa6c439d709e6b5876a6 (contains the Cargo.toml.tmpl fix) is pushed to `origin/main`. The tree was clean, default `~/.cargo` (CARGO_HOME unset). No server was contacted.

- Generated without `--rev` into `~/ossiframe-public-test/app2` (`--name "Ossi Public" --identifier com.example.ossipublic2 --modules linux-inventory --with-example-module`). The generator pinned 3ebdfc1e… in `src-tauri/Cargo.toml`.
- No occurrence of `/Users/shojibur/Desktop/kiro/Ossiframe` in the generated project (excluding `node_modules` and `target`).
- `pnpm install`, `pnpm typecheck`, `pnpm lint`: passed.
- `pnpm tauri build` (full bundle): passed, exit 0. The earlier `invalid key-value pair` Cargo message did not appear (0 matches in the build log).
- `Cargo.lock`: ossiframe-core, -desktop, -ssh and -mod-linux-inventory all resolve to `git+https://github.com/shojibur/Ossiframe?rev=3ebdfc1e…#3ebdfc1e…`.
- `cargo test` in `src-tauri`: 2 passed, 0 failed.
- App: `~/ossiframe-public-test/app2/src-tauri/target/release/bundle/macos/Ossi Public.app` (also a `.dmg` in `bundle/dmg/`).
- Running this app against the server (user, 2026-11-25): the user ran `Ossi Public.app` (identifier `com.example.ossipublic2`, built in Standalone_Mode purely from public rev 3ebdfc1e4db310dda643fa6c439d709e6b5876a6 with `Cargo.lock` git sources) against the Lightsail server `54.179.76.25`. The user added the profile and connected. The fingerprint shown matched `ssh-ed25519 SHA256:qobMUOP//FiL3rW2mD42syTV5iK5BzW0Dw9Rylibswk`. Overview showed real inventory and Ports showed listening ports. The user reported "Worked". The specific ports were not itemized and not compared with `ss -Htln`.

## Standalone mode from the published rev (task 14.2)
**Published_Rev:** `4c8857e444edf60f1274ddf352bd6fcd0007feb1` (`origin/main` = `4c8857e`).

**Task 14.1 (no new push):** `git diff --stat 4c8857e -- crates modules examples packages templates` printed nothing. Since 4c8857e only docs/spec files, a test-only assertion in `tools/ossiframe-cli/src/write.rs`, and the deletion of the empty `tools/ossiframe-cli/src/rite.rs` changed. Nothing that a generated app depends on differs from the published rev.

**Generate** (run from the repository, output outside it):
`cargo run -p ossiframe-cli -- new ~/ossiframe-public-test/app --name "Ossi Public" --identifier com.example.ossipublic --modules linux-inventory --with-example-module --rev 4c8857e444edf60f1274ddf352bd6fcd0007feb1`
The earlier test directory was moved aside to `~/ossiframe-public-test.old-*` rather than deleted. The generator succeeded. A second invocation correctly refused: `dir: exists and is not empty; refusing to write into it`.

**Independence:** grepping the generated `Cargo.toml`, `package.json`, `vite.config.ts`, `tsconfig*` and `src-tauri/Cargo.lock` for `/Users/shojibur/Desktop/kiro/Ossiframe` found nothing. `src-tauri/Cargo.toml` has git dependencies on the rev for `ossiframe-core`, `ossiframe-desktop` and `ossiframe-mod-linux-inventory`, and `ossiframe-mod-ports-example = { path = "../modules/ports" }`.

`src-tauri/Cargo.lock` sources (all four resolve to the public rev):
```
ossiframe-core                 git+https://github.com/shojibur/Ossiframe?rev=4c8857e444edf60f1274ddf352bd6fcd0007feb1#4c8857e444edf60f1274ddf352bd6fcd0007feb1
ossiframe-desktop              git+https://github.com/shojibur/Ossiframe?rev=4c8857e444edf60f1274ddf352bd6fcd0007feb1#4c8857e444edf60f1274ddf352bd6fcd0007feb1
ossiframe-mod-linux-inventory  git+https://github.com/shojibur/Ossiframe?rev=4c8857e444edf60f1274ddf352bd6fcd0007feb1#4c8857e444edf60f1274ddf352bd6fcd0007feb1
ossiframe-ssh                  git+https://github.com/shojibur/Ossiframe?rev=4c8857e444edf60f1274ddf352bd6fcd0007feb1#4c8857e444edf60f1274ddf352bd6fcd0007feb1
```

**Build and checks** (in `~/ossiframe-public-test/app`):
| Command | Result |
|---|---|
| `pnpm install` | Done |
| `pnpm typecheck` | Passed (app, `packages/client`, `packages/ui`) |
| `pnpm lint` | Passed |
| `pnpm tauri build` | Passed. Git fetch from GitHub succeeded. Bundled `.app` and `.dmg`. |
| `cargo test` in `src-tauri` | 2 passed, 0 failed |

CARGO_HOME: the agent shell had `CARGO_HOME=~/ossiframe-public-test/cargo-home` set. The first full build used that isolated home. The build was then repeated with `CARGO_HOME` unset (default `~/.cargo`). It fetched `https://github.com/shojibur/Ossiframe` into `~/.cargo/git/checkouts/ossiframe-caa951a4007c8b17/4c8857e`, built successfully, and left `Cargo.lock` unchanged. `cargo test` was also run with the default home.

Cargo prints a non-fatal `error: invalid key-value pair, expected key` for `templates/starter/src-tauri/Cargo.toml:18` (`{{ossiframe_deps}}`) inside the git checkout while it searches for the package. The build and tests succeed. This comes from the placeholder in the starter template.

**Template fix (after 4c8857e, uncommitted):** the template manifest was renamed to `templates/starter/src-tauri/Cargo.toml.tmpl`, so Cargo no longer sees an invalid `Cargo.toml` in the repository. `copy_tree` in `tools/ossiframe-cli/src/write.rs` writes it as `Cargo.toml` when rendering the starter. `render.rs` test and `write.rs` test were updated (the generation test asserts `Cargo.toml` exists and `Cargo.toml.tmpl` does not). No other template manifest holds tokens.

Fix checks (2026-11-25): `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` passed. A Local_Mode app generated into `~/ossiframe-gen-test/fixcheck` (no modules, `com.example.ossifixcheck`) contains `src-tauri/Cargo.toml` and no `.tmpl`, and `cargo check` in its `src-tauri` finished successfully.

The public-rev verification at 4c8857e stays valid for the dependency crates, since the fix touches only the template and the generator. The fix is contained in rev 3ebdfc1, and the build from that rev showed no `invalid key-value pair` message (see the 3ebdfc1 section).

**Artifacts:**
- `~/ossiframe-public-test/app/src-tauri/target/release/bundle/macos/Ossi Public.app`
- `~/ossiframe-public-test/app/src-tauri/target/release/bundle/dmg/Ossi Public_0.1.0_aarch64.dmg`

**Running the app against the server:** the 4c8857e app (`com.example.ossipublic`) itself was not run. The run was done on the 3ebdfc1 app (see the 3ebdfc1 section).

## S02 completion checklist (Requirement 16)

Only recorded facts are used. "Met with note" means the evidence is real but narrower than the wording.

| # | Criterion | Status | Evidence |
|---|---|---|---|
| 1 | Doctor connects and shows real inventory | Met | Task 6.3 and app C above: the user connected Doctor to the Lightsail server after the migration and after the UI move, and saw real inventory. |
| 2 | Doctor consumes `linux-inventory` | Met | Doctor's Overview loads data through `run_operation` with `linux-inventory` / `server-info` (task 6.3). The old `get_server_info` command is gone. |
| 3 | Registry and Resolver work | Met | Unit tests in `ossiframe-core` (duplicate id, missing dependency, cycle, duplicate operation, deterministic order, unknown operation) and in the generator (unknown module, unknown dependency, cycle, automatic dependency inclusion). Doctor builds its Registry at startup. |
| 4 | Minimal app generated | Met | `Ossi Minimal` generated with zero modules, built (`.app` and `.dmg`), run by the user (app A). |
| 5 | App with selected modules generated | Met | `Ossi Full` generated with `linux-inventory` and the ports example, built, run by the user (app B). |
| 6 | Generated app builds outside the monorepo | Met | `Ossi Public` (`com.example.ossipublic2`) was generated into `~/ossiframe-public-test/app2` and built with `pnpm tauri build` purely from public rev 3ebdfc1, with `Cargo.lock` git sources for all four Ossiframe crates and no checkout path in the project. Earlier builds at 4c8857e and Local_Mode builds also passed. |
| 7 | Generated app connects to the Lightsail server | Met | The 3ebdfc1 Standalone_Mode app connected on 2026-11-25 and the fingerprint matched `SHA256:qobMUOP//FiL3rW2mD42syTV5iK5BzW0Dw9Rylibswk`. Local_Mode apps A and B also connected earlier. |
| 8 | Ports module retrieves real listening ports | Met with note | On app B the Ports tab showed the expected listening TCP ports for All, IPv4 and IPv6. The ports were not itemized and not compared with `ss -Htln` output (reference not collected). |
| 9 | Custom module added without Core changes | Met | Source check (2026-11-25): `examples/custom-module/src/lib.rs` imports from `ossiframe_core` only `CommandSpec`, `Module`, `ModuleId`, `ModuleManifest`, `OperationContext`, `OperationError` and `OperationInfo` (plus `CancelToken`, `ExecError`, `ExecResult`, `Executor`, `ExitStatus` in its tests). All are public crate-root exports. `src/parse.rs` has no `ossiframe` references. `Cargo.toml` depends on `ossiframe-core` only among Ossiframe crates (no `ossiframe-desktop`, `-ssh` or Doctor). The UI folder imports only `@ossiframe/ui` and `@ossiframe/client`. The crate ran in app B. This is an import check, not a git diff of Core. |
| 10 | Standalone_Mode verified from a Published_Rev | Met | App built purely from public rev 3ebdfc1 (contains the template fix) was run by the user against the Lightsail server: connected, fingerprint matched, Overview showed real inventory, Ports showed listening ports ("Worked"). |
| 11 | `docs/developer-guide.md` exists | Met | File exists (task 11) and now states that standalone mode was verified to build and run against a real server. |
| 12 | Results recorded | Met with note | This document. Open items are listed below. |

### Quality gate (task 12)

Task 12 was reported complete by the gate run (`cargo fmt`, `check`, `clippy -D warnings`, `test`, `pnpm -r typecheck`, `pnpm -r lint`, audits, Doctor production build). This file does not hold the per-command output from that run. After the template fix, `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` were run again and passed. `pnpm` checks and audits were not re-run after the fix, which does not touch TypeScript.

### Not verified

- Port-by-port comparison of the Ports tab with `ss -Htln`.
- Changed host key behavior and password authentication (see S01).
- Any server other than the Lightsail Ubuntu 24.04.5 x86_64 instance.

### S01 open checks (unchanged, outside S02 completion)

- Independent fingerprint comparison through the Lightsail console: pending.
- Password authentication: unverified and disabled.
