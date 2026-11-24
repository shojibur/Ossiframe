# ADR-0007: Independent project generation and distribution

Status: Accepted (2026-11-24), with open questions.

## Decision
- Generator: a repository-local Rust CLI in `tools/ossiframe-cli`, run with `cargo run -p ossiframe-cli -- create <name>`. It reuses the module registry's dependency resolver.
- Generated projects contain only selected modules and their dependencies, plus Kiro steering and module-development guidance.
- Generated projects must build outside the Ossiframe repository, without private paths or unpublished packages.
- Desktop apps are distributed through GitHub Releases, built by GitHub Actions, with checksums. macOS Apple Silicon first.
- Publishing to crates.io or npm is optional and later.

## Open questions (resolve in S02)
- How generated projects reference Core. Proposed: Rust crates as Git dependencies pinned to a release tag; frontend packages copied into the generated project. Confirm this builds cleanly from a fresh directory.
- Generator templates: embedded in the binary or read from `templates/starter/`.

## Open questions (resolve in S06)
- macOS signing and notarization.
