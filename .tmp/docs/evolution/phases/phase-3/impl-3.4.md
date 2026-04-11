# 实施总结：3.4 plan mode 系统提示和 plan 文件管理

## 变更文件清单

### 新增文件（3 个）
| 文件 | 行数 | 用途 |
|------|------|------|
| `crates/aion-agent/src/plan/prompt.rs` | 105 | plan mode 系统提示指令和退出通知 |
| `crates/aion-agent/src/plan/file.rs` | 82 | plan 文件路径生成、读写 |
| `crates/aion-agent/tests/plan_prompt_file_test.rs` | 210 | TC-3.4-* 集成测试 |

### 修改文件（6 个）
| 文件 | 变更 | 用途 |
|------|------|------|
| `crates/aion-agent/src/plan/mod.rs` | +2 行 | 导出 `file` 和 `prompt` 模块 |
| `crates/aion-agent/src/context.rs` | +11/-6 行 | 新增 `plan_mode_active: bool` 参数，条件注入 plan mode 指令 |
| `crates/aion-cli/src/main.rs` | +1 行 | 调用 build_system_prompt 传 `false` |
| `crates/aion-agent/tests/memory_context_integration.rs` | +6/-6 行 | 适配新参数 |
| `crates/aion-agent/tests/skills_e2e.rs` | +1/-1 行 | 适配新参数 |
| `tests/skills_e2e.rs` | +1/-1 行 | 适配新参数 |

## 关键设计决策

### 1. plan mode 指令内容（与 plan.md 一致）
采用简洁的 4 阶段工作流：Understand → Design → Write the plan → Submit for review。
相比 bb 的 5 阶段 + 多 agent 并行，aionrs 简化为单线程引导式流程，
适配当前不支持并行子 agent 的架构。

### 2. `build_system_prompt()` 签名变更
新增 `plan_mode_active: bool` 参数而非使用 Option 或 struct。理由：
- 布尔参数语义清晰，零运行时开销
- 现有调用者只需在末尾追加 `false`
- 未来 3.5 集成时 engine 传入实际状态即可

### 3. plan file 管理采用标准 fs 操作
- `write_plan` 自动创建父目录（`create_dir_all`）
- `read_plan` 对 NotFound 返回 None 而非 Error，符合"文件可能不存在"的正常场景
- 路径格式为 `{plan_dir}/{session_id}.md`，无 word-slug 生成

### 4. plan mode 指令注入位置
位于 memory section 之后、skills reminder 之前。确保 LLM 能先看到项目指令和记忆，
再看到 plan mode 限制，最后是可用技能列表。

## 遗留 ISSUE 处理

- R-3.2-01 [LOW] 配置合并逻辑 — **已过期关闭**（连续保留 2 轮未处理）
- R-3.2-02 [LOW] 单元/集成测试重叠 — **已过期关闭**（连续保留 2 轮未处理）
- R-3.3-01 [LOW] Tool trait 文档注释过时 — 继续保留

## 测试统计

- 新增单元测试（prompt.rs）：9 个
- 新增单元测试（file.rs）：7 个
- 新增集成测试（plan_prompt_file_test.rs）：18 个（含 TC-3.4-01 到 TC-3.4-09 + 补充测试）
- 变更适配：更新 22 处 build_system_prompt 调用
- 全量测试：1389 个通过，0 失败
- clippy：无警告

## 对后续子任务的影响

- 3.5（engine 集成）可直接使用 `plan::prompt::plan_mode_instructions()` 和 `plan::prompt::plan_mode_exit_notice()`
- 3.5 需将 `plan_mode_active` 参数从 engine 的 `PlanState` 中动态传入 `build_system_prompt()`
- 3.5 可使用 `plan::file::write_plan()` / `read_plan()` 管理 plan 文件生命周期
- `plan_mode_exit_notice()` 尚未集成到任何地方，3.5 在处理 `PlanModeTransition::Exit` 时需要将其注入到消息中
