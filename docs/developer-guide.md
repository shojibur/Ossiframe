# Developer quick-start

Generate your own desktop app for managing Linux servers over SSH, optionally add modules, and write your own module without changing Ossiframe Core.

Status: the generator is under verification. Generated apps have not yet been verified against a real server, and building from a published revision (standalone mode) has not been verified. See `docs/verification/s02-modules-and-generator.md` for what has been observed.

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
```

Flags of `new`:

| Flag | Meaning |
|---|---|
| `--name` | Product name, 1 to 64 chars, starts with a letter (`A-Z a-z 0-9 space _ -`). |
| `--identifier` | Reverse-DNS id such as `dev.example.acme`. Lowercase segments. `dev.ossiframe.*` is refused unless `--allow-ossiframe-namespace`. |
| `--modules a,b` | Optional modules (comma separated). Today: `linux-inventory`. Required dependencies are added automatically. Default is none. |
| `--with-example-module` | Copies the example `ports` module into `<dir>/modules/ports/`. |
| `--rev <40-hex>` | Ossiframe commit to pin. Default: `HEAD` of this checkout. |
| `--ossiframe-local <path>` | Use path dependencies on a local Ossiframe checkout. Cannot be combined with `--rev`. |

Invalid input stops with a one-line error and writes nothing.

## Local mode and standalone mode

The generated `src-tauri/Cargo.toml` gets Ossiframe from one of two sources.

- **Standalone (default).** Git dependencies on one exact commit of `https://github.com/shojibur/Ossiframe`. The commit is `--rev`, or `HEAD` of the checkout. With no `--rev`, the generator refuses a checkout with uncommitted changes. It does no network access, so the commit must already be pushed to the public repository, or `cargo build` in the generated app fails. Standalone builds from a published revision are not verified yet.
- **Local (`--ossiframe-local <path>`).** Plain `path` dependencies into your checkout. For local testing only. The generator prints a warning, and the paths exist only on your machine, so switch to a published `rev` before sharing the app. A working local build does not show that the standalone build works.

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
- All operations in this version are read-only. Before any mutating or privileged operation is added, explicit authorization and approval boundaries must be added (planned for S04).
- Secrets stay in the OS keychain. The frontend can run only operations registered in the app.

## Read more

- [architecture.md](architecture.md), [security.md](security.md), [roadmap.md](roadmap.md)
- [verification/s02-modules-and-generator.md](verification/s02-modules-and-generator.md)
- `templates/starter/README.md` (the README your generated app gets)
