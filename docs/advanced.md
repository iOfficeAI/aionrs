# Advanced Features

## Sub-Agent Spawning

The LLM can use the Spawn tool to create independent sub-agents that run tasks in parallel. Each sub-agent has its own conversation context and full tool set, but shares the parent agent's LLM provider (connection pool reuse).

### Use Cases

- "Search these 3 files simultaneously and summarize each"
- "Run tests and lint in parallel"
- "Search for X in the codebase while reading Y"

### Limits

| Setting | Default | Description |
|---------|---------|-------------|
| Max parallel sub-agents | 5 | Prevents resource exhaustion |
| Sub-agent max turns | 10 | Per sub-agent conversation turn limit |
| Sub-agent max tokens | 4096 | Per sub-agent response token limit |

### Behavior

- Sub-agents auto-approve all tool calls (no confirmation prompts)
- Sub-agents do not save sessions
- Sub-agents run silently (no stdout output)
- All results are merged and returned to the parent agent

---

## Hook System

Event-driven hooks execute shell commands at specific points in the tool lifecycle, enabling auto-formatting, linting, auditing, and more.

### Hook Types

| Type | Trigger | Behavior |
|------|---------|----------|
| `pre_tool_use` | Before tool execution | Non-zero exit blocks the tool |
| `post_tool_use` | After tool execution | Non-blocking; errors are logged |
| `stop` | When agent session ends | Non-blocking |

### Configuration

```toml
# Auto-format Rust files after modification
[[hooks.post_tool_use]]
name = "rustfmt"
tool_match = ["Write", "Edit"]
file_match = ["*.rs"]
command = "rustfmt ${TOOL_INPUT_FILE_PATH}"

# Auto-format TypeScript files after modification
[[hooks.post_tool_use]]
name = "prettier"
tool_match = ["Write", "Edit"]
file_match = ["*.ts", "*.tsx"]
command = "npx prettier --write ${TOOL_INPUT_FILE_PATH}"

# Audit Bash commands
[[hooks.post_tool_use]]
name = "audit-log"
tool_match = ["Bash"]
command = "echo \"$(date): ${TOOL_INPUT_COMMAND}\" >> .aionrs/audit.log"

# Run lint on session end
[[hooks.stop]]
name = "final-lint"
command = "cargo clippy --quiet 2>&1 | tail -5"
```

### Environment Variables

Hook commands can reference these variables via `${VAR}` syntax:

| Variable | Description |
|----------|-------------|
| `TOOL_NAME` | Tool name |
| `TOOL_INPUT` | Full tool input JSON |
| `TOOL_INPUT_FILE_PATH` | File path (if the tool has a file_path parameter) |
| `TOOL_INPUT_COMMAND` | Command (if the tool has a command parameter) |
| `TOOL_INPUT_PATTERN` | Search pattern (if the tool has a pattern parameter) |
| `TOOL_OUTPUT` | Tool output (post_tool_use only) |

### Matching Rules

- `tool_match`: glob patterns matching tool names; empty = match all
- `file_match`: glob patterns matching file paths; empty = match all
- Default timeout: 30 seconds, configurable via `timeout_ms`

---

## Prompt Caching (Anthropic)

Prompt caching stores system prompts and tool definitions on Anthropic's servers, so subsequent requests only process the changed parts.

- **First request**: full input token cost + 25% write premium
- **Subsequent requests**: cached portion costs only 10%
- **Cache TTL**: 5 minutes (auto-renewed on each hit)

### Configuration

```toml
[providers.anthropic]
api_key = "sk-ant-xxx"
prompt_caching = true   # default true (Anthropic only)
```

### Token Stats

With caching enabled, stats show cache data:

```
[turns: 3 | tokens: 100 in (5000 cached) / 200 out | cache: 5000 created, 5000 read]
```

---

## VCR Recording & Replay

Record real API interactions and replay them in tests — no API key or network needed.

### Usage

```bash
# Record mode
VCR_MODE=record VCR_CASSETTE=tests/cassettes/my_test.json \
  aionrs -k sk-ant-xxx "Read Cargo.toml"

# Replay mode (in tests)
VCR_MODE=replay VCR_CASSETTE=tests/cassettes/my_test.json \
  aionrs "Read Cargo.toml"
```

### Features

- Auto-sanitization: sensitive headers (api-key, auth, token) are replaced with `[REDACTED]` during recording
- JSON-formatted cassette files, editable by hand
- Supports recording/replay of SSE streaming responses

---

## AGENTS.md Auto-Loading

If an `AGENTS.md` file exists in the current working directory, its contents are automatically injected into the system prompt. Use this for:

- Project-specific coding standards
- Architecture descriptions
- Special working constraints

---

## Memory System

Persistent, file-based memory that allows the agent to retain project-specific knowledge across sessions. Memory is automatically loaded into the system prompt at conversation start.

### Memory Types

| Type | Purpose |
|------|---------|
| `user` | User's role, goals, preferences, knowledge |
| `feedback` | Corrections and confirmations on work approach |
| `project` | Ongoing work context not derivable from code/git |
| `reference` | Pointers to external systems and resources |

### Storage

Memory files live in a per-project directory under the global config:

```
<config_dir>/aionrs/projects/<sanitized-project-path>/memory/
├── MEMORY.md              # Index (auto-loaded into prompt, max 200 lines)
├── user_role.md
├── feedback_testing.md
└── project_auth_rewrite.md
```

Each memory file uses YAML frontmatter:

```markdown
---
name: auth rewrite
description: Auth middleware rewrite driven by compliance
type: project
---

Auth middleware rewrite is driven by legal/compliance requirements.
```

### Configuration

Memory is enabled by default with no configuration required. The memory directory is auto-resolved from the current working directory.

Override the base directory via environment variable:

```bash
export AIONRS_MEMORY_DIR=/custom/path
```

### How It Works

1. Agent starts → memory directory resolved from project path
2. `MEMORY.md` index loaded into system prompt (truncated at 200 lines / 25 KB)
3. Agent reads/writes memory files using standard Read/Write tools
4. Agent maintains the `MEMORY.md` index as memories are added or removed

---

## Plan Mode

A read-only exploration mode where the agent focuses on understanding the codebase and producing an implementation plan before making any changes.

### How It Works

1. Agent calls `EnterPlanMode` → tool access restricted to read-only (Read, Grep, Glob)
2. Agent explores code, designs approach, writes a structured plan in its response
3. Agent calls `ExitPlanMode` → full tool access restored, plan optionally saved to disk

### Configuration

```toml
[plan]
enabled = true                    # Register Plan Mode tools (default: true)
plan_directory = ".aionrs/plans"  # Where plan files are saved
```

### Workflow Phases

When in plan mode, the agent follows a structured 4-phase process:

1. **Understand** — Explore the codebase with read-only tools
2. **Design** — Identify files to modify, code to reuse
3. **Write the plan** — Compose a clear, actionable implementation plan
4. **Submit** — Call `ExitPlanMode` to restore full tool access

---

## Context Compression

A three-tier automatic compaction strategy that prevents context window overflow during long conversations.

### Tiers

| Tier | Trigger | Method | LLM Call |
|------|---------|--------|----------|
| **Microcompact** | Tool result count exceeds threshold or time gap | Clears old tool result content, keeping the N most recent | No |
| **Autocompact** | Input tokens approach context limit | LLM summarizes the conversation | Yes |
| **Emergency** | Input tokens near absolute limit | Blocks further API calls, asks user to start fresh | No |

### How It Works

- **Microcompact** runs automatically: replaces old Read/Bash/Grep/Glob/Write/Edit results with `[Tool result cleared]`, keeping the 5 most recent results intact. Triggered by count (>10 compactable results) or time (>1 hour since last assistant message).

- **Autocompact** triggers when input tokens reach `context_window - output_reserve - autocompact_buffer` (default: 200,000 - 20,000 - 13,000 = 167,000 tokens). The agent calls the LLM to produce a conversation summary, then replaces history with a compact boundary marker. A circuit breaker stops retrying after 3 consecutive failures.

- **Emergency** is the last safety net at `context_window - emergency_buffer` (default: 197,000 tokens). Always active regardless of config. Blocks API calls and prompts the user to compact or start a new conversation.

### Configuration

```toml
[compact]
enabled = true              # Enable compaction system (default: true)
context_window = 200000     # Context window in tokens
output_reserve = 20000      # Reserved for output generation
autocompact_buffer = 13000  # Buffer before autocompact triggers
emergency_buffer = 3000     # Buffer before emergency block
max_failures = 3            # Circuit breaker threshold
micro_keep_recent = 5       # Keep N most recent tool results
```

---

## File State Cache

An LRU cache that tracks files the agent has recently accessed, enabling read deduplication and automatic cache updates on writes.

- **Read dedup**: When the agent reads a file it has already seen (and the file hasn't changed), the cache provides the content without re-reading from disk.
- **Write/Edit auto-update**: After Write or Edit operations, the cache is updated immediately with the new content.
- **Dual eviction**: Entries are evicted when either the entry count limit or the total byte size limit is reached.

### Configuration

```toml
[file_cache]
enabled = true                # Enable file state caching (default: true)
max_entries = 100             # Maximum cached files
max_size_bytes = 26214400     # Max total cache size (25 MB)
```

---

## Output Compaction

Post-processes tool output to reduce token usage. Three levels from lightest to heaviest:

| Level | Transformations |
|-------|----------------|
| `off` | No transformation |
| `safe` (default) | Strip ANSI escape codes, merge consecutive blank lines, collapse carriage-return progress bars |
| `full` | Everything in `safe`, plus: fold repeated lines, compact JSON indentation |

### TOON Encoding

When enabled alongside `full` compaction, TOON (Token-Oriented Object Notation) encodes uniform JSON arrays as compact tables:

```
[2]{id,name,role}:
  1,Alice,admin
  2,Bob,user
```

This is equivalent to:

```json
[{"id":1,"name":"Alice","role":"admin"},{"id":2,"name":"Bob","role":"user"}]
```

TOON instructions are injected into the system prompt so the LLM understands the format.

### Configuration

```toml
[compact]
compaction = "safe"   # off | safe | full (default: safe)
toon = false          # Enable TOON encoding (default: false)
```

### Runtime Control

In `--json-stream` mode, the compaction level can be changed at runtime via `set_config`:

```json
{"type": "set_config", "compaction": "full"}
```
