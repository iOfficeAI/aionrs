# Output Compaction 验证测试设计

## 目标

端到端验证 output compaction 功能：压缩确实发生、不同级别产生不同结果、TOON 编码正常工作、运行时 SetConfig 切换生效、LLM 能正确理解压缩后的内容。

## 架构：三层测试

### A 层：Tool 结果层（本地 Mock，确定性）

**文件：** `crates/aion-agent/tests/output_compaction_test.rs`

使用 MockTool + MockLlmProvider，验证同一工具输出在不同 compaction 级别下产生不同的 `ToolResult.content`。

**测试数据：** 精心构造的字符串，确保触发所有压缩阶段：

```
\x1b[32mSTATUS: OK\x1b[0m           ← ANSI 转义码（Safe 剥离）
\n\n\n                              ← 空行（Safe 合并）
50%\r100%\n                         ← CR 进度条（Safe 折叠）
Compiling dep-0 v1.0.0\n           ← 5+ 相似行（Full 折叠）
Compiling dep-1 v1.0.0\n
Compiling dep-2 v1.0.0\n
Compiling dep-3 v1.0.0\n
Compiling dep-4 v1.0.0\n
{"id":1,\n    "name":"Alice"\n}     ← 4 空格缩进 JSON（Full 紧凑化）
```

TOON 编码用的第二组测试数据：

```json
[{"id":1,"name":"Alice","role":"admin"},{"id":2,"name":"Bob","role":"user"}]
```

**测试用例：**

| # | 名称 | 级别 | TOON | 断言 |
|---|------|------|------|------|
| 1 | Off 透传 | Off | false | ToolResult.content == 原始内容（不变） |
| 2 | Safe 剥离 ANSI 并清理 | Safe | false | 无 `\x1b`、空行已合并、CR 已折叠；重复行和 JSON 缩进保持不变 |
| 3 | Full 折叠并紧凑化 | Full | false | 包含 `[... N similar lines]`，JSON 重新缩进为 2 空格 |
| 4 | TOON 编码数组 | Full | true | 包含 `[2]{id,name,role}:` 表头 |
| 5 | TOON 关闭不编码 | Full | false | 不含表头，JSON 保持原样 |

**实现方式：**

- 注册返回测试数据的 MockTool
- 配置 MockLlmProvider：
  - 第 1 轮：请求工具调用
  - 第 2 轮：文本响应（结束循环）
- `engine.run()` 后检查 `messages` 历史中的 `ToolResult` content block
- 对 content 做断言

### B 层：LLM 请求层（CapturingProvider）

**文件：** 与 A 层相同（`output_compaction_test.rs`）

使用自定义 `CapturingProvider` 包装 MockLlmProvider，记录每次 `stream()` 收到的 `LlmRequest`。证明压缩后的内容就是 LLM 实际看到的内容。

**CapturingProvider 设计：**

```rust
struct CapturingProvider {
    inner: MockLlmProvider,
    captured: Arc<Mutex<Vec<LlmRequest>>>,
}
```

- 实现 `LlmProvider`，委托给 `inner.stream()`
- 委托前将 request 克隆到 `captured`
- 测试在 engine.run() 后读取 `captured` 检查发送内容

**测试用例：**

| # | 名称 | 断言 |
|---|------|------|
| 6 | 压缩后内容到达 LLM | 第 2 次捕获的请求（工具执行后）中，`tool_result` 的 content 是压缩后的文本（非原始内容） |
| 7 | SetConfig 运行时切换 | 第 1 轮：Off → 请求中工具结果未压缩。调用 `apply_config_update(compaction: "full")`。第 2 轮：同一工具 → 请求中工具结果已压缩。对比两者。 |
| 8 | TOON system prompt 注入 | TOON 开启：捕获的请求中 `system_prompt` 包含 "TOON" 和 "Token-Oriented Object Notation"。TOON 关闭：system_prompt 中不包含 "TOON"。 |

### C 层：端到端真实 API（OPENAI_API_KEY，gpt-4o-mini）

**文件：** `crates/aion-agent/tests/e2e/compaction.rs`

**跳过模式：**

```rust
let Some(api_key) = openai_api_key() else {
    eprintln!("[e2e:compaction] OPENAI_API_KEY not set — skipping");
    return;
};
```

**日志原则：** 每个测试通过 `eprintln!` 打印关键信息，配合 `--nocapture` 供人工审核：

```
[e2e:compaction] === 测试名称 ===
[e2e:compaction] 工具输出 (原始, N 字符): <content>
[e2e:compaction] 工具输出 (压缩后, M 字符): <content>
[e2e:compaction] 问 LLM: <question>
[e2e:compaction] LLM 回答: <response>
[e2e:compaction] Token 用量: N input / M output
[e2e:compaction] ✓ PASS
```

**测试数据：** 与 A 层相同的精心构造字符串，通过 MockTool 注册（真实 LLM + Mock 工具）。

**测试用例：**

| # | 名称 | 方法 | 断言 |
|---|------|------|------|
| 9 | Off vs Safe 内容对比 | 注册 MockTool 返回含 ANSI 的输出。分别用 Off 和 Safe 跑。直接检查 engine messages 中的 ToolResult.content。问 LLM："工具输出中是否包含颜色转义码？回答 yes 或 no" | Off：content 含 `\x1b`，LLM 回答 "yes"。Safe：content 无 `\x1b`，LLM 回答 "no"。 |
| 10 | Off vs Full token 节省 | 注册 MockTool 返回大量输出（重复行 + 冗长 JSON）。分别用 Off 和 Full 跑。对比第二轮的 `result.usage.input_tokens`。 | Full 的 input_tokens < Off 的 input_tokens（打印两个值） |
| 11 | TOON 可理解性 + system prompt | 注册 MockTool 返回 uniform JSON array。开启 TOON + Full。问 LLM："第二条记录的 name 是什么？只回答名字。" 通过 `eprintln!` 打印 system prompt 中 TOON 相关段落供人工审核。 | LLM 正确回答（如 "Bob"）。直接验证：ToolResult.content 包含 TOON 表头；system prompt 包含 "TOON"。 |

## 文件组织

```
crates/aion-agent/tests/
  output_compaction_test.rs     ← A 层 (5 用例) + B 层 (3 用例)
  e2e/
    mod.rs                      ← 已有，新增 `mod compaction;`
    compaction.rs               ← C 层 (3 用例)
```

## 执行方式

```bash
# A + B 层（无需 API key，始终可跑）
cargo test -p aion-agent output_compaction

# C 层（需要 OPENAI_API_KEY，打印关键信息）
OPENAI_API_KEY=sk-... cargo test -p aion-agent --test e2e compaction -- --nocapture

# 所有层一起跑
OPENAI_API_KEY=sk-... cargo test -p aion-agent -- --nocapture
```

## 设计决策

1. **测试数据确定性** — 精心构造以 100% 触发每个压缩阶段，消除数据差异导致的不稳定性。

2. **CapturingProvider 用于 B 层** — 相比基于文件的 dump_request_path，进程内捕获更干净，无需文件系统协调。

3. **C 层结合直接检查 + LLM 问答** — 直接 ToolResult.content 检查是主断言（确定性）；LLM 问答是次要证据（尽力而为，打印供人工审核）。

4. **gpt-4o-mini** — 最便宜的模型，足够回答事实性 yes/no 和短答案问题。

5. **跳过模式** — 遵循已有 `e2e/openai.rs` 约定：无 key 时 `eprintln!` + `return`。
