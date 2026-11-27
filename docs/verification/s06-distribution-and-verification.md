# S06 Distribution and Verification: verification

## 6.1 CI run

First run, [CI #1](https://github.com/shojibur/Ossiframe/actions/runs/45961331379) on `531ef6c`: failed.

- `cargo clippy` failed: `modules/app-deploy/data/known_hosts` was missing from the repository. The `.gitignore` rule `known_hosts` (meant for user SSH files) also matched this file, so S05's shipped Git host keys were never committed and the published S05 code (`c888a37` onward) did not build from a clean checkout.
- Fixed in `3f5dcca`: a narrow `.gitignore` exception for that one file, and the file committed. It holds only the providers' published public host keys; `ssh-keygen -lf` gave the same 12 fingerprints recorded when the keys were copied in S05.
- The other two jobs passed in this run: `shellcheck` (both scripts in `tools/`) and the generated-projects job.

Second run, [CI #2](https://github.com/shojibur/Ossiframe/actions/runs/45961850923) on `3f5dcca`: all three jobs passed.

| Job | Runner | Result |
|---|---|---|
| Checks: version check, fmt, clippy (with and without all features), tests, separate Doctor and Deploy checks, Doctor dependency guard, typecheck, lint, `cargo audit --deny warnings`, `pnpm audit` | `macos-26` (Apple Silicon) | Passed |
| `shellcheck` | `ubuntu-24.04` | Passed |
| Generated projects: a read-only app and a provisioning app generated with `--rev 3f5dcca` outside the checkout; provisioning-exposure checks; `cargo check`, `pnpm install`, `pnpm typecheck`, `pnpm build` for each | `macos-26` | Passed |

What the generated-projects job shows: standalone mode builds from a published revision on a clean machine, the read-only app exposes no `provision_*` command or `allow-provision-*` permission, and the provisioning app exposes all 10 of each. It runs `cargo check`, not a full `tauri build`, and does not run the apps.

Per-test counts are in the run logs and were not copied here.

## 6.2 Release dry run

Not run yet. It needs the user to start the Release workflow manually and to open the downloaded apps on their Mac.

Local evidence only, from the release script on the development Mac (not CI): `tools/build-release.sh` produced both `.dmg` and `.app.zip` files and `SHA256SUMS`; `shasum -a 256 -c SHA256SUMS` reported OK for all four; `codesign -dv` showed `Signature=adhoc` and flags `adhoc,runtime` for both apps. Gatekeeper behavior on a downloaded build and keychain behavior were not observed.

## 6.3 First release

Not started. Waits for the essential S04 and S05 live checks and the user's approval.
