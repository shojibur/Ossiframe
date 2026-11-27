# Design Document: Distribution and Verification (S06)

## Overview

S06 makes Ossiframe Doctor and Ossiframe Deploy installable from GitHub Releases, checks every push in CI, proves that generated projects build outside the repository from a published commit, and writes the developer docs and the known-limitations list. It adds no runtime feature and no Rust crate.

Scope:
- A CI workflow for pushes and pull requests with the required checks from `engineering.md`.
- A release workflow, started by a version tag, that builds both apps for macOS Apple Silicon and creates a draft GitHub Release with checksums.
- Generated project verification in CI, in standalone mode pinned to the commit under test.
- Docs: install instructions, developer guide updates for Deploy and the provisioning and app-deploy modules, and `docs/known-limitations.md`.

Out of scope: Intel macOS, Linux and Windows builds; an auto-updater (it would need a hosted update feed); crates.io or npm publishing; Apple Developer ID signing and notarization unless the user provides an Apple Developer account (see Decisions).

Release gating: the first tag is created only after the essential S04 and S05 live checks pass. The release workflow creates a draft; only the user publishes it.

## Decisions

| Decision                    | Choice                                                                                                                                                                                                                                                                                                     | Why                                                                                                                                                                                                                                                          | Rejected                                                                               |
| --------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------- |
| Build tool in CI            | `tools/build-release.sh`: `pnpm tauri build` per app, collecting each app right after its build, then `gh release create --draft` with the files                                                                                                                                                           | Two apps in one workspace; no third-party build action; `gh` is preinstalled on GitHub runners. Building the second dmg recreates `target/release/bundle/dmg` and deletes the first, found in a local run, so each app is collected straight after its build | `tauri-apps/tauri-action`: designed for one app per repo and adds an action dependency |
| Third-party actions         | `actions/checkout`, `actions/setup-node`, `pnpm/action-setup`, `Swatinem/rust-cache`, `actions/upload-artifact`, `actions/download-artifact`, each pinned to a full commit SHA with the version in a comment. The Rust toolchain comes from `rust-toolchain.toml` through the runner's preinstalled rustup | Supply-chain safety. The artifact actions pass files from the build job to the release job, which holds the only write permission, and keep dry-run outputs on the run                                                                                       | Tag-pinned actions; `dtolnay/rust-toolchain` (not needed with `rust-toolchain.toml`)   |
| Workflow permissions        | `contents: read` at workflow level; `contents: write` only on the release job                                                                                                                                                                                                                              | Least privilege                                                                                                                                                                                                                                              | Repository-wide write token                                                            |
| macOS signing               | Ad-hoc signature (`signingIdentity: "-"`) by default. Developer ID signing and notarization run only when the `APPLE_*` secrets are present                                                                                                                                                                | Apple Silicon requires a signature for downloaded apps; ad-hoc costs nothing. Gatekeeper still warns on an ad-hoc app, so install docs explain how to open it                                                                                                | Shipping unsigned; requiring a paid Apple account                                      |
| Release artifacts           | Per app: `.dmg` and zipped `.app`, plus one `SHA256SUMS` file                                                                                                                                                                                                                                              | Checksums per ADR-0007                                                                                                                                                                                                                                       | Separate checksum files per artifact                                                   |
| Versioning                  | One version for the workspace and both apps (`Cargo.toml`, both `tauri.conf.json`, both app `package.json`). Tag `v<version>`. `tools/check-version.sh` fails if the tag and the five versions differ                                                                                                      | Two apps released together                                                                                                                                                                                                                                   | Per-app tags                                                                           |
| Generated project check     | CI job generates a read-only app (`linux-inventory`) and a provisioning app (`--enable-provisioning`) with `--rev $GITHUB_SHA` outside the checkout, then runs `cargo check`, `pnpm install`, `pnpm typecheck` and the frontend build, and greps the read-only app for `provision_`                        | Proves standalone mode against a commit that exists on GitHub                                                                                                                                                                                                | Local mode only                                                                        |
| app-deploy in the generator | Not offered in S06                                                                                                                                                                                                                                                                                         | Its Tauri commands live in `apps/deploy/src-tauri`, so a generated app cannot use it without moving them. Recorded as a known limitation                                                                                                                     | Moving the commands now (scope growth)                                                 |
| Script linting              | One script, `tools/check-version.sh`, so the version check runs the same locally and in CI. `shellcheck` runs on every `*.sh` file in CI                                                                                                                                                                   | Required check from `engineering.md`                                                                                                                                                                                                                         |                                                                                        |

## CI workflow (`.github/workflows/ci.yml`)

Triggers: `push` to `main`, `pull_request`. Runner: `macos-26` (Apple Silicon) for the full job, so the same platform as releases is tested. `macos-14` was retired by GitHub on 2026-11-02. `shellcheck` runs in a separate job on `ubuntu-24.04`, because the macOS image does not include it.

Steps: checkout; Rust toolchain from `rust-toolchain.toml` if present, else stable with `clippy` and `rustfmt`; rust cache; pnpm and Node 24 with cache; `pnpm install --frozen-lockfile`; then:
- `cargo fmt --check`
- `cargo clippy --workspace --all-targets -- -D warnings` and with `--all-features`
- `cargo test --workspace --all-features`
- `cargo check -p ossiframe-doctor` and `cargo check -p ossiframe-deploy`, and `cargo tree -p ossiframe-doctor` must not contain `app-deploy` or `ubuntu-provisioning`
- `pnpm typecheck`, `pnpm lint`
- `cargo audit --deny warnings` (installed with `cargo install --locked cargo-audit`, cached) and `pnpm audit --prod`. The three known warnings are listed with reasons in `.cargo/audit.toml`, cargo-audit's own config file; any other advisory, including a new warning, fails the job

A second job, `generated-projects`, runs on `push` to `main` only (the commit must exist on GitHub for `--rev`).

## Release workflow (`.github/workflows/release.yml`)

Trigger: push of a tag matching `v*`, plus `workflow_dispatch` for a dry run that uploads artifacts to the workflow run instead of a release.

Steps: version check; the CI checks above (reused through a reusable workflow `workflow_call` to avoid duplication); `tools/build-release.sh`, which builds each app with `--bundles app,dmg`, zips the `.app` with `ditto -c -k --keepParent`, copies the `.dmg`, and writes `SHA256SUMS`; then the release job runs `gh release create "$TAG" --draft --title ... --notes-file docs/release-notes.md` and upload. The notes file links to install instructions and known limitations.

Signing: if the `APPLE_CERTIFICATE` secret is set, the build step gets `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD` and `APPLE_TEAM_ID` in its `env:`, which the Tauri 2 bundler reads to sign and notarize (per the current Tauri 2 macOS signing guide). Otherwise the ad-hoc identity `-` from `tauri.conf.json` applies; a local run confirmed `codesign` reports `Signature=adhoc` with the hardened runtime. The Developer ID path is untested, since no Apple Developer account is configured. Secrets are read only through `env:` on the step that needs them and are never echoed.

## Docs

- `docs/install.md`: download, verify `SHA256SUMS`, open an ad-hoc signed app (Control-click → Open, or System Settings → Privacy & Security → Open Anyway), keychain prompt behavior, where local data lives, how to uninstall (app, data directory, keychain items).
- `docs/developer-guide.md`: add Deploy, the provisioning opt-in, the app-deploy module and its app types, and how CI checks a generated app.
- `docs/known-limitations.md`: one list gathered from all `docs/verification/` files and spec notes: items not verified live, deferred features, platform limits. Each item names its source.
- `README.md`: short install section linking to `docs/install.md`.
- Fix the starter skill nit: `templates/starter/.kiro/skills/ossiframe-module/SKILL.md` mentions only `moduleUi`; add `moduleUis`.

## Verification

Verified only by running: a CI run on `main` that passes; a `workflow_dispatch` dry run of the release workflow producing both apps and `SHA256SUMS`; downloading the dry-run artifacts on the user's Mac, checking the checksum, and opening each app (records the Gatekeeper and keychain behavior that S01 left open). Publishing a real release is the user's action and happens after the S04 and S05 essential live checks pass.

## Risks

- Standard GitHub-hosted runners are free for public repositories, but macOS minutes are billed on private ones. Confirm the repository's visibility before enabling the workflows.
- The S01 open item (keychain prompts with unsigned builds) may show repeated prompts with ad-hoc signing; record what happens.
- Third-party action SHAs must be looked up from each action's official repository at implementation time, not recalled.
