# aionrs JSON Stream Protocol Spec

> This protocol defines the communication between aionrs (Rust CLI) and a host client (e.g., AionUi Electron app) via stdin/stdout JSON Lines.

## Overview

```
┌──────────────┐   stdin (JSON Lines)    ┌──────────────────┐
│              │ ◄─────────────────────── │                  │
│ aionrs│                          │   Host Client    │
│  (Rust CLI)  │ ──────────────────────► │   (AionUi etc.)  │
│              │   stdout (JSON Lines)    │                  │
└──────────────┘                          └──────────────────┘
     stderr → diagnostic logs (not part of protocol)
```

- **Transport**: stdin/stdout, one JSON object per line (JSON Lines / NDJSON)
- **Encoding**: UTF-8
- **Activation**: `aionrs --json-stream [other flags]`
- **Lifecycle**: One process per conversation; process stays alive for multi-turn

## 1. Agent → Client Events (stdout)

Every line is a JSON object with a `type` field.

### 1.1 `ready`

Emitted once after initialization completes. Client MUST wait for this before sending messages.

```json
{
  "type": "ready",
  "version": "0.2.0",
  "session_id": "a1b2c3",
  "capabilities": {
    "tool_approval": true,
    "thinking": true,
    "effort": false,
    "effort_levels": [],
    "modes": ["default", "auto_edit", "yolo"],
    "current_mode": "default",
    "mcp": true
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `version` | string | Protocol version (semver) |
| `session_id` | string? | Session ID (omitted when sessions are disabled in config) |
| `capabilities.tool_approval` | bool | Whether agent supports pause-and-wait tool approval |
| `capabilities.thinking` | bool | Whether current provider supports extended thinking |
| `capabilities.effort` | bool | Whether current provider supports reasoning_effort |
| `capabilities.effort_levels` | string[] | Valid effort values (e.g., `["low", "medium", "high"]`). Empty when effort is false |
| `capabilities.modes` | string[] | Available approval modes for `set_mode` command |
| `capabilities.current_mode` | string | Currently active approval mode |
| `capabilities.mcp` | bool | Whether MCP tools are available |

### 1.2 `stream_start`

A new response turn has started.

```json
{
  "type": "stream_start",
  "msg_id": "abc-123"
}
```

### 1.3 `text_delta`

Incremental text output (streaming).

```json
{
  "type": "text_delta",
  "text": "Hello, ",
  "msg_id": "abc-123"
}
```

### 1.4 `thinking`

Model's internal reasoning (if extended thinking is enabled).

```json
{
  "type": "thinking",
  "text": "Let me analyze the code structure...",
  "msg_id": "abc-123"
}
```

### 1.5 `tool_request`

Agent wants to invoke a tool and needs client approval. Agent PAUSES execution until it receives `tool_approve` or `tool_deny`.

```json
{
  "type": "tool_request",
  "msg_id": "abc-123",
  "call_id": "tool-call-001",
  "tool": {
    "name": "Write",
    "category": "edit",
    "args": {
      "file_path": "/src/main.rs",
      "content": "fn main() { ... }"
    },
    "description": "Write to /src/main.rs"
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `call_id` | string | Unique ID for this tool invocation |
| `tool.name` | string | Tool name: `Read`, `Write`, `Edit`, `Bash`, `Glob`, `Grep`, `Spawn`, or MCP tool name |
| `tool.category` | string | `"info"` (read-only), `"edit"` (file mutation), `"exec"` (shell), `"mcp"` (MCP tool) |
| `tool.args` | object | Tool arguments |
| `tool.description` | string | Human-readable one-line description |

**Category mapping for built-in tools:**

| Tool | Category | Rationale |
|------|----------|-----------|
| `Read` | `info` | Read-only file access |
| `Glob` | `info` | Read-only file search |
| `Grep` | `info` | Read-only content search |
| `Write` | `edit` | Creates or overwrites files |
| `Edit` | `edit` | Modifies file content |
| `Bash` | `exec` | Executes shell commands |
| `Spawn` | `exec` | Spawns sub-agent |
| MCP tools | `mcp` | External MCP server tools |

> **Note**: When `auto_approve = true` (yolo mode) or when a tool is in the `allow_list`, the agent executes immediately and emits `tool_running` directly, skipping `tool_request`.

### 1.6 `tool_running`

Tool execution has started (after approval or auto-approve).

```json
{
  "type": "tool_running",
  "msg_id": "abc-123",
  "call_id": "tool-call-001",
  "tool_name": "Write"
}
```

### 1.7 `tool_result`

Tool execution completed.

```json
{
  "type": "tool_result",
  "msg_id": "abc-123",
  "call_id": "tool-call-001",
  "tool_name": "Write",
  "status": "success",
  "output": "File written successfully",
  "output_type": "text"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `status` | string | `"success"` or `"error"` |
| `output` | string | Tool output (truncated if exceeds limit) |
| `output_type` | string | `"text"` (default), `"diff"` (for Edit tool), `"image"` (base64) |

**Special output for Edit tool** (`output_type: "diff"`):

```json
{
  "type": "tool_result",
  "msg_id": "abc-123",
  "call_id": "tool-call-002",
  "tool_name": "Edit",
  "status": "success",
  "output": "--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,3 +1,3 @@\n-old line\n+new line",
  "output_type": "diff",
  "metadata": {
    "file_path": "/src/main.rs"
  }
}
```

### 1.8 `tool_cancelled`

Tool was denied by client or cancelled.

```json
{
  "type": "tool_cancelled",
  "msg_id": "abc-123",
  "call_id": "tool-call-001",
  "reason": "User denied"
}
```

### 1.9 `stream_end`

Current response turn finished.

```json
{
  "type": "stream_end",
  "msg_id": "abc-123",
  "usage": {
    "input_tokens": 1500,
    "output_tokens": 320,
    "cache_read_tokens": 800,
    "cache_write_tokens": 200
  }
}
```

### 1.10 `error`

An error occurred. The agent may or may not continue depending on severity.

```json
{
  "type": "error",
  "msg_id": "abc-123",
  "error": {
    "code": "provider_error",
    "message": "Rate limit exceeded",
    "retryable": true
  }
}
```

| Error Code | Description |
|------------|-------------|
| `provider_error` | LLM API error (rate limit, auth, etc.) |
| `tool_error` | Built-in tool execution error |
| `config_error` | Configuration or initialization error |
| `protocol_error` | Invalid command from client |
| `internal_error` | Unexpected internal error |

### 1.11 `info`

Informational message (non-critical, for display only).

```json
{
  "type": "info",
  "msg_id": "abc-123",
  "message": "Stream interrupted, retrying... (1/2)"
}
```

### 1.12 `config_changed`

Emitted after a `set_config` command is processed. Contains the updated capabilities snapshot reflecting the current provider/model configuration.

```json
{
  "type": "config_changed",
  "capabilities": {
    "tool_approval": true,
    "thinking": false,
    "effort": true,
    "effort_levels": ["low", "medium", "high"],
    "modes": ["default", "auto_edit", "yolo"],
    "current_mode": "default",
    "mcp": true
  }
}
```

Clients should update their UI controls (e.g., enable/disable thinking toggle, populate effort dropdown) based on the new capabilities.

### 1.13 `mcp_ready`

Emitted after a dynamically injected MCP server has connected and its tools are registered.

```json
{
  "type": "mcp_ready",
  "name": "my-tools",
  "tools": ["tool_a", "tool_b"]
}
```

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Server name (as provided in `add_mcp_server`) |
| `tools` | string[] | List of tool names registered from this server |

## 2. Client → Agent Commands (stdin)

Every line is a JSON object with a `type` field.

### 2.1 `message`

Send a user message. Agent responds with a stream of events.

```json
{
  "type": "message",
  "msg_id": "abc-123",
  "content": "Read the file src/main.rs and explain the code",
  "files": ["/path/to/attached/file.png"]
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `msg_id` | string | yes | Client-generated unique message ID |
| `content` | string | yes | User's message text |
| `files` | string[] | no | Attached file paths (images, documents) |

### 2.2 `stop`

Abort the current response stream.

```json
{
  "type": "stop"
}
```

Agent MUST:
1. Cancel any in-flight LLM request
2. Cancel any running tool (if possible)
3. Emit `stream_end` for the current msg_id

### 2.3 `tool_approve`

Approve a pending tool execution.

```json
{
  "type": "tool_approve",
  "call_id": "tool-call-001",
  "scope": "once"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `call_id` | string | Must match a pending `tool_request` |
| `scope` | string | `"once"` = this call only; `"always"` = auto-approve this tool+category for the session |

When `scope = "always"`, the agent adds the tool's category to the session allow-list, so future calls of the same category skip approval.

### 2.4 `tool_deny`

Deny a pending tool execution.

```json
{
  "type": "tool_deny",
  "call_id": "tool-call-001",
  "reason": "Not allowed to write this file"
}
```

Agent MUST:
1. Emit `tool_cancelled` event
2. Feed the denial reason back to the LLM as tool result
3. Continue the conversation (LLM decides next action)

### 2.5 `init_history`

Inject prior conversation context (for conversation resume).

```json
{
  "type": "init_history",
  "text": "Previous conversation summary:\nUser asked about X...\nAssistant replied with Y..."
}
```

Must be sent BEFORE the first `message` command. Agent incorporates this as conversation context.

### 2.6 `set_mode`

Change the agent's approval mode for the session.

```json
{
  "type": "set_mode",
  "mode": "yolo"
}
```

| Mode | Behavior |
|------|----------|
| `"default"` | All tools need approval (except allow-listed) |
| `"auto_edit"` | `info` and `edit` auto-approved; `exec` and `mcp` need approval |
| `"yolo"` | All tools auto-approved |

### 2.7 `set_config`

Update model, thinking, or effort configuration at runtime.

```json
{
  "type": "set_config",
  "model": "claude-opus-4",
  "thinking": "enabled",
  "thinking_budget": 16000,
  "effort": "high",
  "compaction": "safe"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `model` | string | no | Switch to a different model |
| `thinking` | string | no | `"enabled"` or `"disabled"` |
| `thinking_budget` | number | no | Token budget for thinking (default: 10000) |
| `effort` | string | no | Reasoning effort level (e.g., `"low"`, `"medium"`, `"high"`) |
| `compaction` | string | no | Output compaction level: `"off"`, `"safe"`, `"full"` |

All fields are optional. Only provided fields are updated.

> **Validation**: The agent validates `thinking` and `effort` values against the current provider's capabilities. If the provider does not support a feature, the change is rejected with a descriptive message in the `info` event. After processing, a `config_changed` event is always emitted with the updated capabilities.

### 2.8 `add_mcp_server`

Dynamically inject an MCP server before the conversation starts. This command is only accepted during the **pre-message phase** — after the `ready` event and before the first `message` command. Any `add_mcp_server` sent after the first `message` is rejected with an error.

```json
{
  "type": "add_mcp_server",
  "name": "my-tools",
  "transport": "stdio",
  "command": "node",
  "args": ["bridge.js", "--port", "9000"],
  "env": {"TOKEN": "abc123"}
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Unique server name |
| `transport` | string | yes | `"stdio"`, `"sse"`, or `"streamable-http"` |
| `command` | string | stdio only | Executable to launch |
| `args` | string[] | no | Command arguments |
| `env` | object | no | Environment variables for the subprocess |
| `url` | string | sse/http only | Server URL |
| `headers` | object | no | HTTP headers (for sse/http) |

**Lifecycle:**

```
Agent  → stdout: {"type":"ready",...}
Client → stdin:  {"type":"add_mcp_server","name":"tools","transport":"stdio","command":"node","args":["bridge.js"]}
Agent  → stdout: {"type":"mcp_ready","name":"tools","tools":["tool_a","tool_b"]}
Client → stdin:  {"type":"message","msg_id":"m1","content":"Hello"}
                  ↑ first message ends the injection window
```

After the first `message`, any further `add_mcp_server` commands are rejected:

```json
{
  "type": "error",
  "error": {
    "code": "protocol_error",
    "message": "AddMcpServer 'name': rejected — only allowed before first Message",
    "retryable": false
  }
}
```

## 3. Lifecycle

### 3.1 Startup

```
Client spawns:
  aionrs --json-stream \
    --provider anthropic \
    --model claude-sonnet-4-20250514 \
    --max-tokens 8192 \
    --max-turns 30

Environment variables set by client:
  ANTHROPIC_API_KEY=sk-...
  # or OPENAI_API_KEY, AWS_REGION, etc.

Agent initializes → stdout: {"type":"ready","session_id":"a1b2c3",...}
```

**Pre-message phase (optional):**

Between receiving `ready` and sending the first `message`, the client may inject MCP servers via `add_mcp_server` commands. The agent connects each server and emits `mcp_ready` when ready. This phase ends when the first `message` is sent.

**Session lifecycle flags** (mutually exclusive):

| Flag | Description |
|------|-------------|
| `--session-id <ID>` | Use a specific session ID instead of auto-generating one. Errors if the ID already exists. |
| `--resume <ID>` | Resume a previous session (loads conversation history). Use `latest` to resume the most recent. |

```bash
# New session with a custom ID
aionrs --json-stream --session-id my-conv-123 --provider openai --model gpt-4o

# Resume an existing session
aionrs --json-stream --resume my-conv-123 --provider openai --model gpt-4o
```

### 3.2 Message Turn

```
Client → stdin:  {"type":"message","msg_id":"m1","content":"Hello"}
Agent  → stdout: {"type":"stream_start","msg_id":"m1"}
Agent  → stdout: {"type":"text_delta","text":"Hi! ","msg_id":"m1"}
Agent  → stdout: {"type":"text_delta","text":"How can I help?","msg_id":"m1"}
Agent  → stdout: {"type":"stream_end","msg_id":"m1","usage":{...}}
```

### 3.3 Tool Approval Flow

```
Client → stdin:  {"type":"message","msg_id":"m2","content":"Create a hello.rs file"}
Agent  → stdout: {"type":"stream_start","msg_id":"m2"}
Agent  → stdout: {"type":"text_delta","text":"I'll create the file.","msg_id":"m2"}
Agent  → stdout: {"type":"tool_request","msg_id":"m2","call_id":"t1","tool":{"name":"Write","category":"edit",...}}
  ← Agent PAUSES here, waiting for approval →
Client → stdin:  {"type":"tool_approve","call_id":"t1","scope":"once"}
Agent  → stdout: {"type":"tool_running","msg_id":"m2","call_id":"t1","tool_name":"Write"}
Agent  → stdout: {"type":"tool_result","msg_id":"m2","call_id":"t1","status":"success",...}
Agent  → stdout: {"type":"text_delta","text":"File created successfully.","msg_id":"m2"}
Agent  → stdout: {"type":"stream_end","msg_id":"m2","usage":{...}}
```

### 3.4 Multi-Tool Parallel Execution

When the LLM requests multiple tools in one turn, agent emits multiple `tool_request` events. Client can approve/deny them independently.

```
Agent  → stdout: {"type":"tool_request","call_id":"t1","tool":{"name":"Read","category":"info",...}}
Agent  → stdout: {"type":"tool_request","call_id":"t2","tool":{"name":"Read","category":"info",...}}
Client → stdin:  {"type":"tool_approve","call_id":"t1","scope":"once"}
Client → stdin:  {"type":"tool_approve","call_id":"t2","scope":"once"}
Agent  → stdout: {"type":"tool_running","call_id":"t1",...}
Agent  → stdout: {"type":"tool_running","call_id":"t2",...}
Agent  → stdout: {"type":"tool_result","call_id":"t1",...}
Agent  → stdout: {"type":"tool_result","call_id":"t2",...}
```

### 3.5 Shutdown

Client closes stdin (EOF) or sends SIGTERM. Agent cleans up and exits.

## 4. Error Handling

### 4.1 Invalid Command

If client sends malformed JSON or unknown command type:

```json
{
  "type": "error",
  "msg_id": null,
  "error": {
    "code": "protocol_error",
    "message": "Unknown command type: foo",
    "retryable": false
  }
}
```

### 4.2 Provider Errors

Agent should emit error and let the conversation continue if possible:

```json
{
  "type": "error",
  "msg_id": "m3",
  "error": {
    "code": "provider_error",
    "message": "Rate limit exceeded. Retry after 30s.",
    "retryable": true
  }
}
```

### 4.3 Fatal Errors

For unrecoverable errors, agent emits error and exits with non-zero status:

```json
{
  "type": "error",
  "msg_id": null,
  "error": {
    "code": "config_error",
    "message": "ANTHROPIC_API_KEY not set",
    "retryable": false
  }
}
```

## 5. Configuration via CLI Flags

When spawned in `--json-stream` mode, all configuration is passed via CLI flags and environment variables:

```bash
aionrs --json-stream \
  --provider <anthropic|openai|bedrock|vertex> \
  --model <model-id> \
  --max-tokens <N> \
  --max-turns <N> \
  --base-url <URL> \
  --system-prompt <TEXT> \
  --auto-approve          # Start in yolo mode
  --workspace <PATH>      # Working directory for file operations
```

**Environment variables** (set by client before spawn):

| Provider | Variables |
|----------|-----------|
| Anthropic | `ANTHROPIC_API_KEY`, `ANTHROPIC_BASE_URL` |
| OpenAI | `OPENAI_API_KEY`, `OPENAI_BASE_URL` |
| Bedrock | `AWS_REGION`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_PROFILE` |
| Vertex AI | `GOOGLE_APPLICATION_CREDENTIALS`, `VERTEX_PROJECT_ID`, `VERTEX_REGION` |

## 6. Protocol Versioning

The `ready` event includes a `version` field. Clients should check version compatibility.

- **Minor version bump**: New optional event types or fields added (backward compatible)
- **Major version bump**: Breaking changes to existing events/commands

Current version: `0.2.0`
