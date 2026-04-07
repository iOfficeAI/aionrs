# AGENTS.md

Project-specific instructions for AI assistants and contributors.

## Build & Test

```bash
cargo build            # Build
cargo test             # Run all tests
cargo clippy           # Lint
```

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

### Provider Abstraction

All providers implement the `LlmProvider` trait. The engine never sees
provider-specific details. Keep it that way:

- `LlmRequest` / `LlmEvent` / `Message` / `ContentBlock` are provider-neutral
- Format conversion happens inside each provider's `build_messages()` / `build_request_body()`
- Shared logic (Anthropic/Bedrock/Vertex SSE parsing) lives in `anthropic_shared.rs`

### File Organization

- `src/provider/` — One file per provider + `compat.rs` + `anthropic_shared.rs`
- `src/tools/` — One file per tool
- `src/types/` — Shared data types (provider-neutral)
- `src/mcp/` — MCP client implementation
- `src/protocol/` — JSON stream protocol for host integration

## Skills Module

`src/skills/` implements the Skill system — user-defined prompt snippets that
the agent can invoke by name.  The module is split into focused submodules:

| Submodule | Responsibility |
|-----------|----------------|
| `types` | Core data types: `SkillDefinition`, `SkillSource`, `SkillPermissions`, etc. |
| `frontmatter` | Parse YAML front matter from SKILL.md files |
| `loader` | Discover and load skills from the filesystem |
| `paths` | Platform skill directory resolution (`~/.config/aionrs/skills/`, `.aionrs/skills/`, legacy paths) |
| `discovery` | Runtime directory lookup keyed on the active working directory |
| `executor` | Execute a skill: variable substitution + optional shell command expansion |
| `substitution` | `$ARGUMENTS`, `$0`, `${CLAUDE_SKILL_DIR}` replacement logic |
| `shell` | Shell command execution for `` !`cmd` `` syntax in skill bodies |
| `permissions` | Permission chain evaluation (deny → allow → safe-properties → ask) |
| `conditional` | Conditional activation: `paths:` glob matching |
| `context_modifier` | Apply skill-specified `model`/`effort`/`allowedTools` overrides |
| `bundled` | Built-in skills compiled into the binary (never truncated by budget) |
| `mcp` | Load skills from MCP servers; shell commands disabled for MCP skills |
| `hooks` | Parse and classify `PreToolUse`/`PostToolUse`/`Stop` hooks from skill front matter |
| `prompt` | Render the skill list for injection into the system prompt; budget control |
| `watcher` | Watch skill directories for file changes; debounced version counter |

### Development conventions

**Adding a new front matter field**

1. Add the field to the appropriate struct in `types.rs`
2. Parse it in `frontmatter.rs` (`parse_frontmatter`)
3. Add a unit test in `frontmatter.rs` inline tests

**Adding a new built-in (bundled) skill**

1. Create a `SKILL.md` file under `src/skills/bundled/`
2. Register it in `bundled.rs` — the `BUNDLED_SKILLS` static slice
3. Bundled skills are never truncated by prompt budget; use sparingly

**Extending the permission system**

- Permission priority is fixed: deny > allow > safe-properties > ask
- Never reorder; tests in `permissions.rs` and `permissions_supplemental_tests.rs`
  encode the expected chain

**Filesystem watcher**

- `SkillWatcher` uses `notify` (cross-platform) with a 300 ms debounce
- `should_ignore` filters spurious events; update it (with a comment) when
  adding new filter rules — do not add `#[cfg(target_os)]` conditionals

### Test organization

| Location | What goes there |
|----------|----------------|
| Inline `#[cfg(test)]` in each `.rs` file | White-box unit tests for that module's internals |
| `src/skills/watcher_tests.rs` | Black-box tests for `SkillWatcher` (filesystem events) |
| `src/skills/permissions_supplemental_tests.rs` | Additional permission chain edge cases |
| `src/skills/bundled_supplemental_tests.rs` | Bundled skill edge cases |
| `src/skills/integration_tests.rs` | Cross-module end-to-end tests |

## Code Style

- Rust 2021 edition, stable toolchain
- `cargo clippy` must pass without warnings
- Comments in English, commit messages in English
- Keep files under 800 lines; extract modules when approaching the limit
