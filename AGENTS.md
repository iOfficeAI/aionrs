# AGENTS.md

Rules and conventions for AI assistants and contributors working on aionrs.

## Overview

aionrs is a **multi-provider AI agent CLI** written in Rust. It connects to
LLM providers (Anthropic, OpenAI, AWS Bedrock, Google Vertex AI), orchestrates
built-in tools (Read, Write, Edit, Bash, Grep, Glob, Spawn), supports MCP
servers, skills, hooks, and long-term memory. It also exposes a JSON stream
protocol for host integration (e.g. Electron-based AionUI).

Tech stack: Rust 2021 edition, stable toolchain, Cargo workspace under `crates/`.

## Crate Map

Dependencies flow **downward** — never introduce circular or upward references.

| Layer | Crate | Responsibility |
|-------|-------|----------------|
| Bottom | `aion-types` | Shared provider-neutral data types (LLM, message, tool) — zero internal deps |
| Bottom | `aion-compact` | Context compression algorithms (folding, sanitization, tokenization) |
| Mid | `aion-config` | Configuration, ProviderCompat, auth, hooks, **cross-platform shell helpers** |
| Mid | `aion-protocol` | JSON stream protocol (events, commands, approval manager) for host integration |
| Mid | `aion-providers` | LLM provider implementations (Anthropic, OpenAI, Bedrock, Vertex) |
| Mid | `aion-tools` | Built-in agent tools (Read, Write, Edit, Bash, Grep, Glob, Spawn) |
| Mid | `aion-mcp` | MCP (Model Context Protocol) client |
| Mid | `aion-skills` | Skills system (prompt snippets, hooks, permissions, shell expansion) |
| Mid | `aion-memory` | Long-term cross-session memory (user prefs, feedback, project context) |
| Top | `aion-agent` | Agent engine, session management, orchestration |
| Top | `aion-cli` | CLI binary entry point |

When adding new functionality, place it in the **lowest crate where it
semantically belongs**. Don't create a new crate just for one shared function.
Run `cargo metadata` to verify dependency changes fit the graph.

## Build & Test

```bash
cargo build            # Build
cargo test             # Run all tests
cargo clippy           # Lint
cargo fmt --all        # Format (CI enforces this)
```

**Pushing code: always use `just push` instead of `git push`.**
It runs fmt → clippy → test before pushing, preventing CI failures.
Supports the same arguments as `git push` (e.g. `just push -u origin branch`).

## Code Style

- `cargo clippy` must pass without warnings
- `cargo fmt` must pass without diffs
- Comments in English, commit messages in English
- Error handling:
  - `thiserror` for public API error types (structured, matchable)
  - `anyhow` for internal/application-level error propagation
  - Never silently swallow errors; never `unwrap()` in production code
    unless the invariant is proven and commented

## File Organization

- Each module (`.rs` file) follows the **single responsibility principle** —
  one clear purpose per file
- Keep files under 1000 lines; extract sub-modules when approaching the limit
- Organize by domain responsibility, not by type

## Architecture Principles

### No Hardcoded Provider Quirks

**This is the single most important rule for this codebase.**

Handle provider differences through the **`ProviderCompat` configuration
layer**, not through hardcoded conditionals.

```rust
// WRONG: hardcoded provider detection
if self.base_url.contains("api.openai.com") {
    body["max_completion_tokens"] = json!(max_tokens);
}

// CORRECT: read from compat config
let field = self.compat.max_tokens_field.as_deref().unwrap_or("max_tokens");
body[field] = json!(request.max_tokens);
```

If you need a new compat behavior:
1. Add an `Option<T>` field to `ProviderCompat`
2. Set its default in the appropriate preset function (e.g. `openai_defaults()`)
3. Use it in provider code via `self.compat.field_name`

All providers implement the `LlmProvider` trait. The engine sees only
provider-neutral types (`LlmRequest`, `LlmEvent`, `Message`, `ContentBlock`).
Format conversion happens inside each provider's `build_messages()` /
`build_request_body()`.

> **Deep dive:** see [docs/providers.md](docs/providers.md) for provider
> setup, auth, aliases, and profile inheritance.

### Centralize Platform Differences

Any platform-specific behavior (paths, permissions, shell commands, line
endings, etc.) must be wrapped in a single centralized function. All call
sites use that function — never scatter raw platform detection across
multiple crates or modules. See [Cross-Platform](#cross-platform) for
concrete rules.

### No Duplicate Code Across Crates

If multiple crates need the same functionality, extract it to the
appropriate existing crate in the dependency graph — don't copy-paste
or reimplement. Choose the extraction target based on where it
semantically belongs and where it minimizes dependency changes.

## Cross-Platform

CI runs on macOS, Linux, **and Windows**. Local dev can only test the
current platform's `#[cfg(...)]` code — other platform branches are
verified by CI alone.

### Paths

- Never hardcode platform paths (`/tmp/...`, `C:\...`) in production code.
  Use `Path::join()`, `dirs::config_dir()`, `tempfile::tempdir()`, etc.
- In tests, hardcoded Unix paths (`Path::new("/foo/...")`) are fine for
  pure string operations (join, display) or nonexistent-path error handling.
  Only add `#[cfg(unix)]` / `#[cfg(windows)]` variants when the path is
  passed to `is_absolute()`, `validate_memory_path()`, or similar
  platform-sensitive checks.
- Use `std::path::Component::Normal` (not byte length) when checking
  path depth — prefix/root components differ across platforms.

### Shell Execution

- All shell invocations must go through `aion_config::shell` module
  (`shell_command()` or `shell_command_builder()`).
- Never call `Command::new("sh")`, `Command::new("bash")`, or
  `Command::new("cmd")` directly — these are platform-specific.
- External CLI tools that differ across platforms (e.g. `grep` vs
  `findstr`) must use `cfg!(windows)` branches or equivalent
  platform-aware selection.

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

## Documentation

Key references in `docs/` (don't duplicate their content here):

| Document | Covers |
|----------|--------|
| [getting-started.md](docs/getting-started.md) | Installation, CLI usage, config format and cascading precedence |
| [providers.md](docs/providers.md) | Provider setup, auth, ProviderCompat, custom aliases, profiles |
| [tools.md](docs/tools.md) | Built-in tool reference and execution flow |
| [skills.md](docs/skills.md) | Writing skills, front matter, shell expansion, conditional activation |
| [mcp.md](docs/mcp.md) | MCP server integration, transport types, deferred loading |
| [advanced.md](docs/advanced.md) | Sub-agents, hooks, memory, plan mode, context compression |
| [json-stream-protocol.md](docs/json-stream-protocol.md) | JSON Lines protocol spec for host integration (e.g. AionUI) |
| [troubleshooting.md](docs/troubleshooting.md) | Common errors and solutions |
