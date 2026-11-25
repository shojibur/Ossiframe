# Developer quick-start

Generate your own desktop app for managing Linux servers over SSH, optionally add modules, and write your own module without changing Ossiframe Core.

Status: generated apps were verified against one real Ubuntu 24.04 server (read-only), in local mode and in standalone mode built from a published revision. Verification covers that one server and macOS Apple Silicon. See `docs/verification/s02-modules-and-generator.md` for what has been observed.

## Prerequisites

- Rust (stable, via rustup), Node.js and pnpm, git.
- Tauri 2 system prerequisites for your OS (see the official Tauri docs). The primary development platform is macOS Apple Silicon.
- A checkout of this repository. The generator is repository-local: you run it from the checkout.

## Generate an app

Run from the repository root. `<dir>` must be absent or empty.

```sh
# Minimal app: no modules, shows profile, endpoint, connection state and pinned fingerprint
cargo run -p ossiframe-cli -- new ../acme-min \
  --name "Acme Min" --identifier dev.example.acme-min

# With the linux-inventory module and the example ports module
cargo run -p ossiframe-cli -- new ../acme-ops \
  --name "Acme Ops" --identifier dev.example.acme-ops \
  --modules linux-inventory --with-example-module

# With the linux-audit module (linux-inventory is added automatically)
cargo run -p ossiframe-cli -- new ../acme-audit \
  --name "Acme Audit" --identifier dev.example.acme-audit \
  --modules linux-audit
```

Flags of `new`:

| Flag | Meaning |
|---|---|
| `--name` | Product name, 1 to 64 chars, starts with a letter (`A-Z a-z 0-9 space _ -`). |
| `--identifier` | Reverse-DNS id such as `dev.example.acme`. Lowercase segments. `dev.ossiframe.*` is refused unless `--allow-ossiframe-namespace`. |
| `--modules a,b` | Optional modules (comma separated). Today: `linux-inventory`, `linux-audit`, and `ubuntu-provisioning` with `--enable-provisioning` (see [Module catalog](#module-catalog)). Required dependencies are added automatically. Default is none. |
| `--enable-provisioning` | Required to include `ubuntu-provisioning`, and valid only together with it. Off by default. |
| `--with-example-module` | Copies the example `ports` module into `<dir>/modules/ports/`. |
| `--rev <40-hex>` | Ossiframe commit to pin. Default: `HEAD` of this checkout. |
| `--ossiframe-local <path>` | Use path dependencies on a local Ossiframe checkout. Cannot be combined with `--rev`. |

Invalid input stops with a one-line error and writes nothing.

## Module catalog

The `linux-inventory`, `linux-audit` and `ports` operations are read-only, take no arguments (pass `null`; anything else is rejected) and use predefined commands without `sudo`. `ubuntu-provisioning` is different: see below.

| Module | Depends on | Operations |
|---|---|---|
| `linux-inventory` | none | `server-info` (hostname, OS, kernel, CPU, memory, root disk), `services` (systemd services and their state), `listening-sockets` (TCP/UDP listeners with binding class and visible process), `software` (server software known to the dpkg database). Includes a UI page. |
| `linux-audit` | `linux-inventory` | Eight steps, in scan order: `resources`, `services`, `network`, `software`, `ssh`, `firewall`, `updates`, `certificates`. No UI page. |
| `ubuntu-provisioning` | `linux-inventory` | Installs a base stack (Git, NGINX, PHP 8.3 CLI and FPM, Composer, MySQL, optional Redis) from the Ubuntu 24.04 archive through preflight, plan and approval. It runs commands as root through `sudo -n`. It registers no `run_operation` operations; the app uses the `provision_*` commands. Has UI pages in `modules/ubuntu-provisioning/ui/`. Live behaviour is pending verification. |
| `ports` (example, `--with-example-module`) | none | The simple third-party example copied into your app. It is not part of the catalog; use it as the model for your own module. |

Selecting `linux-audit` adds `linux-inventory` to the generated app automatically, ordered before it.

### Provisioning is opt-in

`ubuntu-provisioning` runs commands as root on servers, so the generator refuses it unless you pass `--enable-provisioning`, and refuses the flag without the module:

```sh
cargo run -p ossiframe-cli -- new ../acme-deploy \
  --name "Acme Deploy" --identifier dev.example.acme-deploy \
  --modules ubuntu-provisioning --enable-provisioning
```

With the flag, the generated app enables the `ossiframe-desktop` `provisioning` feature, registers the module, calls `provisioning::setup` and lists the ten `provision_*` commands in its handler, build manifest and capability file. Without the flag (the default, including the plain starter) none of that is generated and the app stays read-only: it has no `provision_*` command, and `run_operation` rejects `Privilege::Root` commands. Doctor never includes provisioning. See [security.md](security.md) for the limits of these guards.

### Use `linux-audit` in your app

```sh
cargo run -p ossiframe-cli -- new ../acme-audit \
  --name "Acme Audit" --identifier dev.example.acme-audit --modules linux-audit
```

The generated app registers both modules. The module has no page, so you build your own UI. Call each step with `runOperation` from `@ossiframe/client`:

```ts
const step = await runOperation(serverId, "linux-audit", "resources", null);
```

Each step returns a step result with `step`, `observedAt`, step `evidence` and a list of `checks`. A check has `ruleId`, `title`, `category`, `outcome`, `severity` (only when the outcome is `finding`), `resource`, `evidence` (bounded lines), `explanation`, `recommendation` (advice text, never executed), `observedAt`, `limitations` and `inferred`.

Outcomes are `passed`, `finding`, `unavailable`, `incomplete`, `notApplicable` and `error`. Severities are `critical`, `high`, `medium`, `low` and `info`. Treat `unavailable`, `incomplete` and `error` as "not assessed", not as passed. There is no score. Run the steps one at a time in the order listed by `list_modules`, and combine the results in your UI.

## Local mode and standalone mode

The generated `src-tauri/Cargo.toml` gets Ossiframe from one of two sources.

- **Standalone (default).** Git dependencies on one exact commit of `https://github.com/shojibur/Ossiframe`. The commit is `--rev`, or `HEAD` of the checkout. With no `--rev`, the generator refuses a checkout with uncommitted changes. It does no network access, so the commit must already be pushed to the public repository, or `cargo build` in the generated app fails. A standalone app built purely from published revision 3ebdfc1 was verified to build, and to connect and show inventory and ports on a real server.
- **Local (`--ossiframe-local <path>`).** Plain `path` dependencies into your checkout. For local testing only. The generator prints a warning, and the paths exist only on your machine, so switch to a published `rev` before sharing the app. A working local build does not show that the standalone build works.

**`--rev` compatibility.** An explicit `--rev` must be compatible with the current generator, the frontend packages (`@ossiframe/ui`, `@ossiframe/client`) copied into the app, and the starter template. The generator copies those packages and the template from your checkout, but the Rust crates come from the pinned commit. A `--rev` from a different version can produce an app that does not build or behave correctly. Use a commit from the same checkout version unless you have verified the combination.

`@ossiframe/ui` and `@ossiframe/client` are copied into the app's `packages/`; they are not published to npm.

## Build and run

In the generated directory:

```sh
pnpm install
pnpm tauri dev      # development
pnpm tauri build    # release bundle
pnpm typecheck && pnpm lint
cargo test          # run inside src-tauri
```

Only connect to servers you are authorized to access.

## Add your own module

No change to Core is needed. The `ports` example in `examples/custom-module/` is the worked example, and the generated app contains `.kiro/skills/ossiframe-module/SKILL.md` with the full steps.

1. Create a crate under `modules/<id>/` that depends on `ossiframe-core` and implements the `Module` trait: a manifest (id, operations) and `run`, using structured commands (program, argv, timeout) through `ctx.exec`.
2. Validate operation arguments in Rust and bound and parse the remote output.
3. Test the parser and the operation with a fake executor.
4. Add the crate to `src-tauri/Cargo.toml` and register it with `.register(...)` in `src-tauri/src/lib.rs`.
5. Optional: add a page under `src/modules/<id>/` and list it in `src/modules.ts`. The page calls `runOperation(serverId, "<id>", "<operation>", args)` from `@ossiframe/client`.

## Security notes

- Modules are trusted compile-time code. A module manifest is not a sandbox.
- Modules built with the default starter are read-only: `run_operation` hands them an executor that rejects `Privilege::Root`. That guard checks the privilege field only; it is not a sandbox and does not stop a user-level command from changing a server. Mutating work belongs in the plan, approval and operation engine enabled by `--enable-provisioning`, not in `run_operation`.
- Secrets stay in the OS keychain. The frontend can run only operations registered in the app.

## Read more

- [architecture.md](architecture.md), [security.md](security.md), [roadmap.md](roadmap.md)
- [verification/s02-modules-and-generator.md](verification/s02-modules-and-generator.md)
- `templates/starter/README.md` (the README your generated app gets)
