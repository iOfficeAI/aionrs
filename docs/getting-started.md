# Getting Started

## Installation

```bash
# Build from source
cargo build --release

# Binary location
./target/release/aionrs
```

## Command Format

```
aionrs [OPTIONS] [PROMPT]...
```

- With `PROMPT`: single-shot mode — completes the task and exits
- Without `PROMPT`: enters interactive REPL mode

> For the full list of CLI parameters, run `aionrs --help`.

### Subcommands

Management operations live under noun subcommands instead of flat flags. A
subcommand runs its action and exits — it does not start the agent main flow.

| Subcommand | Description |
|------------|--------------|
| `aionrs config init` | Generate a default global config file |
| `aionrs config path` | Print the global config file path |
| `aionrs auth login` | Login with Anthropic account (OAuth device flow) |
| `aionrs auth logout` | Logout (remove saved OAuth credentials) |
| `aionrs session list` | List saved sessions |
| `aionrs skills path` | Print skill directory paths |

### Key Parameters

| Parameter | Description |
|-----------|-------------|
| `--provider <name>` | Provider: `anthropic`, `openai`, `bedrock`, `vertex`, or a custom alias |
| `--model <id>` | Model name |
| `--profile <name>` | Named profile from config file |
| `--compaction <level>` | Output compaction: `off`, `safe` (default), `full` |
| `--toon` | Enable TOON tabular encoding (with `full` compaction) |
| `--max-turns <n>` | Broad model-turn limit per run; unset by default, `0` disables |
| `--max-tool-call-malformed-turns <n>` | Stop after repeated same tool-call-malformed rounds; `0` disables |
| `--max-tool-call-failure-turns <n>` | Stop after repeated tool-call-failure rounds; `0` disables |
| `--auto-approve` | Skip all tool confirmations |
| `--json-stream` | JSON Lines mode for host integration |
| `--resume <id>` | Resume a previous session |
| `--log-dir <path>` | Enable file logging to the given directory |
| `--log-level <filter>` | Log level filter (e.g. `debug`, `info`, `aion_providers=debug`) |

---

## Configuration

### Three-Level Cascading

```
<global config>                   (global, user-level; run `aionrs config path` to find)
    ↓ overridden by
./.aionrs.toml                  (project-level, working directory)
    ↓ overridden by
CLI parameters / env vars        (highest priority)
```

### Generate Default Config

```bash
aionrs config init
# Creates the global config file (run `aionrs config path` to see the location)
```

### Config File Format

```toml
# Global config file (path varies by OS, use `aionrs config path` to find)

[default]
provider = "anthropic"
# model = "claude-sonnet-4-20250514"
max_tokens = 8192
# max_turns = 20  # optional max model turns per run; omit or set 0 to disable
max_tool_call_malformed_turns = 3  # default; set 0 to disable this breaker
max_tool_call_failure_turns = 3  # default; set 0 to disable this breaker

[providers.anthropic]
# api_key = "sk-ant-xxx"       # or env var ANTHROPIC_API_KEY
# base_url = "https://api.anthropic.com"

[providers.openai]
# api_key = "sk-xxx"           # or env var OPENAI_API_KEY
# base_url = "https://api.openai.com"

# Custom provider alias
[providers.my-service]
provider = "openai"
model = "custom-model-v1"
api_key = "sk-xxx"
base_url = "https://my-service.example.com/api/openai"

# Named profiles, switch with --profile <name>
[profiles.deepseek]
provider = "openai"
model = "deepseek-chat"
api_key = "sk-xxx"
base_url = "https://api.deepseek.com"

[profiles.ollama]
provider = "openai"
model = "qwen2.5:32b"
api_key = "ollama"
base_url = "http://localhost:11434"

[profiles.my-service]
provider = "my-service"

# Profile names are user-defined; this is not a built-in profile.
[profiles.my-weak-provider]
provider = "openai"
max_tool_call_malformed_turns = 2
max_tool_call_failure_turns = 2

[tools]
auto_approve = false
allow_list = ["Read", "Grep", "Glob"]

[session]
enabled = true
directory = ".aionrs/sessions"
max_sessions = 20

[compact]
compaction = "safe"   # off | safe | full
toon = false          # Enable TOON encoding for JSON arrays
# autocompact_threshold_pct = 50  # trigger autocompact at N% of context window

[file_cache]
enabled = true
max_entries = 100

[plan]
enabled = true
plan_directory = ".aionrs/plans"

# [logging]
# enabled = true              # enable file logging (default: false)
# level = "info"              # log level filter (default: "info")
# dir = "/path/to/logs"       # log directory (default: platform-specific)
```

### Runtime Limits

`max_turns` is the broad model-turn limit per run. It is unset by default, so
runs have no broad model-turn limit unless you configure one. Set it to `0` to
explicitly disable the broad limit. See [Core Concepts](core-concepts.md) for
the distinction between runs, turns, tool rounds, and tool calls.

`max_tool_call_malformed_turns` limits consecutive same tool-call-malformed
rounds from a provider. The default is `3`; `0` disables this breaker
and leaves stopping to `max_turns` if a broad turn limit is configured.

`max_tool_call_failure_turns` limits consecutive zero-text turns where every
executable tool result failed. The default is `3`; `0` disables this breaker
and leaves stopping to `max_turns` if a broad turn limit is configured.

Precedence is `CLI > profile > project config > global config > built-in default 3`. Use `--max-tool-call-malformed-turns <n>` or `--max-tool-call-failure-turns <n>` for a one-off CLI override.

### API Key Resolution Order

1. `--api-key` CLI parameter
2. Config file `providers.<name>.api_key`
3. Env var `API_KEY`
4. Env var `ANTHROPIC_API_KEY` or `OPENAI_API_KEY` (depends on provider)
5. OAuth credentials (via `aionrs auth login`)

> **Note**: `bedrock` and `vertex` providers use their own cloud credentials and do not require a traditional API key. See [Providers & Auth](providers.md).

### Custom Provider Alias

如果某个后端兼容内置 provider 的协议，可以在 `providers.<alias>` 下声明一个 alias：

```toml
[default]
provider = "my-service"

[providers.my-service]
provider = "openai"
model = "custom-model-v1"
api_key = "sk-xxx"
base_url = "https://my-service.example.com/api/openai"
```

- `default.provider` 和 `profile.provider` 都可以写 alias 名称
- `providers.<alias>.provider` 必须声明底层类型，目前只能是 `anthropic`、`openai`、`bedrock`、`vertex`
- alias 条目会覆盖对应底层 provider 的默认配置

---

## Quick Start

### 1. Initialize and Configure

```bash
aionrs config init
# Edit the config file (run `aionrs config path` to find it), add your API key
```

### 2. Single-Shot Mode

```bash
aionrs "Read and explain crates/aion-agent/src/engine.rs"
```

### 3. Interactive REPL

```
$ aionrs

> Read the file Cargo.toml
     1  [package]
     2  name = "aionrs"
     ...
[turns: 1 | tokens: 1234 in / 567 out]

> Add serde_yaml to dependencies
[tool] Write({"file_path":"Cargo.toml","content":"..."})
Allow? [y]es / [n]o / [a]lways / [q]uit > y
[Write] OK
[turns: 2 | tokens: 2345 in / 890 out]

> /quit
```

REPL commands: `/quit`, `/exit`, or empty line to exit.

### 4. Switching Profiles

```bash
aionrs --profile deepseek "Fix the bug in main.rs"
aionrs --profile ollama "Analyze code quality"
```

### 5. Environment Variables

```bash
export ANTHROPIC_API_KEY=sk-ant-xxx
aionrs "List all Rust files in this project"
```

---

## Tool Confirmation

Destructive tools (Write, Edit, ExecCommand) prompt for confirmation before execution:

```
[tool] Write({"file_path": "/tmp/test.rs", "content": "..."})
Allow? [y]es / [n]o / [a]lways / [q]uit > y
```

| Option | Description |
|--------|-------------|
| `y` / `yes` / Enter | Allow this execution |
| `n` / `no` | Deny — LLM receives a "denied" error |
| `a` / `always` | Auto-approve this tool for the rest of the session |
| `q` / `quit` | Abort the entire agent run |

- Read-only tools (Read, Grep, Glob) are auto-approved by default
- `--auto-approve` skips all confirmations
- `tools.allow_list` in config customizes the whitelist

---

## Session Management

Sessions auto-save to `.aionrs/sessions/`.

```bash
# List saved sessions
aionrs session list

# Resume the latest session
aionrs --resume latest

# Resume a specific session
aionrs --resume a1b2c3

# Create a session with a custom ID
aionrs --session-id my-conv-123
```

- `--session-id` and `--resume` are mutually exclusive
- `--session-id` errors if the ID already exists
- Both flags work in interactive and `--json-stream` mode
- Auto-saves after each tool round
- Auto-cleans oldest sessions when exceeding `max_sessions`
