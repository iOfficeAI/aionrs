use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::cache_diagnostics::{CacheBreakDetector, CacheDiagnostic, CacheStats};
use crate::compact::state::CompactState;
use crate::compact::{auto, emergency, estimate, micro};
use crate::confirm::ToolConfirmer;
use crate::error::AgentError;
use crate::orchestration::{
    ExecutionControl, execute_tool_calls, execute_tool_calls_with_approval,
};
use crate::output::OutputSink;
use crate::plan::prompt as plan_prompt;
use crate::plan::state::PlanState;
use crate::session::{Session, SessionManager};
use crate::tool_call::{
    DEFAULT_MAX_MALFORMED_TOOL_CALL_TURNS, RepeatedMalformedToolCallTracker,
    malformed_only_fingerprint, reason as malformed_tool_call_reason,
    synthetic_result as synthetic_malformed_tool_result,
};
use aion_config::compact::CompactConfig;
use aion_config::config::Config;
use aion_config::hooks::HookEngine;
use aion_protocol::events::ToolCategory;
use aion_providers::provider::{LlmProvider, create_provider};
use aion_tools::registry::ToolRegistry;
use aion_types::llm::{LlmEvent, LlmRequest, ThinkingConfig};
use aion_types::message::{ContentBlock, Message, Role, StopReason, TokenUsage};
use aion_types::skill_types::{ContextModifier, PlanModeTransition, effort_to_string};
use tracing::Instrument;

const DEFAULT_MAX_CONSECUTIVE_TOOL_ERROR_TURNS: usize = 3;

#[derive(Debug)]
pub struct AgentResult {
    pub text: String,
    pub stop_reason: StopReason,
    pub usage: TokenUsage,
    pub turns: usize,
}

pub struct AgentEngine {
    provider: Arc<dyn LlmProvider>,
    tools: ToolRegistry,
    messages: Vec<Message>,
    system_prompt: String,
    model: String,
    max_tokens: u32,
    max_turns: Option<usize>,
    max_malformed_tool_call_turns: usize,
    total_usage: TokenUsage,
    thinking: Option<ThinkingConfig>,
    /// Resolved provider compat settings (for capability validation)
    compat: aion_config::compat::ProviderCompat,
    confirmer: Arc<Mutex<ToolConfirmer>>,
    hooks: Option<HookEngine>,
    session_manager: Option<SessionManager>,
    current_session: Option<Session>,
    output: Arc<dyn OutputSink>,
    current_msg_id: String,
    approval_manager: Option<Arc<aion_protocol::ToolApprovalManager>>,
    protocol_writer: Option<Arc<dyn aion_protocol::writer::ProtocolEmitter>>,
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
    compaction_level: aion_compact::CompactionLevel,
    toon_enabled: bool,
    commands: crate::commands::CommandRegistry,
}

impl AgentEngine {
    pub fn new(
        config: Config,
        tools: ToolRegistry,
        output: Arc<dyn OutputSink>,
        cwd: PathBuf,
    ) -> Self {
        let provider = create_provider(&config);
        Self::new_with_provider(provider, config, tools, output, cwd)
    }

    /// Create an engine with an externally-provided provider (for sub-agent sharing)
    pub fn new_with_provider(
        provider: Arc<dyn LlmProvider>,
        config: Config,
        tools: ToolRegistry,
        output: Arc<dyn OutputSink>,
        cwd: PathBuf,
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
            max_malformed_tool_call_turns: config
                .max_malformed_tool_call_turns
                .unwrap_or(DEFAULT_MAX_MALFORMED_TOOL_CALL_TURNS),
            total_usage: TokenUsage::default(),
            thinking: config.thinking,
            compat: config.compat.clone(),
            confirmer: Arc::new(Mutex::new(confirmer)),
            hooks: Some(HookEngine::new(config.hooks.clone(), cwd.clone())),
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
            compaction_level: config.compact.compaction,
            toon_enabled: config.compact.toon,
            commands: crate::commands::default_registry(),
        }
    }

    /// Create from a resumed session
    pub fn resume(
        config: Config,
        tools: ToolRegistry,
        output: Arc<dyn OutputSink>,
        session: Session,
        cwd: PathBuf,
    ) -> Self {
        let provider = create_provider(&config);
        Self::resume_with_provider(provider, config, tools, output, session, cwd)
    }

    /// Create from a resumed session with an externally-provided provider
    pub fn resume_with_provider(
        provider: Arc<dyn LlmProvider>,
        config: Config,
        tools: ToolRegistry,
        output: Arc<dyn OutputSink>,
        session: Session,
        cwd: PathBuf,
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
            max_malformed_tool_call_turns: config
                .max_malformed_tool_call_turns
                .unwrap_or(DEFAULT_MAX_MALFORMED_TOOL_CALL_TURNS),
            total_usage: session.total_usage.clone(),
            thinking: config.thinking,
            compat: config.compat.clone(),
            confirmer: Arc::new(Mutex::new(confirmer)),
            hooks: Some(HookEngine::new(config.hooks.clone(), cwd)),
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
            compaction_level: config.compact.compaction,
            toon_enabled: config.compact.toon,
            commands: crate::commands::default_registry(),
        }
    }

    pub fn compaction_level(&self) -> aion_compact::CompactionLevel {
        self.compaction_level
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

    pub fn set_protocol_writer(&mut self, writer: Arc<dyn aion_protocol::writer::ProtocolEmitter>) {
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
}

impl AgentEngine {
    /// Run the agent loop with user input
    pub async fn run(&mut self, user_input: &str, msg_id: &str) -> Result<AgentResult, AgentError> {
        let session_id = self
            .current_session
            .as_ref()
            .map(|s| s.id.clone())
            .unwrap_or_default();
        let span = tracing::info_span!(
            target: "aion_agent",
            "agent_run",
            session_id = %session_id,
            msg_id = %msg_id,
        );
        self.run_inner(user_input, msg_id).instrument(span).await
    }

    async fn run_inner(
        &mut self,
        user_input: &str,
        msg_id: &str,
    ) -> Result<AgentResult, AgentError> {
        // Slash command interception — before any LLM call
        if let Some(result) = self.handle_command(user_input).await {
            let cmd_name = user_input.split_whitespace().next().unwrap_or(user_input);
            return match result {
                Ok(crate::commands::CommandResult::Continue) => {
                    tracing::info!(command = cmd_name, "Slash command executed");
                    Ok(AgentResult {
                        text: String::new(),
                        stop_reason: StopReason::EndTurn,
                        usage: TokenUsage::default(),
                        turns: 0,
                    })
                }
                Ok(crate::commands::CommandResult::Exit) => {
                    tracing::info!(command = cmd_name, "Slash command executed: exit");
                    Err(AgentError::UserAborted)
                }
                Err(e) => {
                    tracing::error!(command = cmd_name, error = %e, "Slash command failed");
                    Err(AgentError::ApiError(e.to_string()))
                }
            };
        }

        self.current_msg_id = msg_id.to_string();
        self.output.emit_stream_start(msg_id);
        self.messages.push(Message::now(
            Role::User,
            vec![ContentBlock::Text {
                text: user_input.to_string(),
            }],
        ));

        let mut turn: usize = 0;
        let mut repeated_malformed_tool_calls = RepeatedMalformedToolCallTracker::default();
        let mut consecutive_tool_error_turns: usize = 0;
        loop {
            if let Some(limit) = self.max_turns && turn >= limit {
                self.save_session();
                let message = format!(
                    "Stopped after reaching max_turns ({limit}); the task did not converge. Try adjusting the request or retrying."
                );
                tracing::warn!(target: "aion_agent", limit, "stopping agent loop at max_turns");
                self.output.emit_error(&message);
                return Ok(AgentResult {
                    text: String::new(),
                    stop_reason: StopReason::MaxTurns,
                    usage: self.total_usage.clone(),
                    turns: turn,
                });
            }
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
            let mut thinking_signature: Option<String> = None;
            let mut tool_calls: Vec<ContentBlock> = Vec::new();
            let mut stop_reason = StopReason::EndTurn;
            let mut turn_usage = TokenUsage::default();

            while let Some(event) = rx.recv().await {
                match event {
                    LlmEvent::TextDelta(text) => {
                        self.output.emit_text_delta(&text, &self.current_msg_id);
                        assistant_text.push_str(&text);
                    }
                    LlmEvent::ToolUse {
                        id,
                        name,
                        input,
                        extra,
                    } => {
                        if id.trim().is_empty() {
                            tracing::error!(
                                target: "aion_agent",
                                tool = %name,
                                "provider emitted tool call with empty tool_use_id"
                            );
                        } else {
                            tracing::debug!(
                                target: "aion_agent",
                                tool_use_id = %id,
                                tool = %name,
                                "provider tool call received"
                            );
                        }
                        let input_str = serde_json::to_string(&input).unwrap_or_default();
                        self.output.emit_tool_call(&id, &name, &input_str);
                        tool_calls.push(ContentBlock::ToolUse {
                            id,
                            name,
                            input,
                            extra,
                        });
                    }
                    LlmEvent::ThinkingDelta(text) => {
                        self.output.emit_thinking(&text, &self.current_msg_id);
                        thinking_text.push_str(&text);
                    }
                    LlmEvent::ThinkingSignature(signature) => {
                        thinking_signature = Some(signature);
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

            // Track per-turn input tokens for compaction watermark.
            // Use max(provider_reported, local_estimate) as a safety net:
            // some providers (e.g. DeepSeek with prefix caching) underreport
            // prompt_tokens, causing compaction to never trigger.
            let local_estimate = estimate::estimate_tokens_from_messages(&self.messages);
            let effective_watermark = turn_usage.input_tokens.max(local_estimate);

            if local_estimate > turn_usage.input_tokens
                && local_estimate.saturating_sub(turn_usage.input_tokens) > 10_000
            {
                self.output.emit_info(&format!(
                    "Token watermark override: provider={}, local_estimate={}, using={}",
                    turn_usage.input_tokens, local_estimate, effective_watermark
                ));
            }

            self.compact_state.last_input_tokens = effective_watermark;

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
            if !thinking_text.is_empty() || thinking_signature.is_some() {
                assistant_content.push(ContentBlock::Thinking {
                    thinking: thinking_text,
                    signature: thinking_signature,
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

            let malformed_reasons: Vec<_> = tool_calls
                .iter()
                .map(|call| {
                    let ContentBlock::ToolUse { id, name, .. } = call else {
                        return None;
                    };
                    malformed_tool_call_reason(id, name)
                })
                .collect();
            let malformed_only_fingerprint = malformed_only_fingerprint(&tool_calls, &malformed_reasons);
            let executable_tool_calls: Vec<_> = tool_calls
                .iter()
                .zip(&malformed_reasons)
                .filter_map(|(call, reason)| {
                    if reason.is_none() {
                        Some(call.clone())
                    } else {
                        None
                    }
                })
                .collect();

            let (executable_results, executable_modifiers) = if executable_tool_calls.is_empty() {
                (Vec::new(), Vec::new())
            } else if let Some(ref approval_mgr) = self.approval_manager {
                // JSON stream mode: use protocol-based approval
                let writer = self
                    .protocol_writer
                    .as_ref()
                    .expect("protocol writer required for approval");
                let auto_approve = self.confirmer.lock().unwrap().is_auto_approve();
                match execute_tool_calls_with_approval(
                    &self.tools,
                    &executable_tool_calls,
                    approval_mgr,
                    writer,
                    &self.current_msg_id,
                    auto_approve,
                    &self.allow_list,
                    self.hooks.as_mut(),
                    self.compaction_level,
                    self.toon_enabled,
                )
                .await
                {
                    Ok(o) => (o.results, o.modifiers),
                    Err(ExecutionControl::Quit) => {
                        self.save_session();
                        return Err(AgentError::UserAborted);
                    }
                }
            } else {
                // Terminal mode: use interactive confirmation
                match execute_tool_calls(
                    &self.tools,
                    &executable_tool_calls,
                    &self.confirmer,
                    self.hooks.as_mut(),
                    self.compaction_level,
                    self.toon_enabled,
                )
                .await
                {
                    Ok(o) => (o.results, o.modifiers),
                    Err(ExecutionControl::Quit) => {
                        self.save_session();
                        return Err(AgentError::UserAborted);
                    }
                }
            };

            let mut executable_results = executable_results.into_iter();
            let mut executable_modifiers = executable_modifiers.into_iter();
            let mut tool_results = Vec::with_capacity(tool_calls.len());
            let mut tool_modifiers = Vec::with_capacity(tool_calls.len());

            for (call, reason) in tool_calls.iter().zip(&malformed_reasons) {
                if let Some(reason) = reason {
                    let ContentBlock::ToolUse { id, name, .. } = call else {
                        continue;
                    };
                    tracing::warn!(
                        target: "aion_agent",
                        tool_call_id = %id,
                        tool = %name,
                        reason = reason.log_reason(),
                        "generated synthetic error result for malformed tool call"
                    );
                    tool_results.push(synthetic_malformed_tool_result(id.clone(), *reason));
                    tool_modifiers.push(None);
                } else {
                    tool_results.push(
                        executable_results
                            .next()
                            .expect("tool execution result missing for executable tool call"),
                    );
                    tool_modifiers.push(
                        executable_modifiers
                            .next()
                            .expect("tool execution modifier missing for executable tool call"),
                    );
                }
            }

            // Apply any context modifiers from skill executions before the next turn.
            self.apply_context_modifiers(&tool_modifiers);

            // Display tool results
            for result in &tool_results {
                if let ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } = result
                {
                    let tool_name = tool_calls
                        .iter()
                        .find_map(|c| {
                            if let ContentBlock::ToolUse { id, name, .. } = c
                                && id == tool_use_id
                            {
                                return Some(name.as_str());
                            }
                            None
                        })
                        .unwrap_or("unknown");
                    let status = if *is_error { "error" } else { "completed" };
                    if tool_use_id.trim().is_empty() {
                        tracing::error!(
                            target: "aion_agent",
                            tool = %tool_name,
                            status,
                            "tool result has empty tool_use_id"
                        );
                    } else {
                        tracing::debug!(
                            target: "aion_agent",
                            tool_use_id = %tool_use_id,
                            tool = %tool_name,
                            status,
                            "tool result emitted"
                        );
                    }
                    self.output
                        .emit_tool_result(tool_use_id, tool_name, *is_error, content);
                }
            }

            let executable_tool_error_turn = malformed_only_fingerprint.is_none()
                && assistant_text.trim().is_empty()
                && !tool_results.is_empty()
                && tool_results.iter().all(|result| {
                    matches!(result, ContentBlock::ToolResult { is_error: true, .. })
                });

            self.messages.push(Message::now(Role::User, tool_results));

            // Save session after each turn
            self.save_session();
            let repeated_malformed_count =
                repeated_malformed_tool_calls.observe(malformed_only_fingerprint);
            if self.max_malformed_tool_call_turns > 0
                && repeated_malformed_count >= self.max_malformed_tool_call_turns
            {
                tracing::warn!(
                    target: "aion_agent",
                    count = repeated_malformed_count,
                    limit = self.max_malformed_tool_call_turns,
                    "stopping repeated malformed tool call loop"
                );
                return Err(AgentError::RepeatedMalformedToolCall {
                    count: repeated_malformed_count,
                    limit: self.max_malformed_tool_call_turns,
                });
            }
            if executable_tool_error_turn {
                consecutive_tool_error_turns += 1;
            } else {
                consecutive_tool_error_turns = 0;
            }
            if consecutive_tool_error_turns >= DEFAULT_MAX_CONSECUTIVE_TOOL_ERROR_TURNS {
                tracing::warn!(
                    target: "aion_agent",
                    count = consecutive_tool_error_turns,
                    limit = DEFAULT_MAX_CONSECUTIVE_TOOL_ERROR_TURNS,
                    "stopping repeated tool failure loop"
                );
                return Err(AgentError::RepeatedToolFailures {
                    count: consecutive_tool_error_turns,
                    limit: DEFAULT_MAX_CONSECUTIVE_TOOL_ERROR_TURNS,
                });
            }
            turn += 1;
        }
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
        let should_compact =
            auto::should_autocompact(self.compact_state.last_input_tokens, &self.compact_config);
        if should_compact {
            tracing::info!(target: "aion_agent", last_input_tokens = self.compact_state.last_input_tokens, "context compaction triggered");
            let threshold = if let Some(pct) = self.compact_config.autocompact_threshold_pct {
                let t = self.compact_config.context_window * pct as usize / 100;
                self.output.emit_info(&format!(
                    "Autocompact threshold: {} tokens ({}% of {})",
                    t, pct, self.compact_config.context_window
                ));
                t
            } else {
                self.compact_config
                    .context_window
                    .saturating_sub(self.compact_config.output_reserve)
                    .saturating_sub(self.compact_config.autocompact_buffer)
            };
            let _ = threshold;
        }
        if should_compact && !self.compact_state.is_circuit_broken(&self.compact_config) {
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
        } else if should_compact {
            self.output.emit_info(&format!(
                "Autocompact: skipped (circuit breaker tripped after {} consecutive failures, \
                 last_input_tokens={})",
                self.compact_state.consecutive_failures, self.compact_state.last_input_tokens
            ));
        } else if !self.compact_config.enabled {
            let threshold = if let Some(pct) = self.compact_config.autocompact_threshold_pct {
                self.compact_config.context_window * pct as usize / 100
            } else {
                self.compact_config
                    .context_window
                    .saturating_sub(self.compact_config.output_reserve)
                    .saturating_sub(self.compact_config.autocompact_buffer)
            };
            if self.compact_state.last_input_tokens as usize >= threshold {
                self.output.emit_info(&format!(
                    "Autocompact: disabled (compact.enabled=false, \
                     last_input_tokens={}, threshold={})",
                    self.compact_state.last_input_tokens, threshold
                ));
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
}

impl AgentEngine {
    /// Initialize a new session for this engine run
    pub fn init_session(
        &mut self,
        provider_name: &str,
        cwd: &str,
        session_id: Option<&str>,
    ) -> anyhow::Result<()> {
        if let Some(mgr) = &self.session_manager {
            let session = mgr.create(provider_name, &self.model, cwd, session_id)?;
            tracing::info!(target: "aion_agent", session_id = %session.id, provider = %provider_name, model = %self.model, "session started");
            self.current_session = Some(session);
        }
        Ok(())
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
        compaction: Option<String>,
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
                        self.thinking = Some(ThinkingConfig::Enabled {
                            budget_tokens: budget,
                        });
                        changes.push(format!("thinking: enabled (budget: {budget})"));
                    }
                    "disabled" => {
                        self.thinking = Some(ThinkingConfig::Disabled);
                        changes.push("thinking: disabled".to_string());
                    }
                    other => {
                        changes.push(format!("thinking: ignored invalid value \"{other}\""));
                    }
                }
            }
        } else if let Some(new_budget) = thinking_budget
            && let Some(ThinkingConfig::Enabled { budget_tokens }) = &mut self.thinking
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

        if let Some(ref level_str) = compaction {
            match level_str.parse::<aion_compact::CompactionLevel>() {
                Ok(new_level) => {
                    let old = self.compaction_level.to_string();
                    self.compaction_level = new_level;
                    changes.push(format!("compaction: {old} → {new_level}"));
                }
                Err(e) => {
                    changes.push(format!("compaction: invalid ({e})"));
                }
            }
        }

        changes
    }

    /// Handle a slash command. Returns `None` if input is not a recognized command.
    pub async fn handle_command(
        &mut self,
        input: &str,
    ) -> Option<Result<crate::commands::CommandResult, anyhow::Error>> {
        let input = input.trim();
        let without_slash = input.strip_prefix('/')?;
        let (name, args) = match without_slash.split_once(char::is_whitespace) {
            Some((n, rest)) => (n, rest.trim()),
            None => (without_slash, ""),
        };

        let cmd = self.commands.find(name)?;

        // We need to borrow self mutably for CommandContext while also
        // borrowing self.commands immutably (already done above via find()).
        // Use a raw pointer to break the borrow conflict — safe because
        // the command is not modified during execution.
        let cmd_ptr = cmd as *const dyn crate::commands::SlashCommand;

        let mut ctx = crate::commands::CommandContext {
            messages: &mut self.messages,
            compact_state: &mut self.compact_state,
            compact_config: &self.compact_config,
            provider: Arc::clone(&self.provider),
            model: &self.model,
            output: self.output.as_ref(),
            registry: &self.commands,
        };

        // SAFETY: cmd_ptr points to a command inside self.commands which is only
        // borrowed immutably and not mutated during execute().
        let result = unsafe { &*cmd_ptr }.execute(&mut ctx, args).await;
        Some(result)
    }

    /// Return metadata for all registered slash commands.
    pub fn slash_command_list(&self) -> Vec<(String, String)> {
        self.commands
            .all()
            .iter()
            .map(|cmd| (cmd.name().to_string(), cmd.description().to_string()))
            .collect()
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

    /// Close a partially recorded turn after the host cancels execution.
    ///
    /// Providers in the Anthropic family require every assistant `tool_use` to
    /// be followed immediately by user `tool_result` blocks. If the host drops
    /// `run()` while tools are executing, the assistant `tool_use` message may
    /// already be in memory without its matching results. Add synthetic error
    /// results so the next request can safely reuse this history.
    pub fn abort_current_turn(&mut self, reason: &str) {
        let Some(last_message) = self.messages.last() else {
            return;
        };
        if last_message.role != Role::Assistant {
            return;
        }

        let pending_results: Vec<_> = last_message
            .content
            .iter()
            .filter_map(|block| {
                let ContentBlock::ToolUse { id, name, .. } = block else {
                    return None;
                };
                Some((id.clone(), name.clone()))
            })
            .collect();

        if pending_results.is_empty() {
            return;
        }

        let result_blocks = pending_results
            .into_iter()
            .map(|(tool_use_id, name)| {
                tracing::info!(
                    target: "aion_agent",
                    tool_use_id = %tool_use_id,
                    tool = %name,
                    "closing pending tool_use after abort"
                );
                self.output
                    .emit_tool_result(&tool_use_id, &name, true, reason);
                ContentBlock::ToolResult {
                    tool_use_id,
                    content: reason.to_string(),
                    is_error: true,
                }
            })
            .collect();

        self.messages.push(Message::now(Role::User, result_blocks));
        self.save_session();
    }

    /// Run stop hooks when the agent session ends
    pub async fn run_stop_hooks(&self) {
        if let Some(hook_engine) = &self.hooks {
            let messages = hook_engine.run_stop().await;
            for msg in messages {
                tracing::info!(target: "aion_agent", hook_message = %msg, "stop hook output");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// set_config tests — apply_config_update()
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests_set_config {
    use std::sync::{Arc, Mutex};

    use aion_providers::error::ProviderError;
    use aion_providers::provider::LlmProvider;
    use aion_tools::registry::ToolRegistry;
    use aion_types::llm::{LlmEvent, LlmRequest};

    use crate::confirm::ToolConfirmer;
    use crate::output::OutputSink;

    struct NullOutput;
    impl OutputSink for NullOutput {
        fn emit_text_delta(&self, _: &str, _: &str) {}
        fn emit_thinking(&self, _: &str, _: &str) {}
        fn emit_tool_call(&self, _: &str, _: &str, _: &str) {}
        fn emit_tool_result(&self, _: &str, _: &str, _: bool, _: &str) {}
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
            max_turns: Some(10),
            max_malformed_tool_call_turns: 3,
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
            compaction_level: aion_compact::CompactionLevel::default(),
            toon_enabled: false,
            commands: crate::commands::default_registry(),
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
        let changes = engine.apply_config_update(Some("new-model".into()), None, None, None, None);
        assert_eq!(engine.model, "new-model");
        assert_eq!(changes.len(), 1);
        assert!(changes[0].contains("old-model"));
        assert!(changes[0].contains("new-model"));
    }

    #[test]
    fn set_config_none_model_no_change() {
        let mut engine = make_engine("current");
        let changes = engine.apply_config_update(None, None, None, None, None);
        assert_eq!(engine.model, "current");
        assert!(changes.is_empty());
    }

    #[test]
    fn set_config_same_model_still_reports_change() {
        let mut engine = make_engine("same");
        let changes = engine.apply_config_update(Some("same".into()), None, None, None, None);
        assert_eq!(changes.len(), 1);
    }

    #[test]
    fn set_config_empty_string_model_accepted() {
        let mut engine = make_engine("real-model");
        engine.apply_config_update(Some(String::new()), None, None, None, None);
        assert_eq!(engine.model, "");
    }

    #[test]
    fn set_config_model_does_not_affect_other_state() {
        let mut engine = make_engine("m");
        engine.current_reasoning_effort = Some("high".into());
        engine.apply_config_update(Some("new-m".into()), None, None, None, None);
        assert_eq!(engine.model, "new-m");
        assert_eq!(engine.current_reasoning_effort.as_deref(), Some("high"));
    }

    // --- Cycle 2: Effort config tests ---

    #[test]
    fn set_config_changes_effort() {
        let mut engine =
            make_engine_with_compat("m", aion_config::compat::ProviderCompat::openai_defaults());
        assert!(engine.current_reasoning_effort.is_none());
        let changes = engine.apply_config_update(None, None, None, Some("high".into()), None);
        assert_eq!(engine.current_reasoning_effort.as_deref(), Some("high"));
        assert_eq!(changes.len(), 1);
        assert!(changes[0].contains("high"));
    }

    #[test]
    fn set_config_clears_effort_with_empty_string() {
        let mut engine = make_engine("m");
        engine.current_reasoning_effort = Some("high".into());
        let changes = engine.apply_config_update(None, None, None, Some(String::new()), None);
        assert!(engine.current_reasoning_effort.is_none());
        assert_eq!(changes.len(), 1);
    }

    // --- Cycle 2: Thinking config tests ---

    #[test]
    fn set_config_enables_thinking() {
        let mut engine = make_engine("m");
        let changes =
            engine.apply_config_update(None, Some("enabled".into()), Some(16000), None, None);
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
        let changes = engine.apply_config_update(None, Some("disabled".into()), None, None, None);
        match &engine.thinking {
            Some(aion_types::llm::ThinkingConfig::Disabled) => {}
            other => panic!("expected Disabled, got: {other:?}"),
        }
        assert_eq!(changes.len(), 1);
    }

    #[test]
    fn set_config_thinking_enabled_default_budget() {
        let mut engine = make_engine("m");
        let changes = engine.apply_config_update(None, Some("enabled".into()), None, None, None);
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
        let changes =
            engine.apply_config_update(None, Some("invalid_value".into()), None, None, None);
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
            None,
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
        let changes = engine.apply_config_update(None, None, Some(20000), None, None);
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
        let changes = engine.apply_config_update(None, None, Some(20000), None, None);
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
            engine.apply_config_update(None, None, None, Some(value.to_string()), None);
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
        let changes = engine.apply_config_update(None, Some("enabled".into()), None, None, None);
        assert!(changes.iter().any(|c| c.contains("not supported")));
        assert!(engine.thinking.is_none());
    }

    #[test]
    fn set_config_effort_rejected_when_unsupported() {
        let mut engine = make_engine("m"); // anthropic defaults: supports_effort = false
        let changes = engine.apply_config_update(None, None, None, Some("high".into()), None);
        assert!(changes.iter().any(|c| c.contains("not supported")));
        assert!(engine.current_reasoning_effort.is_none());
    }

    #[test]
    fn set_config_effort_rejected_invalid_level() {
        let mut engine =
            make_engine_with_compat("m", aion_config::compat::ProviderCompat::openai_defaults());
        let changes = engine.apply_config_update(None, None, None, Some("max".into()), None);
        assert!(changes.iter().any(|c| c.contains("invalid")));
        assert!(engine.current_reasoning_effort.is_none());
    }

    #[test]
    fn set_config_effort_clear_always_works() {
        let mut engine = make_engine("m"); // anthropic defaults: supports_effort = false
        engine.current_reasoning_effort = Some("high".into());
        let changes = engine.apply_config_update(None, None, None, Some(String::new()), None);
        assert!(engine.current_reasoning_effort.is_none());
        assert!(changes.iter().any(|c| c.contains("cleared")));
    }
}

// ---------------------------------------------------------------------------
// Phase 6 tests — apply_context_modifiers()
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests_phase6 {
    use std::sync::{Arc, Mutex};

    use aion_providers::error::ProviderError;
    use aion_providers::provider::LlmProvider;
    use aion_tools::registry::ToolRegistry;
    use aion_types::llm::{LlmEvent, LlmRequest};
    use aion_types::skill_types::{ContextModifier, EffortLevel};

    use crate::confirm::ToolConfirmer;
    use crate::output::OutputSink;

    struct NullOutput;
    impl OutputSink for NullOutput {
        fn emit_text_delta(&self, _: &str, _: &str) {}
        fn emit_thinking(&self, _: &str, _: &str) {}
        fn emit_tool_call(&self, _: &str, _: &str, _: &str) {}
        fn emit_tool_result(&self, _: &str, _: &str, _: bool, _: &str) {}
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
            max_turns: Some(10),
            max_malformed_tool_call_turns: 3,
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
            compaction_level: aion_compact::CompactionLevel::default(),
            toon_enabled: false,
            commands: crate::commands::default_registry(),
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
        let mut engine = make_engine("m", vec!["ExecCommand".to_string()]);
        let modifiers = vec![Some(ContextModifier {
            allowed_tools: vec!["ExecCommand".to_string(), "Read".to_string()],
            ..Default::default()
        })];
        engine.apply_context_modifiers(&modifiers);
        let bash_count = engine
            .allow_list
            .iter()
            .filter(|t| t.as_str() == "ExecCommand")
            .count();
        assert_eq!(bash_count, 1, "ExecCommand should appear exactly once");
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
            allowed_tools: vec!["ExecCommand".to_string()],
            ..Default::default()
        })]);
        assert_eq!(engine.model, "current-model");
        assert!(engine.allow_list.contains(&"ExecCommand".to_string()));
    }

    #[test]
    fn tc_6_27_multiple_modifiers_stacked() {
        let mut engine = make_engine("initial", vec![]);
        let modifiers = vec![
            Some(ContextModifier {
                model: Some("model-a".to_string()),
                allowed_tools: vec!["ExecCommand".to_string()],
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
        assert!(engine.allow_list.contains(&"ExecCommand".to_string()));
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
mod tests_compact {
    use std::sync::{Arc, Mutex};

    use aion_config::compact::CompactConfig;
    use aion_providers::error::ProviderError;
    use aion_providers::provider::LlmProvider;
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
        fn emit_tool_call(&self, _: &str, _: &str, _: &str) {}
        fn emit_tool_result(&self, _: &str, _: &str, _: bool, _: &str) {}
        fn emit_stream_start(&self, _: &str) {}
        fn emit_stream_end(&self, _: &str, _: usize, _: u64, _: u64, _: u64, _: u64) {}
        fn emit_error(&self, _: &str) {}
        fn emit_info(&self, _: &str) {}
    }

    #[derive(Default)]
    struct RecordingOutput {
        tool_results: Mutex<Vec<(String, String, bool, String)>>,
    }

    impl OutputSink for RecordingOutput {
        fn emit_text_delta(&self, _: &str, _: &str) {}
        fn emit_thinking(&self, _: &str, _: &str) {}
        fn emit_tool_call(&self, _: &str, _: &str, _: &str) {}
        fn emit_tool_result(&self, tool_use_id: &str, name: &str, is_error: bool, content: &str) {
            self.tool_results.lock().unwrap().push((
                tool_use_id.to_string(),
                name.to_string(),
                is_error,
                content.to_string(),
            ));
        }
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
        make_compact_engine_with_output(
            compact_config,
            compact_state,
            messages,
            Arc::new(NullOutput),
        )
    }

    fn make_compact_engine_with_output(
        compact_config: CompactConfig,
        compact_state: CompactState,
        messages: Vec<Message>,
        output: Arc<dyn OutputSink>,
    ) -> super::AgentEngine {
        super::AgentEngine {
            provider: Arc::new(NullProvider),
            tools: ToolRegistry::new(),
            messages,
            system_prompt: String::new(),
            model: "test-model".to_string(),
            max_tokens: 4096,
            max_turns: Some(10),
            max_malformed_tool_call_turns: 3,
            total_usage: Default::default(),
            thinking: None,
            compat: aion_config::compat::ProviderCompat::anthropic_defaults(),
            confirmer: Arc::new(Mutex::new(ToolConfirmer::new(true, vec![]))),
            hooks: None,
            session_manager: None,
            current_session: None,
            output,
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
            compaction_level: aion_compact::CompactionLevel::default(),
            toon_enabled: false,
            commands: crate::commands::default_registry(),
        }
    }

    fn tool_use_msg(id: &str, name: &str) -> Message {
        Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: id.to_string(),
                name: name.to_string(),
                input: json!({}),
                extra: None,
            }],
        )
    }

    fn tool_use_msg_with_two_calls(first_id: &str, second_id: &str) -> Message {
        Message::new(
            Role::Assistant,
            vec![
                ContentBlock::ToolUse {
                    id: first_id.to_string(),
                    name: "Read".to_string(),
                    input: json!({}),
                    extra: None,
                },
                ContentBlock::ToolUse {
                    id: second_id.to_string(),
                    name: "ExecCommand".to_string(),
                    input: json!({}),
                    extra: None,
                },
            ],
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

    #[test]
    fn abort_current_turn_closes_pending_tool_uses() {
        let output = Arc::new(RecordingOutput::default());
        let mut engine = make_compact_engine_with_output(
            CompactConfig::default(),
            CompactState::new(),
            vec![
                Message::new(
                    Role::User,
                    vec![ContentBlock::Text {
                        text: "run tools".to_string(),
                    }],
                ),
                tool_use_msg_with_two_calls("call_read", "call_bash"),
            ],
            output.clone(),
        );

        engine.abort_current_turn("Tool execution canceled by user");

        let last = engine.messages.last().expect("synthetic result message");
        assert_eq!(last.role, Role::User);
        assert_eq!(last.content.len(), 2);
        assert!(
            matches!(&last.content[0], ContentBlock::ToolResult { tool_use_id, content, is_error }
                if tool_use_id == "call_read" && content == "Tool execution canceled by user" && *is_error)
        );
        assert!(
            matches!(&last.content[1], ContentBlock::ToolResult { tool_use_id, content, is_error }
                if tool_use_id == "call_bash" && content == "Tool execution canceled by user" && *is_error)
        );

        let emitted = output.tool_results.lock().unwrap();
        assert_eq!(emitted.len(), 2);
        assert_eq!(
            emitted[0],
            (
                "call_read".into(),
                "Read".into(),
                true,
                "Tool execution canceled by user".into()
            )
        );
        assert_eq!(
            emitted[1],
            (
                "call_bash".into(),
                "ExecCommand".into(),
                true,
                "Tool execution canceled by user".into()
            )
        );
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
mod tests_plan_mode {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use aion_providers::error::ProviderError;
    use aion_providers::provider::LlmProvider;
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
        fn emit_tool_call(&self, _: &str, _: &str, _: &str) {}
        fn emit_tool_result(&self, _: &str, _: &str, _: bool, _: &str) {}
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
            max_turns: Some(10),
            max_malformed_tool_call_turns: 3,
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
            compaction_level: aion_compact::CompactionLevel::default(),
            toon_enabled: false,
            commands: crate::commands::default_registry(),
        }
    }

    // --- TC-3.5-03: Enter transition activates plan mode ---

    #[test]
    fn enter_transition_activates_plan_mode() {
        let mut engine = make_plan_engine(vec!["Read".into(), "ExecCommand".into()]);
        let modifiers = vec![Some(ContextModifier {
            plan_mode_transition: Some(PlanModeTransition::Enter),
            ..Default::default()
        })];

        engine.apply_context_modifiers(&modifiers);

        assert!(engine.plan_state.is_active, "plan mode should be active");
        assert_eq!(
            engine.plan_state.pre_plan_allow_list,
            vec!["Read".to_string(), "ExecCommand".to_string()],
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
        let mut engine = make_plan_engine(vec!["Read".into(), "ExecCommand".into()]);

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
            vec!["Read".to_string(), "ExecCommand".to_string()],
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

#[cfg(test)]
mod tests_handle_command {
    use std::sync::{Arc, Mutex};

    use aion_providers::error::ProviderError;
    use aion_providers::provider::LlmProvider;
    use aion_tools::registry::ToolRegistry;
    use aion_types::llm::{LlmEvent, LlmRequest};
    use aion_types::message::{ContentBlock, Message, Role};

    use crate::compact::state::CompactState;
    use crate::confirm::ToolConfirmer;
    use crate::output::OutputSink;

    struct NullOutput;
    impl OutputSink for NullOutput {
        fn emit_text_delta(&self, _: &str, _: &str) {}
        fn emit_thinking(&self, _: &str, _: &str) {}
        fn emit_tool_call(&self, _: &str, _: &str, _: &str) {}
        fn emit_tool_result(&self, _: &str, _: &str, _: bool, _: &str) {}
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

    fn make_engine() -> super::AgentEngine {
        super::AgentEngine {
            provider: Arc::new(NullProvider),
            tools: ToolRegistry::new(),
            messages: vec![],
            system_prompt: String::new(),
            model: "test-model".to_string(),
            max_tokens: 4096,
            max_turns: Some(10),
            max_malformed_tool_call_turns: 3,
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
            compact_state: CompactState::new(),
            plan_state: Default::default(),
            plan_active_flag: None,
            cache_detector: super::CacheBreakDetector::new(),
            compaction_level: aion_compact::CompactionLevel::default(),
            toon_enabled: false,
            commands: crate::commands::default_registry(),
        }
    }

    #[tokio::test]
    async fn handle_command_quit() {
        let mut engine = make_engine();
        let result = engine.handle_command("/quit").await;
        assert!(matches!(
            result,
            Some(Ok(crate::commands::CommandResult::Exit))
        ));
    }

    #[tokio::test]
    async fn handle_command_exit_alias() {
        let mut engine = make_engine();
        let result = engine.handle_command("/exit").await;
        assert!(matches!(
            result,
            Some(Ok(crate::commands::CommandResult::Exit))
        ));
    }

    #[tokio::test]
    async fn handle_command_unknown() {
        let mut engine = make_engine();
        let result = engine.handle_command("/nonexistent").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn handle_command_clear() {
        let mut engine = make_engine();
        engine.messages.push(Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "hello".to_string(),
            }],
        ));
        assert_eq!(engine.messages.len(), 1);

        let result = engine.handle_command("/clear").await;
        assert!(matches!(
            result,
            Some(Ok(crate::commands::CommandResult::Continue))
        ));
        assert!(engine.messages.is_empty());
        assert_eq!(engine.compact_state.last_input_tokens, 0);
    }

    #[tokio::test]
    async fn handle_command_with_args() {
        let mut engine = make_engine();
        let result = engine.handle_command("/help compact").await;
        assert!(matches!(
            result,
            Some(Ok(crate::commands::CommandResult::Continue))
        ));
    }

    #[tokio::test]
    async fn handle_command_not_a_command() {
        let mut engine = make_engine();
        let result = engine.handle_command("hello world").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn run_intercepts_help_returns_zero_turns() {
        let mut engine = make_engine();
        let result = engine.run("/help", "msg-1").await.unwrap();
        assert_eq!(result.turns, 0);
        assert_eq!(result.usage.input_tokens, 0);
    }

    #[tokio::test]
    async fn run_intercepts_quit_returns_user_aborted() {
        let mut engine = make_engine();
        let err = engine.run("/quit", "msg-1").await.unwrap_err();
        assert!(matches!(err, super::AgentError::UserAborted));
    }

    #[test]
    fn slash_command_list_returns_all() {
        let engine = make_engine();
        let list = engine.slash_command_list();
        assert!(list.len() >= 4);
        let names: Vec<&str> = list.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"help"));
        assert!(names.contains(&"compact"));
        assert!(names.contains(&"clear"));
        assert!(names.contains(&"quit"));
    }
}
