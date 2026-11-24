# {{product_name}}

A desktop app for managing remote Linux servers over SSH, built on [Ossiframe](https://github.com/shojibur/Ossiframe). Identifier: `{{identifier}}`.

This version supports read-only operations only.

## Requirements

- Rust (stable) and Cargo
- Node.js and pnpm
- Tauri 2 system prerequisites for your OS (see the official Tauri docs)

## Commands

```sh
pnpm install        # install frontend dependencies
pnpm tauri dev      # run the app in development
pnpm tauri build    # build a release bundle
```

Other checks: `pnpm typecheck`, `pnpm lint`, and `cargo test` in `src-tauri`.

## Add a module

1. Write a Rust crate under `modules/<id>/` that depends on `ossiframe-core` (see `.kiro/skills/ossiframe-module/SKILL.md`).
2. Add it as a dependency in `src-tauri/Cargo.toml`.
3. Register it with `.register(...)` in `src-tauri/src/lib.rs`.
4. Optional: add a page folder under `src/modules/<id>/` and list it in `src/modules.ts`.

Modules are trusted compile-time code. Module metadata is not a sandbox, and the frontend can only run operations that are registered in the app.

## Where data lives

- Server profiles, pinned host keys: SQLite file `app.sqlite3` in the app data directory that Tauri assigns for `{{identifier}}` (on macOS, `~/Library/Application Support/{{identifier}}/`).
- Credentials (passwords, key passphrases): only in the OS keychain, under the service name `{{identifier}}`. Nothing secret is stored in SQLite or logs.
- Removing a server from the app removes its keychain entry. Uninstalling the app does not touch the servers.

## Ossiframe dependency modes

The `ossiframe-*` crates in `src-tauri/Cargo.toml` come from one of two sources.

- **Standalone:** git dependencies on an exact public commit (`rev`). This builds without an Ossiframe checkout, and it works only if that commit is pushed to the public repository.
- **Local:** the app was generated with a local Ossiframe checkout. Plain `path =` dependencies point at that checkout. Use this for local testing only. Switch to a public `rev` before you share or release the app, because the paths only exist on your machine. A successful local build does not prove the standalone build works.

`@ossiframe/ui` and `@ossiframe/client` are copied into `packages/` in both modes and are not published to npm.
