# AGENTS.md

Project-specific rules and conventions for AI assistants and contributors.

## Build & Test

```bash
cargo build            # Build
cargo test             # Run all tests
cargo clippy           # Lint
cargo fmt --all        # Format (CI enforces this)
```

Before pushing, always run `cargo fmt --all` — CI will reject unformatted code.

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

// WRONG: hardcoded model name check
if request.model.starts_with("deepseek") {
    msg["reasoning_content"] = json!("");
}

// WRONG: hardcoded vendor workaround
if is_kimi_model {
    body["temperature"] = json!(1.0);
}
```

**Always do this:**

```rust
// CORRECT: read from compat config
let field = self.compat.max_tokens_field.as_deref().unwrap_or("max_tokens");
body[field] = json!(request.max_tokens);

// CORRECT: configurable content filtering
if let Some(patterns) = &self.compat.strip_patterns {
    for p in patterns { text = text.replace(p, ""); }
}
```

**Why:** Hardcoded quirks accumulate fast and turn the codebase into an
unmaintainable "workaround warehouse". Provider behaviors change, new providers
appear, and model-name checks go stale. Configuration-driven compat keeps the
code clean and gives users control.

**How it works:**

1. Each provider type has **default compat presets** (see `ProviderCompat::openai_defaults()`, etc.)
2. Users override any setting via `[providers.xxx.compat]` or `[profiles.xxx.compat]` in config
3. Provider code reads `self.compat.*` fields — never inspects URLs or model names

If you need a new compat behavior:
- Add an `Option<T>` field to `ProviderCompat`
- Set its default in the appropriate preset function
- Use it in provider code via `self.compat.field_name`
- Document it in the config reference

All providers implement the `LlmProvider` trait. The engine never sees
provider-specific details. Keep it that way:

- `LlmRequest` / `LlmEvent` / `Message` / `ContentBlock` are provider-neutral
- Format conversion happens inside each provider's `build_messages()` / `build_request_body()`

### Centralize Platform Differences

Any platform-specific behavior (paths, permissions, shell commands, line
endings, etc.) must be wrapped in a single centralized function. All call
sites use that function — never scatter raw platform detection across
multiple crates or modules.

When adding new platform-aware logic:
1. Create one function in the appropriate low-level crate
2. Replace all direct platform API calls with calls to that function
3. Tests and docs must use platform-neutral notation (e.g.
   `<config_dir>/aionrs`), never hardcoded platform-specific literals
   (e.g. `~/.config/aionrs`)

### No Duplicate Code Across Crates

If multiple crates need the same functionality, extract it to the
appropriate existing crate in the dependency graph — don't copy-paste
or reimplement. Choose the extraction target based on where it
semantically belongs and where it minimizes dependency changes.

Don't create a new crate just for one shared function.

### Crate Dependency Direction

This is a Cargo workspace under `crates/`. Dependencies flow downward:

- `aion-types` is the bottom layer — zero internal dependencies
- `aion-config`, `aion-protocol` depend only on `aion-types`
- Higher-level crates (`aion-agent`, `aion-cli`) may depend on lower ones
- Never introduce circular dependencies or upward references

When adding a new crate, check `cargo metadata` to verify it fits the
existing dependency graph before adding cross-crate imports.

### File Organization

- One file per logical unit within each crate
- Keep files under 800 lines; extract modules when approaching the limit
- Organize by domain responsibility, not by type

## Test Organization

| Location | What goes there |
|----------|----------------|
| Inline `#[cfg(test)]` in each `.rs` file | Unit tests for that module's internals |
| `crates/<crate>/tests/` | Integration tests for that crate |

Unit tests target internal logic and code paths.
Integration tests target functional requirements and public API —
write them from the spec, not from reading the implementation.

Every test must verify a meaningful behavior or edge case.
No trivial tests that just assert the happy path without checking boundaries,
error conditions, or non-obvious logic.

## Code Style

- Rust 2021 edition, stable toolchain
- `cargo clippy` must pass without warnings
- Comments in English, commit messages in English
