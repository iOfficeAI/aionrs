# Output Compaction Verification Test Design

## Goal

Verify that the output compaction feature works correctly end-to-end: compression actually happens, different levels produce different results, TOON encoding works, runtime switching via SetConfig takes effect, and LLMs can still understand compressed content.

## Architecture: Three-Layer Testing

### A Layer: Tool Result Layer (Local Mock, Deterministic)

**File:** `crates/aion-agent/tests/output_compaction_test.rs`

Uses MockTool + MockLlmProvider to verify that the same tool output produces different `ToolResult.content` under different compaction levels.

**Test Data:** A single carefully constructed string that triggers all compaction stages:

```
\x1b[32mSTATUS: OK\x1b[0m           ← ANSI escape (Safe strips)
\n\n\n                              ← blank lines (Safe merges)
50%\r100%\n                         ← CR progress (Safe collapses)
Compiling dep-0 v1.0.0\n           ← 5+ similar lines (Full folds)
Compiling dep-1 v1.0.0\n
Compiling dep-2 v1.0.0\n
Compiling dep-3 v1.0.0\n
Compiling dep-4 v1.0.0\n
{"id":1,\n    "name":"Alice"\n}     ← JSON with 4-space indent (Full compacts)
```

A second test data string for TOON encoding:

```json
[{"id":1,"name":"Alice","role":"admin"},{"id":2,"name":"Bob","role":"user"}]
```

**Test Cases:**

| # | Name | Level | TOON | Assertion |
|---|------|-------|------|-----------|
| 1 | Off passthrough | Off | false | ToolResult.content == raw input (unchanged) |
| 2 | Safe strips ANSI and cleans | Safe | false | No `\x1b`, blank lines merged, CR collapsed; repeated lines and JSON indent preserved |
| 3 | Full folds and compacts | Full | false | `[... N similar lines]` present, JSON re-indented to 2-space |
| 4 | TOON encodes arrays | Full | true | Contains `[2]{id,name,role}:` table header |
| 5 | TOON off does not encode | Full | false | No table header, JSON remains as-is |

**Implementation Approach:**

- Register MockTool that returns the test data
- Configure MockLlmProvider to:
  - Turn 1: request tool call
  - Turn 2: text response (to end the loop)
- After `engine.run()`, inspect `messages` history to find the `ToolResult` content block
- Assert on the content

### B Layer: LLM Request Layer (Capturing Provider)

**File:** Same as A layer (`output_compaction_test.rs`)

Uses a custom `CapturingProvider` that wraps MockLlmProvider and records every `LlmRequest` it receives via `stream()`. This proves that the compressed content is what the LLM actually sees.

**CapturingProvider Design:**

```rust
struct CapturingProvider {
    inner: MockLlmProvider,
    captured: Arc<Mutex<Vec<LlmRequest>>>,
}
```

- Implements `LlmProvider` by delegating to `inner.stream()`
- Before delegating, clones the request into `captured`
- Tests read `captured` after engine.run() to inspect what was sent

**Test Cases:**

| # | Name | Assertion |
|---|------|-----------|
| 6 | Compressed content reaches LLM | In the 2nd captured request (after tool execution), the `tool_result` content block contains compressed text (not raw) |
| 7 | SetConfig runtime switch | Turn 1: Off → tool result in request is raw. Call `apply_config_update(compaction: "full")`. Turn 2: same tool → tool result in request is compressed. Compare the two. |

### C Layer: End-to-End Real API (OPENAI_API_KEY, gpt-4o-mini)

**File:** `crates/aion-agent/tests/e2e/compaction.rs`

**Skip Pattern:**

```rust
let Some(api_key) = openai_api_key() else {
    eprintln!("[e2e:compaction] OPENAI_API_KEY not set — skipping");
    return;
};
```

**Logging Principle:** Every test prints key info via `eprintln!` for human review with `--nocapture`:

```
[e2e:compaction] === Test Name ===
[e2e:compaction] Tool output (raw, N chars): <content>
[e2e:compaction] Tool output (after compaction, M chars): <content>
[e2e:compaction] Question to LLM: <question>
[e2e:compaction] LLM response: <response>
[e2e:compaction] Token usage: N input / M output
[e2e:compaction] ✓ PASS
```

**Test Data:** Same carefully constructed strings as A layer, registered via MockTool (real LLM, mock tool).

**Test Cases:**

| # | Name | Method | Assertion |
|---|------|--------|-----------|
| 8 | Off vs Safe content | Register MockTool returning ANSI-laden output. Run twice: Off and Safe. Directly check ToolResult.content in engine messages. Ask LLM: "Does the tool output contain color escape codes? Answer yes or no" | Off: content has `\x1b`, LLM says "yes". Safe: content has no `\x1b`, LLM says "no". |
| 9 | Off vs Full token savings | Register MockTool returning large output (repeated lines + verbose JSON). Run twice: Off and Full. Compare `result.usage.input_tokens` from second turn. | Full's input_tokens < Off's input_tokens (print both values) |
| 10 | TOON comprehension | Register MockTool returning uniform JSON array. Run with TOON enabled + Full. Ask LLM: "What is the name in the second record? Answer with just the name." | LLM answers correctly (e.g., "Bob"). Also directly verify ToolResult.content contains TOON header. |

## File Organization

```
crates/aion-agent/tests/
  output_compaction_test.rs     ← A layer (5 cases) + B layer (2 cases)
  e2e/
    mod.rs                      ← existing, add `mod compaction;`
    compaction.rs               ← C layer (3 cases)
```

## Test Execution

```bash
# A + B layer (always runs, no API key needed)
cargo test -p aion-agent output_compaction

# C layer (requires OPENAI_API_KEY, prints key info)
OPENAI_API_KEY=sk-... cargo test -p aion-agent --test e2e compaction -- --nocapture

# All layers at once
OPENAI_API_KEY=sk-... cargo test -p aion-agent -- --nocapture
```

## Design Decisions

1. **Test data is deterministic** — carefully constructed to 100% trigger each compression stage, eliminating flakiness from data variance.

2. **CapturingProvider for B layer** — instead of file-based dump_request_path, an in-process capture is cleaner for testing and doesn't require filesystem coordination.

3. **C layer combines direct checks + LLM questions** — direct ToolResult.content inspection is the primary assertion (deterministic); LLM question/answer is secondary evidence of comprehension (best-effort, printed for human review).

4. **gpt-4o-mini** — cheapest model, sufficient for factual yes/no and short-answer questions.

5. **Skip pattern** — follows existing `e2e/openai.rs` convention: `eprintln!` + `return` when key is absent.
