# aionrs 演进进度

## 路线图

### 阶段 1：长期记忆系统
- [x] 1.1 研究 bb SessionMemory + memdir 模块设计
- [x] 1.2 定义 aionrs 记忆类型和存储结构（types.rs + error.rs）
- [x] 1.3 实现 aion-memory crate 基础框架（paths.rs + 目录管理）
- [x] 1.4 实现记忆文件读写和扫描（store.rs）
- [x] 1.5 实现 MEMORY.md 索引管理（index.rs）
- [x] 1.6 实现记忆系统提示构建（prompt.rs）
- [x] 1.7 集成到 agent 上下文组装流程（修改 context.rs）
- [x] 1.8 测试和阶段收尾

### 阶段 2：多级上下文压缩
- [x] 2.1 研究 bb compact 模块（auto/micro/emergency）
- [x] 2.2 定义类型和配置（CompactConfig, CompactState, Message.timestamp）
- [x] 2.3 实现 microcompact（清除旧工具结果，无 LLM 调用）
- [x] 2.4 实现 autocompact（水位线触发 + LLM 摘要 + 熔断器）
- [x] 2.5 实现紧急截断（安全网）
- [x] 2.6 集成到 engine 循环
- [x] 2.7 测试和阶段收尾

### 阶段 3：Plan Mode
- [x] 3.1 研究 bb planModeV2 + EnterPlanMode/ExitPlanMode
- [x] 3.2 实现 plan mode 类型和配置（PlanConfig + ContextModifier 扩展）
- [x] 3.3 实现 plan mode 状态管理和工具（PlanState + EnterPlanMode/ExitPlanMode）
- [x] 3.4 实现 plan mode 系统提示和 plan 文件管理
- [x] 3.5 集成到 engine 循环（状态转换 + 工具过滤）
- [ ] 3.6 测试和阶段收尾

### 阶段 4：工具描述质量增强
- [ ] 4.1 研究 bb 各工具的描述和使用指导
- [ ] 4.2 增强 7 个内置工具描述（适用/不适用场景、最佳实践）
- [ ] 4.3 增强 system prompt 质量（并行调用引导、错误处理指导等）
- [ ] 4.4 review

### 阶段 5：文件状态追踪缓存
- [ ] 5.1 研究 bb 文件状态缓存（LRU）
- [ ] 5.2 设计 aionrs 文件状态追踪
- [ ] 5.3 实现 LRU 文件缓存
- [ ] 5.4 集成到工具系统（Read/Write/Edit 感知）
- [ ] 5.5 实现重复读取避免
- [ ] 5.6 测试和 review

## 当前状态
- **当前阶段**: 阶段 3 — Plan Mode
- **最近完成**: 3.5 集成到 engine 循环（状态转换 + 工具过滤）（2026-04-11）
  - engine.rs（plan_state/plan_active_flag + apply_context_modifiers 扩展 + run() 工具过滤）
  - registry.rs（to_tool_defs_filtered()）、main.rs（条件注册 plan tools）
  - 修复 R-3.4-01、ToolCategory 添加 Copy/PartialEq
  - 28 个新增测试，累计 ~1412 个
- **下一任务**: 3.6 测试和阶段收尾
