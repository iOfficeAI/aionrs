# Skills

Skills are named prompt snippets that the agent can invoke on demand.  They
let you package reusable instructions, workflows, or tool sequences into a
single callable name.

## Overview

A skill is a Markdown file with a YAML front matter header.  When the agent
invokes a skill, it:

1. Resolves the skill by name from the loaded skill list
2. Substitutes variables (`$ARGUMENTS`, `$0`, `${CLAUDE_SKILL_DIR}`)
3. Expands any shell commands (`` !`cmd` `` syntax)
4. Returns the processed text as the skill's output

## Directory Structure

Skills are loaded from the following locations, in priority order (first match
wins for duplicate names):

| Priority | Path | Description |
|----------|------|-------------|
| 1 | `.aionrs/skills/` | Project-local skills (checked-in with the repo) |
| 2 | `<CONFIG_DIR>/aionrs/skills/` | User-global skills (see below) |
| 3 | `.aionrs/commands/` | Legacy flat `.md` files (backward compatibility) |

> **`<CONFIG_DIR>` by platform:**
> - **macOS:** `~/Library/Application Support/`
> - **Linux:** `~/.config/` (or `$XDG_CONFIG_HOME`)
> - **Windows:** `C:\Users\<USER>\AppData\Roaming\`
>
> Run `aionrs --skills-path` to see the actual paths on your machine.

Each skill is either a single `SKILL.md` file inside a named subdirectory, or
a flat `.md` file in a `commands/` directory:

```
.aionrs/skills/
├── deploy/
│   └── SKILL.md          # invoked as "deploy"
├── review-pr/
│   └── SKILL.md          # invoked as "review-pr"
```

## Writing a Skill

### Minimal skill

```markdown
---
name: greet
description: Print a greeting
---

Hello! How can I help you today?
```

### Full front matter reference

```yaml
---
# Required
name: skill-name          # Unique identifier; used to invoke the skill
description: One-line description shown in the skill list

# Optional — conditional activation
paths:
  - "src/**/*.rs"         # Skill is only active when the working path matches

# Optional — context overrides applied when the skill runs
model: claude-opus-4-5    # Override the active model
effort: high              # reasoning effort: low | medium | high
allowedTools:             # Restrict which tools the skill may use
  - Read
  - Grep

# Optional — permission rules
permissions:
  allow:
    - "Bash(git *)"
  deny:
    - "Bash(rm *)"

# Optional — hooks registered when the skill is active
hooks:
  PreToolUse:
    - "echo 'about to run a tool'"
  PostToolUse:
    - "echo 'tool finished'"
  Stop:
    - "echo 'session ended'"
---

Skill body goes here.
```

### Front matter fields

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | **Required.** Unique skill name. |
| `description` | string | **Required.** Shown in system prompt skill list. |
| `paths` | string[] | Glob patterns; skill is dormant unless the current path matches at least one. |
| `model` | string | Override active model for the duration of the skill. |
| `effort` | string | Override reasoning effort: `low`, `medium`, or `high`. |
| `allowedTools` | string[] | Restrict tools to this list when the skill is running. |
| `permissions.allow` | string[] | Tool patterns that are always allowed. |
| `permissions.deny` | string[] | Tool patterns that are always denied (highest priority). |
| `hooks.PreToolUse` | string[] | Shell commands run before each tool call. |
| `hooks.PostToolUse` | string[] | Shell commands run after each tool call. |
| `hooks.Stop` | string[] | Shell commands run when the session ends. |

## Variable Substitution

Inside the skill body, the following variables are replaced at runtime:

| Variable | Replaced with |
|----------|---------------|
| `$ARGUMENTS` | The full argument string passed to the skill invocation |
| `$0` | The skill name itself |
| `${CLAUDE_SKILL_DIR}` | Absolute path to the directory containing this skill's `SKILL.md` |

Example:

```markdown
---
name: run-tests
description: Run tests for a specific module
---

Run the test suite for module: $ARGUMENTS

Working directory: ${CLAUDE_SKILL_DIR}
```

## Shell Command Expansion

Lines containing `` !`cmd` `` execute `cmd` in a shell and substitute the
output inline:

```markdown
---
name: git-status
description: Show current git status
---

Current branch: !`git rev-parse --abbrev-ref HEAD`

Recent commits:
!`git log --oneline -5`
```

## Conditional Activation

Skills with a `paths:` field are **dormant** by default and become **active**
only when the current working path matches one of the glob patterns:

```yaml
---
name: rust-review
description: Rust-specific code review checklist
paths:
  - "**/*.rs"
  - "Cargo.toml"
---

When reviewing Rust code, check:
- No unwrap() in library code
- Error types implement std::error::Error
- Public APIs have doc comments
```

The skill appears in the system prompt only when a `.rs` file or `Cargo.toml`
is in scope.

## MCP Skills

Skills can also be loaded from MCP servers.  MCP-sourced skills behave
identically to local skills with one restriction: **shell command expansion
(`` !`cmd` ``) is disabled** for MCP skills to prevent arbitrary code
execution from untrusted sources.

Configure MCP skill sources in the config file — see [mcp.md](mcp.md) for
server configuration.

## Bundled Skills

A small set of skills is compiled into the binary.  Bundled skills:

- Are always available regardless of the filesystem skill directories
- Are **never truncated** by the prompt budget (they survive even when the
  skill list is shortened to stay within token limits)
- Cannot be overridden by a user skill with the same name

## Prompt Budget

When the total size of all skill descriptions exceeds the prompt budget, the
agent truncates the non-bundled skill list.  Bundled skills are always
preserved.  To stay within budget, keep skill descriptions concise.

## Troubleshooting

Use `--skills-path` to see which directories are being scanned and whether
they exist on disk:

```
$ aionrs --skills-path
User:    ~/Library/Application Support/aionrs/skills  (exists)
Project: /path/to/repo/.aionrs/skills                 (exists)
Legacy:  /path/to/repo/.aionrs/commands                (not found)
```
