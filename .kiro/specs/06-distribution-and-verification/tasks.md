# Implementation Plan: Distribution and Verification (S06)

## Overview

Order: signing config and version check, CI workflow, generated project check, release workflow, docs, then verification. No Rust crate and no runtime feature is added. Task 6.2 needs the user's Mac, and task 6.3 needs the user's approval and the S04 and S05 live checks.

## Tasks

- [x] 1. Signing config and version consistency
  - Set `bundle.macOS.signingIdentity` to `"-"` in both `tauri.conf.json` files, after checking the current Tauri 2 config schema.
  - Add `tools/check-version.sh`, checking that the workspace `Cargo.toml`, both `tauri.conf.json` files and both app `package.json` files share one version, and that a given tag `v<version>` matches.
  - _Requirements: 3.1, 4.1_

- [x] 2. CI workflow
  - `.github/workflows/ci.yml` as in the design, on `macos-26`, workflow-level `contents: read`, exposed as a reusable workflow (`workflow_call`) for the release workflow.
  - Look up each action's current release and full commit SHA from its official repository at implementation time; record the version in a comment. Do not recall SHAs.
  - `.cargo/audit.toml` with the three known `cargo audit` warnings and a reason each; CI runs `cargo audit --deny warnings`, so anything not listed fails.
  - Doctor dependency-tree check for `app-deploy` and `ubuntu-provisioning`.
  - _Requirements: 1.1, 1.2, 1.3, 1.4, 1.5_

- [x] 3. Generated project check
  - A `generated-projects` job on push to `main`: generate a read-only app and a provisioning app with `--rev ${{ github.sha }}` under `$RUNNER_TEMP`, build and typecheck both, and grep the read-only app for `provision_` and `allow-provision-`.
  - _Requirements: 2.1, 2.2, 2.3_

- [x] 4. Release workflow
  - `.github/workflows/release.yml`: tag and manual triggers, version check, CI via `workflow_call`, build both apps, zip `.app` with `ditto`, `SHA256SUMS`, then a Draft_Release with `gh` on a tag, or workflow artifacts on a manual run. `contents: write` on the release job only.
  - Optional Developer ID signing and notarization when the `APPLE_*` secrets exist, following the current Tauri 2 macOS signing guide; secrets only in `env:` of the steps that use them.
  - `docs/release-notes.md` template linking install instructions and known limitations.
  - _Requirements: 3.2, 3.3, 3.4, 3.5, 3.6, 4.2, 4.3_

- [x] 5. Docs
  - `docs/install.md`, `docs/known-limitations.md` (gathered from `docs/verification/` and spec notes, each item with its source), developer guide updates, README install section, starter skill `moduleUis` fix.
  - Write keychain and Gatekeeper behavior in `install.md` only after task 6.2 observes it; until then mark it "not yet observed".
  - _Requirements: 5.1, 5.2, 5.3, 5.4, 5.5_

- [ ] 6. Verification
  - [ ] 6.1 CI run
    - After the user approves pushing the workflows, confirm a CI run on `main` passes, including the generated project job. Fix real failures only. Record the run link in `docs/verification/s06-distribution-and-verification.md`.
    - _Requirements: 6.1_
  - [ ] 6.2 Release dry run on the user's Mac
    - The user starts the release workflow manually. The user downloads the artifacts, checks `SHA256SUMS`, opens both apps and reports Gatekeeper and keychain behavior. Record it, update `install.md`, and close the S01 keychain item if observed.
    - _Requirements: 3.5, 6.2_
  - [ ] 6.3 STOP: first release
    - Ask the user to confirm that the essential S04 and S05 live checks have passed and to approve tagging. Create no tag without that approval. After tagging, confirm the draft release has both apps and `SHA256SUMS`; the user publishes it.
    - _Requirements: 3.4, 6.3_

- [ ] 7. Roadmap
  - Update `docs/roadmap.md` with S06 status and resolve ADR-0007's signing open question with the decision taken.
  - _Requirements: 5.5_

## Notes

- Do not commit, push, tag or publish without explicit user approval.
- No new Rust crate, no new npm package, and no third-party action beyond those named in the design.

## Task Dependency Graph

```json
{
  "waves": [
    { "id": 0, "tasks": ["1"] },
    { "id": 1, "tasks": ["2"] },
    { "id": 2, "tasks": ["3", "4", "5"] },
    { "id": 3, "tasks": ["6.1"] },
    { "id": 4, "tasks": ["6.2"] },
    { "id": 5, "tasks": ["6.3", "7"] }
  ]
}
```
