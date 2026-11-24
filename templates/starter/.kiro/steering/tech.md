---
inclusion: always
---
# Technology

- Desktop: Tauri 2. Backend: Rust. Frontend: React + TypeScript, built with Vite.
- Ossiframe crates (`ossiframe-core`, `ossiframe-desktop`, and any selected modules) are Cargo dependencies in `src-tauri/Cargo.toml`. They come from a public git `rev`, or from local `path` dependencies when the app was generated in local mode.
- `@ossiframe/ui` and `@ossiframe/client` are vendored in `packages/` and used as `workspace:*` dependencies.
- Package manager: pnpm. Rust: Cargo.
- SSH, host key checks, SQLite and keychain access are done by `ossiframe-desktop` and `ossiframe-ssh`. Do not write SSH protocol or cryptography code in this app.
- Add a dependency only when it is needed. It must be maintained and permissively licensed.
- Check the current official Tauri 2 docs instead of relying on memory.

Checks before you finish a change: `cargo fmt --check`, `cargo clippy -- -D warnings` and `cargo test` in `src-tauri`, then `pnpm typecheck`, `pnpm lint` and `pnpm build`.
