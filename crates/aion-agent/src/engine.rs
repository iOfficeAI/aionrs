use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use aion_config::compact::CompactConfig;
use aion_config::config::Config;
use aion_config::hooks::HookEngine;
use aion_protocol::events::ToolCategory;
use aion_providers::{LlmProvider, ProviderError, create_provider};
use aion_tools::registry::ToolRegistry;
use aion_types::llm::{LlmEvent, LlmRequest};
use aion_types::message::{ContentBlock, Message, Role, StopReason, TokenUsage};
use aion_types::skill_types::{ContextModifier, PlanModeTransition, effort_to_string};

use crate::cache_diagnostics::{CacheBreakDetector, CacheDiagnostic, CacheStats};
use crate::compact::state::CompactState;
use crate::compact::{auto, emergency, micro};
use crate::confirm::ToolConfirmer;
use crate::orchestration::{
    ExecutionControl, execute_tool_calls, execute_tool_calls_with_approval,
};
use crate::output::OutputSink;
use crate::plan::prompt as plan_prompt;
use crate::plan::state::PlanState;
use crate::session::{Session, SessionManager};

pub struct AgentEngine {
    provider: Arc<dyn LlmProvider>,
    tools: ToolRegistry,
    messages: Vec<Message>,
    system_prompt: String,
    model: String,
    max_tokens: u32,
    max_turns: usize,
    total_usage: TokenUsage,
    thinking: Option<aion_types::llm::ThinkingConfig>,
    /// Resolved provider compat settings (for capability validation)
    compat: aion_config::compat::ProviderCompat,
    confirmer: Arc<Mutex<ToolConfirmer>>,
    hooks: Option<HookEngine>,
    session_manager: Option<SessionManager>,
    current_session: Option<Session>,
    output: Arc<dyn OutputSink>,
    current_msg_id: String,
    approval_manager: Option<Arc<aion_protocol::ToolApprovalManager>>,
    protocol_writer: Option<Arc<aion_protocol::writer::ProtocolWriter>>,
    allow_list: Vec<String>,
    /// Persisted reasoning effort, updated by skill context modifiers.
    /// Carried into each turn's LlmRequest.reasoning_effort.
    current_reasoning_effort: Option<String>,
    /// Compaction configuration (thresholds, enabled flag, etc.)
    compact_config: CompactConfig,
    /// Runtime compaction state (circuit breaker, last input tokens)
    compact_state: CompactState,
    /// Runtime plan mode state (active flag, pre-plan allow-list, plan file path)
    plan_state: PlanState,
    /// Shared flag read by EnterPlanMode/ExitPlanMode tools to validate transitions.
    /// Updated by the engine when processing PlanModeTransition modifiers.
    plan_active_flag: Option<Arc<AtomicBool>>,
    /// Prompt cache break detector for diagnostics.
    cache_detector: CacheBreakDetector,
}

impl AgentEngine {
    pub fn new(config: Config, tools: ToolRegistry, output: Arc<dyn OutputSink>) -> Self {
        let provider = create_provider(&config);
        Self::new_with_provider(provider, config, tools, output)
    }

    /// Create an engine with an externally-provided provider (for sub-agent sharing)
    pub fn new_with_provider(
        provider: Arc<dyn LlmProvider>,
        config: Config,
        tools: ToolRegistry,
        output: Arc<dyn OutputSink>,
    ) -> Self {
        let system_prompt = config.system_prompt.clone().unwrap_or_default();
        let confirmer =
            ToolConfirmer::new(config.tools.auto_approve, config.tools.allow_list.clone());

        let session_manager = if config.session.enabled {
            Some(SessionManager::new(
                config.session.directory.clone().into(),
                config.session.max_sessions,
            ))
        } else {
            None
        };

        let allow_list = config.tools.allow_list.clone();
        let compact_config = config.compact.clone();

        Self {
            provider,
            tools,
            messages: Vec::new(),
            system_prompt,
            model: config.model,
            max_tokens: config.max_tokens,
            max_turns: config.max_turns,
            total_usage: TokenUsage::default(),
            thinking: config.thinking,
            compat: config.compat.clone(),
            confirmer: Arc::new(Mutex::new(confirmer)),
            // Always initialise Some so that skill-declared hooks can be merged in
            // even when the global config has no static hooks configured.
            hooks: Some(HookEngine::new(config.hooks.clone())),
            session_manager,
            current_session: None,
            output,
            current_msg_id: String::new(),
            approval_manager: None,
            protocol_writer: None,
            allow_list,
            current_reasoning_effort: None,
            compact_config,
            compact_state: CompactState::new(),
            plan_state: PlanState::default(),
            plan_active_flag: None,
            cache_detector: CacheBreakDetector::new(),
        }
    }

    /// Create from a resumed session
    pub fn resume(
        config: Config,
        tools: ToolRegistry,
        output: Arc<dyn OutputSink>,
        session: Session,
    ) -> Self {
        let provider = create_provider(&config);
        Self::resume_with_provider(provider, config, tools, output, session)
    }

    /// Create from a resumed session with an externally-provided provider
    pub fn resume_with_provider(
        provider: Arc<dyn LlmProvider>,
        config: Config,
        tools: ToolRegistry,
        output: Arc<dyn OutputSink>,
        session: Session,
    ) -> Self {
        let system_prompt = config.system_prompt.clone().unwrap_or_default();
        let confirmer =
            ToolConfirmer::new(config.tools.auto_approve, config.tools.allow_list.clone());

        let session_manager = if config.session.enabled {
            Some(SessionManager::new(
                config.session.directory.clone().into(),
                config.session.max_sessions,
            ))
        } else {
            None
        };

        let allow_list = config.tools.allow_list.clone();
        let compact_config = config.compact.clone();

        Self {
            provider,
            tools,
            messages: session.messages.clone(),
            system_prompt,
            model: config.model.clone(),
            max_tokens: config.max_tokens,
            max_turns: config.max_turns,
            total_usage: session.total_usage.clone(),
            thinking: config.thinking,
            compat: config.compat.clone(),
            confirmer: Arc::new(Mutex::new(confirmer)),
            hooks: Some(HookEngine::new(config.hooks.clone())),
            session_manager,
            current_session: Some(session),
            output,
            current_msg_id: String::new(),
            approval_manager: None,
            protocol_writer: None,
            allow_list,
            current_reasoning_effort: None,
            compact_config,
            compact_state: CompactState::new(),
            plan_state: PlanState::default(),
            plan_active_flag: None,
            cache_detector: CacheBreakDetector::new(),
        }
    }

    /// Get a reference to the shared provider
    pub fn provider(&self) -> &Arc<dyn LlmProvider> {
        &self.provider
    }

    /// Get a reference to the resolved compat settings
    pub fn compat(&self) -> &aion_config::compat::ProviderCompat {
        &self.compat
    }

    pub fn tool_names(&self) -> Vec<String> {
        self.tools.tool_names()
    }

    pub fn registry_mut(&mut self) -> &mut ToolRegistry {
        &mut self.tools
    }

    /// Initialize a new session for this engine run
    pub fn init_session(
        &mut self,
        provider_name: &str,
        cwd: &str,
        session_id: Option<&str>,
    ) -> anyhow::Result<()> {
        if let Some(mgr) = &self.session_manager {
            let session = mgr.create(provider_name, &self.model, cwd, session_id)?;
            self.current_session = Some(session);
        }
        Ok(())
    }

    /// Get the current session ID (if sessions are enabled and initialized)
    pub fn current_session_id(&self) -> Option<String> {
        self.current_session.as_ref().map(|s| s.id.clone())
    }

    /// Get a reference to the output sink
    pub fn output(&self) -> &dyn OutputSink {
        self.output.as_ref()
    }

    pub fn set_approval_manager(&mut self, mgr: Arc<aion_protocol::ToolApprovalManager>) {
        self.approval_manager = Some(mgr);
    }

    pub fn set_protocol_writer(&mut self, writer: Arc<aion_protocol::writer::ProtocolWriter>) {
        self.protocol_writer = Some(writer);
    }

    /// Set the initial reasoning effort override (used by sub-agents spawned with an effort override).
    pub fn set_initial_reasoning_effort(&mut self, effort: Option<String>) {
        self.current_reasoning_effort = effort;
    }

    /// Set the shared plan-mode active flag.
    ///
    /// This flag is shared with EnterPlanMode/ExitPlanMode tools so they can
    /// validate transitions (e.g. reject double-entry).  The engine updates
    /// the flag when processing `PlanModeTransition` context modifiers.
    pub fn set_plan_active_flag(&mut self, flag: Arc<AtomicBool>) {
        self.plan_active_flag = Some(flag);
    }

    /// Default thinking budget when "enabled" is requested without a specific budget.
    const DEFAULT_THINKING_BUDGET: u32 = 10_000;

    /// Apply a runtime config update received from the protocol layer.
    ///
    /// Returns a list of human-readable change descriptions for the Info event.
    /// Empty list means no fields were changed.
    pub fn apply_config_update(
        &mut self,
        model: Option<String>,
        thinking: Option<String>,
        thinking_budget: Option<u32>,
        effort: Option<String>,
    ) -> Vec<String> {
        let mut changes = Vec::new();

        if let Some(new_model) = model {
            let old = std::mem::replace(&mut self.model, new_model.clone());
            changes.push(format!("model: {old} → {new_model}"));
        }

        if let Some(thinking_str) = thinking {
            if !self.compat.supports_thinking() {
                changes.push("thinking: not supported by current provider".to_string());
            } else {
                match thinking_str.as_str() {
                    "enabled" => {
                        let budget = thinking_budget.unwrap_or(Self::DEFAULT_THINKING_BUDGET);
                        self.thinking = Some(aion_types::llm::ThinkingConfig::Enabled {
                            budget_tokens: budget,
                        });
                        changes.push(format!("thinking: enabled (budget: {budget})"));
                    }
                    "disabled" => {
                        self.thinking = Some(aion_types::llm::ThinkingConfig::Disabled);
                        changes.push("thinking: disabled".to_string());
                    }
                    other => {
                        changes.push(format!("thinking: ignored invalid value \"{other}\""));
                    }
                }
            }
        } else if let Some(new_budget) = thinking_budget
            && let Some(aion_types::llm::ThinkingConfig::Enabled { budget_tokens }) =
                &mut self.thinking
        {
            *budget_tokens = new_budget;
            changes.push(format!("thinking budget: {new_budget}"));
        }

        if let Some(new_effort) = effort {
            if new_effort.is_empty() {
                self.current_reasoning_effort = None;
                changes.push("effort: cleared".to_string());
            } else if !self.compat.supports_effort() {
                changes.push("effort: not supported by current provider".to_string());
            } else {
                let levels = self.compat.effort_levels();
                if !levels.is_empty() && !levels.iter().any(|l| l == &new_effort) {
                    changes.push(format!(
                        "effort: invalid level \"{}\" (valid: {})",
                        new_effort,
                        levels.join(", ")
                    ));
                } else {
                    let old = self
                        .current_reasoning_effort
                        .replace(new_effort.clone())
                        .unwrap_or_else(|| "none".to_string());
                    changes.push(format!("effort: {old} → {new_effort}"));
                }
            }
        }

        changes
    }

    /// Run the agent loop with user input
    pub async fn run(&mut self, user_input: &str, msg_id: &str) -> Result<AgentResult, AgentError> {
        self.current_msg_id = msg_id.to_string();
        self.output.emit_stream_start(msg_id);
        self.messages.push(Message::now(
            Role::User,
            vec![ContentBlock::Text {
                text: user_input.to_string(),
            }],
        ));

        for turn in 0..self.max_turns {
            // Run multi-level compaction before each API call.
            // On the first turn last_input_tokens is 0 so neither
            // autocompact nor emergency will fire.
            self.run_compaction().await?;

            // Build tool list: filter based on plan mode state
            let tools = if self.plan_state.is_active {
                // Plan mode: only Info-category tools (excluding EnterPlanMode)
                self.tools.to_tool_defs_filtered(|t| {
                    t.category() == ToolCategory::Info && t.name() != "EnterPlanMode"
                })
            } else {
                // Normal mode: all tools except ExitPlanMode
                self.tools
                    .to_tool_defs_filtered(|t| t.name() != "ExitPlanMode")
            };

            // Build system prompt: append plan mode instructions when active
            let system = if self.plan_state.is_active {
                format!(
                    "{}\n\n{}",
                    self.system_prompt,
                    plan_prompt::plan_mode_instructions()
                )
            } else {
                self.system_prompt.clone()
            };

            // Record prompt state for cache diagnostics
            self.cache_detector.record_request(&system, &tools);

            let request = LlmRequest {
                model: self.model.clone(),
                system,
                messages: self.messages.clone(),
                tools,
                max_tokens: self.max_tokens,
                thinking: self.thinking.clone(),
                reasoning_effort: self.current_reasoning_effort.clone(),
            };

            let mut rx = self.provider.stream(&request).await?;
            let mut assistant_text = String::new();
            let mut thinking_text = String::new();
            let mut tool_calls: Vec<ContentBlock> = Vec::new();
            let mut stop_reason = StopReason::EndTurn;
            let mut turn_usage = TokenUsage::default();

            while let Some(event) = rx.recv().await {
                match event {
                    LlmEvent::TextDelta(text) => {
                        self.output.emit_text_delta(&text, &self.current_msg_id);
                        assistant_text.push_str(&text);
                    }
                    LlmEvent::ToolUse { id, name, input } => {
                        let input_str = serde_json::to_string(&input).unwrap_or_default();
                        self.output.emit_tool_call(&name, &input_str);
                        tool_calls.push(ContentBlock::ToolUse { id, name, input });
                    }
                    LlmEvent::ThinkingDelta(text) => {
                        self.output.emit_thinking(&text, &self.current_msg_id);
                        thinking_text.push_str(&text);
                    }
                    LlmEvent::Done {
                        stop_reason: sr,
                        usage,
                    } => {
                        stop_reason = sr;
                        turn_usage = usage;
                    }
                    LlmEvent::Error(e) => {
                        return Err(AgentError::ApiError(e));
                    }
                }
            }

            self.total_usage.input_tokens += turn_usage.input_tokens;
            self.total_usage.output_tokens += turn_usage.output_tokens;
            self.total_usage.cache_creation_tokens += turn_usage.cache_creation_tokens;
            self.total_usage.cache_read_tokens += turn_usage.cache_read_tokens;

            // Track per-turn input tokens for compaction watermark
            self.compact_state.last_input_tokens = turn_usage.input_tokens;

            // Cache break detection
            let cache_stats = CacheStats {
                input_tokens: turn_usage.input_tokens,
                cache_read_tokens: turn_usage.cache_read_tokens,
                cache_creation_tokens: turn_usage.cache_creation_tokens,
            };
            if let Some(diagnostic) = self.cache_detector.check_response(cache_stats) {
                match &diagnostic {
                    CacheDiagnostic::FullMiss { cause } => {
                        self.output
                            .emit_error(&format!("Cache full miss: {cause:?}"));
                    }
                    CacheDiagnostic::PartialMiss { hit_rate, cause } => {
                        if self.compact_config.cache_diagnostics {
                            self.output.emit_info(&format!(
                                "Cache: {:.0}% hit rate (cause: {cause:?})",
                                hit_rate * 100.0
                            ));
                        }
                    }
                    CacheDiagnostic::Healthy { hit_rate } => {
                        if self.compact_config.cache_diagnostics {
                            self.output
                                .emit_info(&format!("Cache: {:.0}% hit rate", hit_rate * 100.0));
                        }
                    }
                }
            }

            let mut assistant_content: Vec<ContentBlock> = Vec::new();
            if !thinking_text.is_empty() {
                assistant_content.push(ContentBlock::Thinking {
                    thinking: thinking_text,
                });
            }
            if !assistant_text.is_empty() {
                assistant_content.push(ContentBlock::Text {
                    text: assistant_text.clone(),
                });
            }
            assistant_content.extend(tool_calls.clone());

            self.messages
                .push(Message::now(Role::Assistant, assistant_content));

            if tool_calls.is_empty() {
                self.save_session();
                return Ok(AgentResult {
                    text: assistant_text,
                    stop_reason,
                    usage: self.total_usage.clone(),
                    turns: turn + 1,
                });
            }

            let outcome = if let Some(ref approval_mgr) = self.approval_manager {
                // JSON stream mode: use protocol-based approval
                let writer = self
                    .protocol_writer
                    .as_ref()
                    .expect("protocol writer required for approval");
                let auto_approve = self.confirmer.lock().unwrap().is_auto_approve();
                match execute_tool_calls_with_approval(
                    &self.tools,
                    &tool_calls,
                    approval_mgr,
                    writer,
                    &self.current_msg_id,
                    auto_approve,
                    &self.allow_list,
                    self.hooks.as_mut(),
                )
                .await
                {
                    Ok(o) => o,
                    Err(ExecutionControl::Quit) => {
                        self.save_session();
                        return Err(AgentError::UserAborted);
                    }
                }
            } else {
                // Terminal mode: use interactive confirmation
                match execute_tool_calls(
                    &self.tools,
                    &tool_calls,
                    &self.confirmer,
                    self.hooks.as_mut(),
                )
                .await
                {
                    Ok(o) => o,
                    Err(ExecutionControl::Quit) => {
                        self.save_session();
                        return Err(AgentError::UserAborted);
                    }
                }
            };

            // Apply any context modifiers from skill executions before the next turn
            self.apply_context_modifiers(&outcome.modifiers);

            // Display tool results
            for result in &outcome.results {
                if let ContentBlock::ToolResult {
                    content, is_error, ..
                } = result
                {
                    let tool_name = tool_calls
                        .iter()
                        .find_map(|c| {
                            if let ContentBlock::ToolUse { id, name, .. } = c
                                && let ContentBlock::ToolResult { tool_use_id, .. } = result
                                && id == tool_use_id
                            {
                                return Some(name.as_str());
                            }
                            None
                        })
                        .unwrap_or("unknown");
                    self.output.emit_tool_result(tool_name, *is_error, content);
                }
            }

            self.messages
                .push(Message::now(Role::User, outcome.results));

            // Save session after each turn
            self.save_session();
        }

        self.save_session();
        Err(AgentError::MaxTurnsExceeded(self.max_turns))
    }

    /// Run the multi-level compaction pipeline before each API call.
    ///
    /// Execution order: microcompact → autocompact → emergency check.
    /// After a successful autocompact the emergency check is skipped
    /// because the context has been significantly reduced.
    async fn run_compaction(&mut self) -> Result<(), AgentError> {
        // 1. Microcompact (lightweight, no LLM call)
        if micro::should_microcompact(&self.messages, &self.compact_config) {
            let result = micro::microcompact(&mut self.messages, &self.compact_config);
            if result.cleared_count > 0 {
                self.output.emit_info(&format!(
                    "Microcompact: cleared {} tool results (~{} tokens freed)",
                    result.cleared_count, result.estimated_tokens_freed
                ));
            }
        }

        // 2. Autocompact (LLM summarization)
        let mut compacted = false;
        if auto::should_autocompact(self.compact_state.last_input_tokens, &self.compact_config)
            && !self.compact_state.is_circuit_broken(&self.compact_config)
        {
            let provider = Arc::clone(&self.provider);
            match auto::autocompact(
                provider.as_ref(),
                &self.messages,
                &self.model,
                &self.compact_config,
                &mut self.compact_state,
            )
            .await
            {
                Ok(result) => {
                    self.output.emit_info(&format!(
                        "Autocompact: summarized {} messages ({} tokens → compact)",
                        result.messages_summarized, result.pre_compact_tokens
                    ));
                    self.messages = result.messages;
                    compacted = true;
                }
                Err(auto::CompactError::CircuitBroken { .. }) => {
                    // Already tripped; logged at circuit-breaker level
                }
                Err(e) => {
                    self.output
                        .emit_error(&format!("Autocompact failed: {}", e));
                }
            }
        }

        // 3. Emergency check (skip if autocompact just succeeded)
        if !compacted
            && emergency::is_at_emergency_limit(
                self.compact_state.last_input_tokens,
                &self.compact_config,
            )
        {
            return Err(AgentError::ContextTooLong {
                input_tokens: self.compact_state.last_input_tokens,
                limit: self
                    .compact_config
                    .context_window
                    .saturating_sub(self.compact_config.emergency_buffer),
            });
        }

        Ok(())
    }

    /// Run stop hooks when the agent session ends
    pub async fn run_stop_hooks(&self) {
        if let Some(hook_engine) = &self.hooks {
            let messages = hook_engine.run_stop().await;
            for msg in messages {
                eprintln!("{}", msg);
            }
        }
    }

    /// Apply context modifiers collected from skill tool executions.
    fn apply_context_modifiers(&mut self, modifiers: &[Option<ContextModifier>]) {
        for modifier in modifiers.iter().flatten() {
            if let Some(ref model) = modifier.model {
                self.model = model.clone();
            }
            if let Some(effort) = modifier.effort {
                self.current_reasoning_effort = Some(effort_to_string(effort));
            }
            for tool_name in &modifier.allowed_tools {
                if !self.allow_list.contains(tool_name) {
                    self.allow_list.push(tool_name.clone());
                }
                self.confirmer.lock().unwrap().add_to_allow_list(tool_name);
            }

            // Handle plan mode transitions
            if let Some(ref transition) = modifier.plan_mode_transition {
                match transition {
                    PlanModeTransition::Enter => {
                        self.plan_state.pre_plan_allow_list = self.allow_list.clone();
                        self.plan_state.is_active = true;
                        if let Some(ref flag) = self.plan_active_flag {
                            flag.store(true, Ordering::Release);
                        }
                    }
                    PlanModeTransition::Exit { .. } => {
                        self.plan_state.is_active = false;
                        self.allow_list = self.plan_state.pre_plan_allow_list.clone();
                        if let Some(ref flag) = self.plan_active_flag {
                            flag.store(false, Ordering::Release);
                        }
                    }
                }
            }
        }
    }

    fn save_session(&mut self) {
        if let (Some(mgr), Some(session)) = (&self.session_manager, &mut self.current_session) {
            session.messages = self.messages.clone();
            session.total_usage = self.total_usage.clone();
            session.updated_at = chrono::Utc::now();
            if let Err(e) = mgr.save(session) {
                self.output
                    .emit_error(&format!("Failed to save session: {}", e));
            }
            if let Err(e) = mgr.update_index_for(session) {
                self.output
                    .emit_error(&format!("Failed to update session index: {}", e));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// set_config tests — apply_config_update()
// ---------------------------------------------------------------------------

#[cfg(test)]
mod set_config_tests {
    use std::sync::{Arc, Mutex};

    use aion_providers::{LlmProvider, ProviderError};
    use aion_tools::registry::ToolRegistry;
    use aion_types::llm::{LlmEvent, LlmRequest};

    use crate::confirm::ToolConfirmer;
    use crate::output::OutputSink;

    struct NullOutput;
    impl OutputSink for NullOutput {
        fn emit_text_delta(&self, _: &str, _: &str) {}
        fn emit_thinking(&self, _: &str, _: &str) {}
        fn emit_tool_call(&self, _: &str, _: &str) {}
        fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
        fn emit_stream_start(&self, _: &str) {}
        fn emit_stream_end(&self, _: &str, _: usize, _: u64, _: u64, _: u64, _: u64) {}
        fn emit_error(&self, _: &str) {}
        fn emit_info(&self, _: &str) {}
    }

    struct NullProvider;
    #[async_trait::async_trait]
    impl LlmProvider for NullProvider {
        async fn stream(
            &self,
            _: &LlmRequest,
        ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            Ok(rx)
        }
    }

    fn make_engine(model: &str) -> super::AgentEngine {
        super::AgentEngine {
            provider: Arc::new(NullProvider),
            tools: ToolRegistry::new(),
            messages: vec![],
            system_prompt: String::new(),
            model: model.to_string(),
            max_tokens: 4096,
            max_turns: 10,
            total_usage: Default::default(),
            thinking: None,
            compat: aion_config::compat::ProviderCompat::anthropic_defaults(),
            confirmer: Arc::new(Mutex::new(ToolConfirmer::new(true, vec![]))),
            hooks: None,
            session_manager: None,
            current_session: None,
            output: Arc::new(NullOutput),
            current_msg_id: String::new(),
            approval_manager: None,
            protocol_writer: None,
            allow_list: vec![],
            current_reasoning_effort: None,
            compact_config: aion_config::compact::CompactConfig::default(),
            compact_state: super::CompactState::new(),
            plan_state: Default::default(),
            plan_active_flag: None,
            cache_detector: super::CacheBreakDetector::new(),
        }
    }

    fn make_engine_with_compat(
        model: &str,
        compat: aion_config::compat::ProviderCompat,
    ) -> super::AgentEngine {
        let mut engine = make_engine(model);
        engine.compat = compat;
        engine
    }

    // --- Cycle 1 tests (updated signature) ---

    #[test]
    fn set_config_changes_model() {
        let mut engine = make_engine("old-model");
        let changes = engine.apply_config_update(Some("new-model".into()), None, None, None);
        assert_eq!(engine.model, "new-model");
        assert_eq!(changes.len(), 1);
        assert!(changes[0].contains("old-model"));
        assert!(changes[0].contains("new-model"));
    }

    #[test]
    fn set_config_none_model_no_change() {
        let mut engine = make_engine("current");
        let changes = engine.apply_config_update(None, None, None, None);
        assert_eq!(engine.model, "current");
        assert!(changes.is_empty());
    }

    #[test]
    fn set_config_same_model_still_reports_change() {
        let mut engine = make_engine("same");
        let changes = engine.apply_config_update(Some("same".into()), None, None, None);
        assert_eq!(changes.len(), 1);
    }

    #[test]
    fn set_config_empty_string_model_accepted() {
        let mut engine = make_engine("real-model");
        engine.apply_config_update(Some(String::new()), None, None, None);
        assert_eq!(engine.model, "");
    }

    #[test]
    fn set_config_model_does_not_affect_other_state() {
        let mut engine = make_engine("m");
        engine.current_reasoning_effort = Some("high".into());
        engine.apply_config_update(Some("new-m".into()), None, None, None);
        assert_eq!(engine.model, "new-m");
        assert_eq!(engine.current_reasoning_effort.as_deref(), Some("high"));
    }

    // --- Cycle 2: Effort config tests ---

    #[test]
    fn set_config_changes_effort() {
        let mut engine =
            make_engine_with_compat("m", aion_config::compat::ProviderCompat::openai_defaults());
        assert!(engine.current_reasoning_effort.is_none());
        let changes = engine.apply_config_update(None, None, None, Some("high".into()));
        assert_eq!(engine.current_reasoning_effort.as_deref(), Some("high"));
        assert_eq!(changes.len(), 1);
        assert!(changes[0].contains("high"));
    }

    #[test]
    fn set_config_clears_effort_with_empty_string() {
        let mut engine = make_engine("m");
        engine.current_reasoning_effort = Some("high".into());
        let changes = engine.apply_config_update(None, None, None, Some(String::new()));
        assert!(engine.current_reasoning_effort.is_none());
        assert_eq!(changes.len(), 1);
    }

    // --- Cycle 2: Thinking config tests ---

    #[test]
    fn set_config_enables_thinking() {
        let mut engine = make_engine("m");
        let changes = engine.apply_config_update(None, Some("enabled".into()), Some(16000), None);
        match &engine.thinking {
            Some(aion_types::llm::ThinkingConfig::Enabled { budget_tokens }) => {
                assert_eq!(*budget_tokens, 16000);
            }
            other => panic!("expected Enabled, got: {other:?}"),
        }
        assert_eq!(changes.len(), 1);
    }

    #[test]
    fn set_config_disables_thinking() {
        let mut engine = make_engine("m");
        engine.thinking = Some(aion_types::llm::ThinkingConfig::Enabled {
            budget_tokens: 8000,
        });
        let changes = engine.apply_config_update(None, Some("disabled".into()), None, None);
        match &engine.thinking {
            Some(aion_types::llm::ThinkingConfig::Disabled) => {}
            other => panic!("expected Disabled, got: {other:?}"),
        }
        assert_eq!(changes.len(), 1);
    }

    #[test]
    fn set_config_thinking_enabled_default_budget() {
        let mut engine = make_engine("m");
        let changes = engine.apply_config_update(None, Some("enabled".into()), None, None);
        match &engine.thinking {
            Some(aion_types::llm::ThinkingConfig::Enabled { budget_tokens }) => {
                assert!(*budget_tokens > 0);
            }
            other => panic!("expected Enabled with default budget, got: {other:?}"),
        }
        assert_eq!(changes.len(), 1);
    }

    #[test]
    fn set_config_invalid_thinking_ignored() {
        let mut engine = make_engine("m");
        engine.thinking = Some(aion_types::llm::ThinkingConfig::Enabled {
            budget_tokens: 8000,
        });
        let changes = engine.apply_config_update(None, Some("invalid_value".into()), None, None);
        match &engine.thinking {
            Some(aion_types::llm::ThinkingConfig::Enabled { budget_tokens }) => {
                assert_eq!(*budget_tokens, 8000);
            }
            other => panic!("expected Enabled unchanged, got: {other:?}"),
        }
        assert_eq!(changes.len(), 1);
        assert!(changes[0].contains("invalid") || changes[0].contains("ignored"));
    }

    // --- Cycle 2: Combined fields test ---

    #[test]
    fn set_config_all_fields_at_once() {
        let compat = aion_config::compat::ProviderCompat {
            supports_thinking: Some(true),
            supports_effort: Some(true),
            effort_levels: Some(vec!["low".into()]),
            ..Default::default()
        };
        let mut engine = make_engine_with_compat("old-model", compat);
        let changes = engine.apply_config_update(
            Some("new-model".into()),
            Some("enabled".into()),
            Some(12000),
            Some("low".into()),
        );
        assert_eq!(engine.model, "new-model");
        assert_eq!(engine.current_reasoning_effort.as_deref(), Some("low"));
        match &engine.thinking {
            Some(aion_types::llm::ThinkingConfig::Enabled { budget_tokens }) => {
                assert_eq!(*budget_tokens, 12000);
            }
            other => panic!("expected Enabled, got: {other:?}"),
        }
        assert_eq!(changes.len(), 3);
    }

    // --- Cycle 2: White-box edge case tests ---

    #[test]
    fn set_config_thinking_budget_only_updates_existing_enabled() {
        let mut engine = make_engine("m");
        engine.thinking = Some(aion_types::llm::ThinkingConfig::Enabled {
            budget_tokens: 5000,
        });
        let changes = engine.apply_config_update(None, None, Some(20000), None);
        match &engine.thinking {
            Some(aion_types::llm::ThinkingConfig::Enabled { budget_tokens }) => {
                assert_eq!(*budget_tokens, 20000);
            }
            other => panic!("expected Enabled with 20000, got: {other:?}"),
        }
        assert_eq!(changes.len(), 1);
    }

    #[test]
    fn set_config_thinking_budget_ignored_when_disabled() {
        let mut engine = make_engine("m");
        engine.thinking = Some(aion_types::llm::ThinkingConfig::Disabled);
        let changes = engine.apply_config_update(None, None, Some(20000), None);
        match &engine.thinking {
            Some(aion_types::llm::ThinkingConfig::Disabled) => {}
            other => panic!("expected Disabled unchanged, got: {other:?}"),
        }
        assert!(changes.is_empty());
    }

    #[test]
    fn set_config_effort_valid_values() {
        let compat = aion_config::compat::ProviderCompat {
            supports_effort: Some(true),
            effort_levels: Some(vec![
                "low".into(),
                "medium".into(),
                "high".into(),
                "max".into(),
            ]),
            ..Default::default()
        };
        for value in ["low", "medium", "high", "max"] {
            let mut engine = make_engine_with_compat("m", compat.clone());
            engine.apply_config_update(None, None, None, Some(value.to_string()));
            assert_eq!(
                engine.current_reasoning_effort.as_deref(),
                Some(value),
                "effort should be set to {value}"
            );
        }
    }

    // --- Capability validation tests ---

    #[test]
    fn set_config_thinking_rejected_when_unsupported() {
        let mut engine =
            make_engine_with_compat("m", aion_config::compat::ProviderCompat::openai_defaults());
        let changes = engine.apply_config_update(None, Some("enabled".into()), None, None);
        assert!(changes.iter().any(|c| c.contains("not supported")));
        assert!(engine.thinking.is_none());
    }

    #[test]
    fn set_config_effort_rejected_when_unsupported() {
        let mut engine = make_engine("m"); // anthropic defaults: supports_effort = false
        let changes = engine.apply_config_update(None, None, None, Some("high".into()));
        assert!(changes.iter().any(|c| c.contains("not supported")));
        assert!(engine.current_reasoning_effort.is_none());
    }

    #[test]
    fn set_config_effort_rejected_invalid_level() {
        let mut engine =
            make_engine_with_compat("m", aion_config::compat::ProviderCompat::openai_defaults());
        let changes = engine.apply_config_update(None, None, None, Some("max".into()));
        assert!(changes.iter().any(|c| c.contains("invalid")));
        assert!(engine.current_reasoning_effort.is_none());
    }

    #[test]
    fn set_config_effort_clear_always_works() {
        let mut engine = make_engine("m"); // anthropic defaults: supports_effort = false
        engine.current_reasoning_effort = Some("high".into());
        let changes = engine.apply_config_update(None, None, None, Some(String::new()));
        assert!(engine.current_reasoning_effort.is_none());
        assert!(changes.iter().any(|c| c.contains("cleared")));
    }
}

// ---------------------------------------------------------------------------
// Phase 6 tests — apply_context_modifiers()
// ---------------------------------------------------------------------------

#[cfg(test)]
mod phase6_tests {
    use std::sync::{Arc, Mutex};

    use aion_providers::{LlmProvider, ProviderError};
    use aion_tools::registry::ToolRegistry;
    use aion_types::llm::{LlmEvent, LlmRequest};
    use aion_types::skill_types::{ContextModifier, EffortLevel};

    use crate::confirm::ToolConfirmer;
    use crate::output::OutputSink;

    struct NullOutput;
    impl OutputSink for NullOutput {
        fn emit_text_delta(&self, _: &str, _: &str) {}
        fn emit_thinking(&self, _: &str, _: &str) {}
        fn emit_tool_call(&self, _: &str, _: &str) {}
        fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
        fn emit_stream_start(&self, _: &str) {}
        fn emit_stream_end(&self, _: &str, _: usize, _: u64, _: u64, _: u64, _: u64) {}
        fn emit_error(&self, _: &str) {}
        fn emit_info(&self, _: &str) {}
    }

    struct NullProvider;
    #[async_trait::async_trait]
    impl LlmProvider for NullProvider {
        async fn stream(
            &self,
            _: &LlmRequest,
        ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            Ok(rx)
        }
    }

    fn make_engine(model: &str, allow_list: Vec<String>) -> super::AgentEngine {
        super::AgentEngine {
            provider: Arc::new(NullProvider),
            tools: ToolRegistry::new(),
            messages: vec![],
            system_prompt: String::new(),
            model: model.to_string(),
            max_tokens: 4096,
            max_turns: 10,
            total_usage: Default::default(),
            thinking: None,
            compat: aion_config::compat::ProviderCompat::anthropic_defaults(),
            confirmer: Arc::new(Mutex::new(ToolConfirmer::new(true, allow_list.clone()))),
            hooks: None,
            session_manager: None,
            current_session: None,
            output: Arc::new(NullOutput),
            current_msg_id: String::new(),
            approval_manager: None,
            protocol_writer: None,
            allow_list,
            current_reasoning_effort: None,
            compact_config: aion_config::compact::CompactConfig::default(),
            compact_state: super::CompactState::new(),
            plan_state: Default::default(),
            plan_active_flag: None,
            cache_detector: super::CacheBreakDetector::new(),
        }
    }

    #[test]
    fn tc_6_21_model_override_applied() {
        let mut engine = make_engine("original-model", vec![]);
        let modifiers = vec![Some(ContextModifier {
            model: Some("override-model".to_string()),
            ..Default::default()
        })];
        engine.apply_context_modifiers(&modifiers);
        assert_eq!(engine.model, "override-model");
    }

    #[test]
    fn tc_6_22_effort_override_applied() {
        let mut engine = make_engine("m", vec![]);
        let modifiers = vec![Some(ContextModifier {
            effort: Some(EffortLevel::High),
            ..Default::default()
        })];
        engine.apply_context_modifiers(&modifiers);
        assert_eq!(engine.current_reasoning_effort.as_deref(), Some("high"));
    }

    #[test]
    fn tc_6_22b_effort_all_variants() {
        for (level, expected) in [
            (EffortLevel::Low, "low"),
            (EffortLevel::Medium, "medium"),
            (EffortLevel::High, "high"),
            (EffortLevel::Max, "max"),
        ] {
            let mut engine = make_engine("m", vec![]);
            engine.apply_context_modifiers(&[Some(ContextModifier {
                effort: Some(level),
                ..Default::default()
            })]);
            assert_eq!(
                engine.current_reasoning_effort.as_deref(),
                Some(expected),
                "EffortLevel::{level:?} should map to {expected:?}"
            );
        }
    }

    #[test]
    fn tc_6_23_allowed_tools_no_duplicates() {
        let mut engine = make_engine("m", vec!["Bash".to_string()]);
        let modifiers = vec![Some(ContextModifier {
            allowed_tools: vec!["Bash".to_string(), "Read".to_string()],
            ..Default::default()
        })];
        engine.apply_context_modifiers(&modifiers);
        let bash_count = engine
            .allow_list
            .iter()
            .filter(|t| t.as_str() == "Bash")
            .count();
        assert_eq!(bash_count, 1, "Bash should appear exactly once");
        assert!(engine.allow_list.contains(&"Read".to_string()));
    }

    #[test]
    fn tc_6_24_none_modifiers_skipped() {
        let mut engine = make_engine("original", vec![]);
        engine.apply_context_modifiers(&[None, None]);
        assert_eq!(engine.model, "original");
        assert!(engine.current_reasoning_effort.is_none());
    }

    #[test]
    fn tc_6_25_empty_modifiers_no_change() {
        let mut engine = make_engine("current-model", vec![]);
        engine.apply_context_modifiers(&[]);
        assert_eq!(engine.model, "current-model");
        assert!(engine.allow_list.is_empty());
    }

    #[test]
    fn tc_6_26_none_model_does_not_overwrite() {
        let mut engine = make_engine("current-model", vec![]);
        engine.apply_context_modifiers(&[Some(ContextModifier {
            allowed_tools: vec!["Bash".to_string()],
            ..Default::default()
        })]);
        assert_eq!(engine.model, "current-model");
        assert!(engine.allow_list.contains(&"Bash".to_string()));
    }

    #[test]
    fn tc_6_27_multiple_modifiers_stacked() {
        let mut engine = make_engine("initial", vec![]);
        let modifiers = vec![
            Some(ContextModifier {
                model: Some("model-a".to_string()),
                allowed_tools: vec!["Bash".to_string()],
                ..Default::default()
            }),
            Some(ContextModifier {
                model: Some("model-b".to_string()),
                allowed_tools: vec!["Read".to_string()],
                ..Default::default()
            }),
        ];
        engine.apply_context_modifiers(&modifiers);
        assert_eq!(engine.model, "model-b", "last model wins");
        assert!(engine.allow_list.contains(&"Bash".to_string()));
        assert!(engine.allow_list.contains(&"Read".to_string()));
    }

    #[test]
    fn tc_6_28_modifier_applied_after_tool_execution_not_during() {
        let mut engine = make_engine("original", vec![]);
        let model_before = engine.model.clone();
        let modifiers = vec![Some(ContextModifier {
            model: Some("new-model".to_string()),
            ..Default::default()
        })];
        assert_eq!(engine.model, model_before);
        engine.apply_context_modifiers(&modifiers);
        assert_eq!(engine.model, "new-model");
        assert_eq!(model_before, "original");
    }
}

// ---------------------------------------------------------------------------
// Phase 2 tests — run_compaction()
// ---------------------------------------------------------------------------

#[cfg(test)]
mod compact_tests {
    use std::sync::{Arc, Mutex};

    use aion_config::compact::CompactConfig;
    use aion_providers::{LlmProvider, ProviderError};
    use aion_tools::registry::ToolRegistry;
    use aion_types::llm::{LlmEvent, LlmRequest};
    use aion_types::message::{ContentBlock, Message, Role};
    use serde_json::json;

    use crate::compact::state::CompactState;
    use crate::confirm::ToolConfirmer;
    use crate::output::OutputSink;

    struct NullOutput;
    impl OutputSink for NullOutput {
        fn emit_text_delta(&self, _: &str, _: &str) {}
        fn emit_thinking(&self, _: &str, _: &str) {}
        fn emit_tool_call(&self, _: &str, _: &str) {}
        fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
        fn emit_stream_start(&self, _: &str) {}
        fn emit_stream_end(&self, _: &str, _: usize, _: u64, _: u64, _: u64, _: u64) {}
        fn emit_error(&self, _: &str) {}
        fn emit_info(&self, _: &str) {}
    }

    struct NullProvider;
    #[async_trait::async_trait]
    impl LlmProvider for NullProvider {
        async fn stream(
            &self,
            _: &LlmRequest,
        ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            Ok(rx)
        }
    }

    fn make_compact_engine(
        compact_config: CompactConfig,
        compact_state: CompactState,
        messages: Vec<Message>,
    ) -> super::AgentEngine {
        super::AgentEngine {
            provider: Arc::new(NullProvider),
            tools: ToolRegistry::new(),
            messages,
            system_prompt: String::new(),
            model: "test-model".to_string(),
            max_tokens: 4096,
            max_turns: 10,
            total_usage: Default::default(),
            thinking: None,
            compat: aion_config::compat::ProviderCompat::anthropic_defaults(),
            confirmer: Arc::new(Mutex::new(ToolConfirmer::new(true, vec![]))),
            hooks: None,
            session_manager: None,
            current_session: None,
            output: Arc::new(NullOutput),
            current_msg_id: String::new(),
            approval_manager: None,
            protocol_writer: None,
            allow_list: vec![],
            current_reasoning_effort: None,
            compact_config,
            compact_state,
            plan_state: Default::default(),
            plan_active_flag: None,
            cache_detector: super::CacheBreakDetector::new(),
        }
    }

    fn tool_use_msg(id: &str, name: &str) -> Message {
        Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: id.to_string(),
                name: name.to_string(),
                input: json!({}),
            }],
        )
    }

    fn tool_result_msg(id: &str, content: &str) -> Message {
        Message::new(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content: content.to_string(),
                is_error: false,
            }],
        )
    }

    // -- Emergency check fires when at limit --

    #[tokio::test]
    async fn emergency_fires_when_at_limit() {
        let config = CompactConfig {
            context_window: 200_000,
            emergency_buffer: 3_000,
            ..Default::default()
        };
        let mut state = CompactState::new();
        state.last_input_tokens = 198_000; // >= 197k limit

        let mut engine = make_compact_engine(config, state, vec![]);
        let result = engine.run_compaction().await;

        match result {
            Err(super::AgentError::ContextTooLong {
                input_tokens,
                limit,
            }) => {
                assert_eq!(input_tokens, 198_000);
                assert_eq!(limit, 197_000);
            }
            other => panic!("expected ContextTooLong, got: {:?}", other),
        }
    }

    // -- Emergency does not fire when below limit --

    #[tokio::test]
    async fn emergency_silent_below_limit() {
        let config = CompactConfig::default();
        let mut state = CompactState::new();
        state.last_input_tokens = 190_000; // below 197k

        let mut engine = make_compact_engine(config, state, vec![]);
        assert!(engine.run_compaction().await.is_ok());
    }

    // -- Microcompact runs when count trigger fires --

    #[tokio::test]
    async fn microcompact_clears_old_results() {
        // 12 tool results with keep_recent=3 (threshold=6) → should clear 9
        let mut messages = Vec::new();
        for i in 0..12 {
            let id = format!("t{i}");
            messages.push(tool_use_msg(&id, "Read"));
            messages.push(tool_result_msg(&id, &format!("data-{i}")));
        }

        let config = CompactConfig {
            micro_keep_recent: 3,
            ..Default::default()
        };
        let state = CompactState::new();

        let mut engine = make_compact_engine(config, state, messages);
        engine.run_compaction().await.unwrap();

        // Last 3 tool results should be preserved
        let cleared_count = engine
            .messages
            .iter()
            .flat_map(|m| &m.content)
            .filter(|b| {
                matches!(b, ContentBlock::ToolResult { content, .. } if content == "[Tool result cleared]")
            })
            .count();

        assert_eq!(cleared_count, 9);
    }

    // -- Disabled config skips micro and auto but not emergency --

    #[tokio::test]
    async fn disabled_config_skips_micro_auto() {
        let mut messages = Vec::new();
        for i in 0..12 {
            let id = format!("t{i}");
            messages.push(tool_use_msg(&id, "Read"));
            messages.push(tool_result_msg(&id, &format!("data-{i}")));
        }

        let config = CompactConfig {
            enabled: false,
            micro_keep_recent: 3,
            ..Default::default()
        };
        let state = CompactState::new();

        let mut engine = make_compact_engine(config, state, messages);
        engine.run_compaction().await.unwrap();

        // Nothing should be cleared (microcompact skipped)
        let cleared_count = engine
            .messages
            .iter()
            .flat_map(|m| &m.content)
            .filter(|b| {
                matches!(b, ContentBlock::ToolResult { content, .. } if content == "[Tool result cleared]")
            })
            .count();

        assert_eq!(
            cleared_count, 0,
            "microcompact should be skipped when disabled"
        );
    }

    #[tokio::test]
    async fn disabled_config_still_fires_emergency() {
        let config = CompactConfig {
            enabled: false,
            context_window: 200_000,
            emergency_buffer: 3_000,
            ..Default::default()
        };
        let mut state = CompactState::new();
        state.last_input_tokens = 198_000;

        let mut engine = make_compact_engine(config, state, vec![]);
        let result = engine.run_compaction().await;

        assert!(
            matches!(result, Err(super::AgentError::ContextTooLong { .. })),
            "emergency should fire even when disabled"
        );
    }

    // -- Zero tokens on first turn does not trigger anything --

    #[tokio::test]
    async fn first_turn_zero_tokens_no_compaction() {
        let config = CompactConfig::default();
        let state = CompactState::new(); // last_input_tokens = 0

        let mut engine = make_compact_engine(config, state, vec![]);
        assert!(engine.run_compaction().await.is_ok());
        assert_eq!(engine.compact_state.last_input_tokens, 0);
    }

    // -- Circuit broken prevents autocompact, emergency still fires --

    #[tokio::test]
    async fn circuit_broken_skips_auto_but_emergency_fires() {
        let config = CompactConfig {
            context_window: 200_000,
            emergency_buffer: 3_000,
            max_failures: 3,
            ..Default::default()
        };
        let mut state = CompactState::new();
        state.last_input_tokens = 198_000; // triggers both auto and emergency
        state.consecutive_failures = 3; // circuit broken

        let mut engine = make_compact_engine(config, state, vec![]);
        let result = engine.run_compaction().await;

        // Auto is skipped due to circuit breaker; emergency fires
        assert!(matches!(
            result,
            Err(super::AgentError::ContextTooLong { .. })
        ));
    }
}

// ---------------------------------------------------------------------------
// Phase 3 tests — plan mode integration in apply_context_modifiers()
// ---------------------------------------------------------------------------

#[cfg(test)]
mod plan_mode_tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use aion_providers::{LlmProvider, ProviderError};
    use aion_tools::registry::ToolRegistry;
    use aion_types::llm::{LlmEvent, LlmRequest};
    use aion_types::skill_types::{ContextModifier, PlanModeTransition};

    use crate::compact::state::CompactState;
    use crate::confirm::ToolConfirmer;
    use crate::output::OutputSink;
    use crate::plan::state::PlanState;

    struct NullOutput;
    impl OutputSink for NullOutput {
        fn emit_text_delta(&self, _: &str, _: &str) {}
        fn emit_thinking(&self, _: &str, _: &str) {}
        fn emit_tool_call(&self, _: &str, _: &str) {}
        fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
        fn emit_stream_start(&self, _: &str) {}
        fn emit_stream_end(&self, _: &str, _: usize, _: u64, _: u64, _: u64, _: u64) {}
        fn emit_error(&self, _: &str) {}
        fn emit_info(&self, _: &str) {}
    }

    struct NullProvider;
    #[async_trait::async_trait]
    impl LlmProvider for NullProvider {
        async fn stream(
            &self,
            _: &LlmRequest,
        ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            Ok(rx)
        }
    }

    fn make_plan_engine(allow_list: Vec<String>) -> super::AgentEngine {
        let flag = Arc::new(AtomicBool::new(false));
        super::AgentEngine {
            provider: Arc::new(NullProvider),
            tools: ToolRegistry::new(),
            messages: vec![],
            system_prompt: String::new(),
            model: "test-model".to_string(),
            max_tokens: 4096,
            max_turns: 10,
            total_usage: Default::default(),
            thinking: None,
            compat: aion_config::compat::ProviderCompat::anthropic_defaults(),
            confirmer: Arc::new(Mutex::new(ToolConfirmer::new(true, allow_list.clone()))),
            hooks: None,
            session_manager: None,
            current_session: None,
            output: Arc::new(NullOutput),
            current_msg_id: String::new(),
            approval_manager: None,
            protocol_writer: None,
            allow_list,
            current_reasoning_effort: None,
            compact_config: aion_config::compact::CompactConfig::default(),
            compact_state: CompactState::new(),
            plan_state: PlanState::default(),
            plan_active_flag: Some(flag),
            cache_detector: super::CacheBreakDetector::new(),
        }
    }

    // --- TC-3.5-03: Enter transition activates plan mode ---

    #[test]
    fn enter_transition_activates_plan_mode() {
        let mut engine = make_plan_engine(vec!["Read".into(), "Bash".into()]);
        let modifiers = vec![Some(ContextModifier {
            plan_mode_transition: Some(PlanModeTransition::Enter),
            ..Default::default()
        })];

        engine.apply_context_modifiers(&modifiers);

        assert!(engine.plan_state.is_active, "plan mode should be active");
        assert_eq!(
            engine.plan_state.pre_plan_allow_list,
            vec!["Read".to_string(), "Bash".to_string()],
            "pre_plan_allow_list should capture original allow_list"
        );
    }

    // --- TC-3.5-03 supplement: shared flag updated on enter ---

    #[test]
    fn enter_transition_updates_shared_flag() {
        let mut engine = make_plan_engine(vec![]);
        let flag = engine.plan_active_flag.clone().unwrap();
        assert!(!flag.load(Ordering::Acquire));

        engine.apply_context_modifiers(&[Some(ContextModifier {
            plan_mode_transition: Some(PlanModeTransition::Enter),
            ..Default::default()
        })]);

        assert!(flag.load(Ordering::Acquire), "shared flag should be true");
    }

    // --- TC-3.5-04: Exit transition deactivates plan mode and restores allow_list ---

    #[test]
    fn exit_transition_deactivates_and_restores() {
        let mut engine = make_plan_engine(vec!["Read".into(), "Bash".into()]);

        // Enter plan mode first
        engine.apply_context_modifiers(&[Some(ContextModifier {
            plan_mode_transition: Some(PlanModeTransition::Enter),
            ..Default::default()
        })]);
        assert!(engine.plan_state.is_active);

        // Modify allow_list while in plan mode (simulating a skill adding tools)
        engine.allow_list.push("NewTool".into());

        // Exit plan mode
        engine.apply_context_modifiers(&[Some(ContextModifier {
            plan_mode_transition: Some(PlanModeTransition::Exit { plan_content: None }),
            ..Default::default()
        })]);

        assert!(!engine.plan_state.is_active, "plan mode should be inactive");
        assert_eq!(
            engine.allow_list,
            vec!["Read".to_string(), "Bash".to_string()],
            "allow_list should be restored to pre-plan state"
        );
    }

    // --- TC-3.5-04 supplement: shared flag updated on exit ---

    #[test]
    fn exit_transition_updates_shared_flag() {
        let mut engine = make_plan_engine(vec![]);
        let flag = engine.plan_active_flag.clone().unwrap();

        // Enter
        engine.apply_context_modifiers(&[Some(ContextModifier {
            plan_mode_transition: Some(PlanModeTransition::Enter),
            ..Default::default()
        })]);
        assert!(flag.load(Ordering::Acquire));

        // Exit
        engine.apply_context_modifiers(&[Some(ContextModifier {
            plan_mode_transition: Some(PlanModeTransition::Exit { plan_content: None }),
            ..Default::default()
        })]);
        assert!(
            !flag.load(Ordering::Acquire),
            "shared flag should be false after exit"
        );
    }

    // --- TC-3.5-05: No transition does not affect plan state ---

    #[test]
    fn no_transition_does_not_affect_plan_state() {
        let mut engine = make_plan_engine(vec![]);

        engine.apply_context_modifiers(&[Some(ContextModifier {
            model: Some("new-model".into()),
            plan_mode_transition: None,
            ..Default::default()
        })]);

        assert_eq!(engine.model, "new-model");
        assert!(
            !engine.plan_state.is_active,
            "plan state should remain inactive"
        );
    }

    // --- Enter + other modifiers applied together ---

    #[test]
    fn enter_with_model_override_both_applied() {
        let mut engine = make_plan_engine(vec![]);

        engine.apply_context_modifiers(&[Some(ContextModifier {
            model: Some("planning-model".into()),
            plan_mode_transition: Some(PlanModeTransition::Enter),
            ..Default::default()
        })]);

        assert!(engine.plan_state.is_active);
        assert_eq!(engine.model, "planning-model");
    }

    // --- No plan_active_flag set does not panic ---

    #[test]
    fn enter_without_flag_does_not_panic() {
        let mut engine = make_plan_engine(vec![]);
        engine.plan_active_flag = None;

        engine.apply_context_modifiers(&[Some(ContextModifier {
            plan_mode_transition: Some(PlanModeTransition::Enter),
            ..Default::default()
        })]);

        assert!(engine.plan_state.is_active);
    }
}

#[derive(Debug)]
pub struct AgentResult {
    pub text: String,
    pub stop_reason: StopReason,
    pub usage: TokenUsage,
    pub turns: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("API error: {0}")]
    ApiError(String),
    #[error("Provider error: {0}")]
    Provider(#[from] ProviderError),
    #[error("Max turns ({0}) exceeded")]
    MaxTurnsExceeded(usize),
    #[error("User aborted the session")]
    UserAborted,
    #[error("Context window nearly full ({input_tokens} tokens used, limit {limit})")]
    ContextTooLong { input_tokens: u64, limit: usize },
}
