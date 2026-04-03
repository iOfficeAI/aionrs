# AGENTS.md

Project-specific instructions for AI assistants and contributors.

## Workspace Layout

```
aionrs/
├── crates/
│   ├── aion-core/   # Core engine, providers, tools, MCP, config, session, auth
│   └── aion-cli/    # CLI binary (main.rs) — thin wrapper over aion-core
├── workspace-hack/  # Managed by cargo-hakari for build-time deduplication
├── Cargo.toml       # Workspace root
├── justfile         # Dev task runner (use: vx just <recipe>)
└── .github/
    └── workflows/
        ├── ci.yml              # CI checks (fmt, clippy, tests, audit)
        ├── release.yml         # Multi-platform binary builds
        └── release-please.yml  # Automated versioning & changelog
```

## Build & Test

```bash
vx just build          # Build workspace
vx just test           # Run all tests
vx just lint           # cargo clippy (warnings = errors)
vx just fmt            # cargo fmt
vx just check-all      # Run all CI checks locally

# Or call cargo directly
vx cargo build --workspace
vx cargo test --workspace
vx cargo clippy --workspace --all-targets -- -D warnings
```

## workspace-hack (cargo-hakari)

The `workspace-hack` crate deduplicates feature compilation across the workspace,
significantly speeding up incremental builds.

```bash
vx just hakari-generate   # Regenerate after adding/changing dependencies
vx just hakari-verify     # Verify it is up-to-date (run in CI)
```

If you add or change a dependency in any crate, run `cargo hakari generate` before
committing. CI runs `cargo hakari verify` and will fail if the file is stale.

## Release Process (release-please)

Versioning is fully automated via [release-please](https://github.com/googleapis/release-please):

1. Write commits using [Conventional Commits](https://www.conventionalcommits.org/):
   - `feat: ...` → minor bump
   - `fix: ...` → patch bump
   - `feat!: ...` or `BREAKING CHANGE` in footer → major bump
2. `release-please` opens a PR titled `chore: release vX.Y.Z`.
3. Merge the PR → a tag is pushed → `release.yml` builds binaries for all platforms.

**Never manually bump versions in `Cargo.toml`** — let release-please do it.

## Architecture Principles

### No Hardcoded Provider Quirks

**This is the single most important rule for this codebase.**

Different LLM providers have different API quirks (field names, message format
requirements, schema restrictions, etc.). We handle these differences through
the **`ProviderCompat` configuration layer**, not through hardcoded conditionals.

**Never do this:**

```rust
// WRONG: hardcoded provider detection
if self.base_url.contains("api.openai.com") {
    body["max_completion_tokens"] = json!(max_tokens);
} else {
    body["max_tokens"] = json!(max_tokens);
}
```

**Always do this:**

```rust
// CORRECT: read from compat config
let field = self.compat.max_tokens_field.as_deref().unwrap_or("max_tokens");
body[field] = json!(request.max_tokens);
```

**How it works:**

1. Each provider type has **default compat presets** (see `ProviderCompat::openai_defaults()`, etc.)
2. Users override any setting via `[providers.xxx.compat]` or `[profiles.xxx.compat]` in config
3. Provider code reads `self.compat.*` fields — never inspects URLs or model names

If you need a new compat behavior:
- Add an `Option<T>` field to `ProviderCompat` in `crates/aion-core/src/provider/compat.rs`
- Set its default in the appropriate preset function
- Use it in provider code via `self.compat.field_name`

### Provider Abstraction

All providers implement the `LlmProvider` trait (in `crates/aion-core/src/provider/mod.rs`).

- `LlmRequest` / `LlmEvent` / `Message` / `ContentBlock` are provider-neutral
- Format conversion happens inside each provider's `build_messages()` / `build_request_body()`
- Shared logic (Anthropic/Bedrock/Vertex SSE parsing) lives in `anthropic_shared.rs`

### File Organization

- `crates/aion-core/src/provider/` — One file per provider + `compat.rs` + `anthropic_shared.rs`
- `crates/aion-core/src/tools/`    — One file per tool
- `crates/aion-core/src/types/`    — Shared data types (provider-neutral)
- `crates/aion-core/src/mcp/`      — MCP client implementation
- `crates/aion-core/src/protocol/` — JSON stream protocol for host integration
- `crates/aion-cli/src/main.rs`    — CLI entry point

## Code Style

- Rust 2021 edition (workspace default), stable toolchain (`rust-toolchain.toml`)
- `cargo clippy` must pass without warnings (`-D warnings`)
- Tests go in `crates/<name>/tests/` (integration) or `src/**/*.rs` `#[cfg(test)]` (unit)
- Use `rstest` as the testing framework for parameterized tests
- Comments in English, commit messages in English (Conventional Commits format)
- Keep files under 800 lines; extract modules when approaching the limit
