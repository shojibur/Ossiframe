---
name: ossiframe-module
description: Write and register a read-only Ossiframe module (a Rust crate with an optional React page) for this app. Use when adding a server capability such as a new discovery or diagnostic operation.
---

# Writing an Ossiframe module

A module is trusted, compile-time Rust code. It depends only on `ossiframe-core`. It is registered explicitly in `src-tauri/src/lib.rs`. There are no dynamic plugins.

## Rules

- Read-only operations only. Do not write files, restart services, install packages or use `sudo`. This version has no mutating or privileged operation support.
- The manifest is metadata, not a sandbox. It grants and restricts nothing. Safety comes from the commands your code builds.
- Build every command with `CommandSpec::new(program, &[args], timeout)`. The program and its arguments are fixed. Never pass user text into a shell string.
- Validate `args` in Rust (typed struct with `#[serde(deny_unknown_fields)]`, or a typed enum). Reject anything else with `OperationError::InvalidArguments`. Never put argument text into argv.
- Treat command output as untrusted. Bound its size, parse it, skip lines you cannot parse, and strip control characters.
- Errors use `OperationError`. `Failed` carries a short message, never raw command output.
- Never log output or secrets. No secrets in args.

## Steps

1. Create `modules/<id>/` with a `Cargo.toml` that depends on `ossiframe-core`, `serde`, `serde_json` and `async-trait`. Use `tokio` as a dev dependency for tests.
2. Implement the `Module` trait:

```rust
use async_trait::async_trait;
use ossiframe_core::{
    CommandSpec, Module, ModuleId, ModuleManifest, OperationContext, OperationError,
    OperationInfo,
};
use std::time::Duration;

const OPERATIONS: &[OperationInfo] = &[OperationInfo {
    id: "uptime",
    description: "Seconds since boot",
}];

pub struct Uptime;

#[async_trait]
impl Module for Uptime {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            id: ModuleId::parse("uptime").expect("valid module id"),
            name: "Uptime".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "Read-only uptime".to_owned(),
            dependencies: Vec::new(), // ids of other modules this one needs
        }
    }

    fn operations(&self) -> &'static [OperationInfo] {
        OPERATIONS
    }

    async fn run(
        &self,
        operation: &str,
        args: serde_json::Value,
        ctx: &OperationContext<'_>,
    ) -> Result<serde_json::Value, OperationError> {
        if !args.is_null() {
            return Err(OperationError::InvalidArguments("takes no arguments".into()));
        }
        // Only called with an id listed in operations().
        debug_assert_eq!(operation, "uptime");
        let spec = CommandSpec::new("cat", &["/proc/uptime"], Duration::from_secs(10))
            .map_err(|e| OperationError::Failed(e.to_string()))?;
        let out = ctx.exec.run(&spec, ctx.cancel).await?; // ExecError converts automatically
        let seconds: f64 = out
            .stdout
            .split_whitespace()
            .next()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| OperationError::Failed("unexpected output".into()))?;
        Ok(serde_json::json!({ "observedAt": ctx.observed_at, "seconds": seconds }))
    }
}
```

   Check the exit status (`out.exit.success()`) when the command can fail. `ExecResult` also has `truncated`.
3. Test the parser and the module with a fake `Executor` (implement the `Executor` trait and return a canned `ExecResult`). Test malformed and oversized input.
4. Add the dependency in `src-tauri/Cargo.toml`: `ossiframe-mod-<id> = { path = "../modules/<id>" }`. The module crate depends on `ossiframe-core` with the same source as the app.
5. Register it in `src-tauri/src/lib.rs`, in the `Registry::builder()` chain: `.register(my_crate::Uptime)`. A duplicate id, a missing dependency or a cycle makes the app fail at startup with a readable message.
6. Optional page. Put `.tsx` files in `src/modules/<id>/`. The entry `index.ts` exports `moduleUi: ModuleUi` (`id` equal to the Rust module id, `title`, `Page`). It may also export `hooks = { onStale, onConnected, onRemoved }`, each taking a server id. The page calls `api.runOperation(serverId, "<id>", "<operation>", args)` from `@ossiframe/client` and renders results as text with components from `@ossiframe/ui`.
7. Add the page in `src/modules.ts`: import the folder and add it to the `entries` list. A backend-only module needs no UI.
8. Verify: `cargo test` and `cargo clippy -- -D warnings` in `src-tauri`, then `pnpm typecheck`, `pnpm lint`, `pnpm tauri dev`. A module is not done until it works against a real, authorized server.
