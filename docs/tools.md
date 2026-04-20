# Built-in Tools

The agent has 7 built-in tools. The LLM automatically selects and invokes them based on the task.

| Tool | Function | Concurrent |
|------|----------|------------|
| **Read** | Read file contents (with line numbers) | Yes |
| **Write** | Write files (auto-creates directories) | No |
| **Edit** | Precise string replacement | No |
| **Bash** | Execute shell commands | No |
| **Grep** | Regex search file contents (via ripgrep) | Yes |
| **Glob** | Find files by pattern matching | Yes |
| **Spawn** | Spawn sub-agents for parallel tasks | No |
| **ToolSearch** | Load schemas for deferred tools | Yes |

---

## Read

Read file contents with line numbers, similar to `cat -n`.

- Supports `offset` and `limit` parameters for reading file slices
- Auto-detects binary files
- Output format: line-numbered text

## Write

Write content to a file atomically.

- Atomic write: writes to a temp file first, then renames
- Auto-creates parent directories

## Edit

Find and replace exact strings in a file.

- Matches `old_string` exactly and replaces with `new_string`
- Requires a unique match by default; errors on multiple matches
- Use `replace_all` to replace all occurrences

## Bash

Execute a shell command and return the result.

- Default timeout: 120 seconds, max 600 seconds
- Returns exit code, stdout, and stderr

## Grep

Search file contents with regular expressions.

- Uses `rg` (ripgrep) when available, falls back to `grep -rn`
- Supports glob filtering and case-insensitive search
- Results limited to 250 lines

## Glob

Find files matching a glob pattern.

- Standard glob patterns (e.g., `**/*.rs`)
- Results sorted by modification time (newest first)
- Returns up to 100 files

## Spawn

See [Sub-Agent Spawning](advanced.md#sub-agent-spawning) in the Advanced Features guide.

## ToolSearch

Load full schemas for deferred tools so the LLM can invoke them. Deferred tools (from MCP servers with `deferred = true`) are registered by name only — their parameter schemas are not loaded until the LLM calls ToolSearch.

- Query by exact name: `"select:Read,Edit,Grep"`
- Keyword search: `"slack send"` returns best matches
- Returns up to 5 results by default

---

## How It Works

```
User input → Build request (system prompt + history + tool definitions)
           → Stream LLM API response
           → Output text to stdout in real-time
           → If LLM returns tool_use → confirm → execute → send result back
           → Loop until LLM stops calling tools
           → Output final reply → save session
```

- Concurrent-safe tools (Read, Grep, Glob) execute in parallel
- Non-concurrent tools (Write, Edit, Bash) execute sequentially
- Tool output is auto-truncated to prevent context window overflow
- Tool output can be compacted (see [Output Compaction](advanced.md#output-compaction))

## Tool Descriptions

Each built-in tool includes a detailed description and usage guidance that is injected into the system prompt. These descriptions help the LLM select the right tool and use it effectively — for example, preferring Grep over Bash for content search, or using Edit instead of Write for modifications.
