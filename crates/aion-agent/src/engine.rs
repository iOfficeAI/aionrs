use std::mem::replace;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::cache_diagnostics::{CacheBreakDetector, CacheDiagnostic, CacheStats};
use crate::commands::{CommandContext, CommandRegistry, CommandResult, SlashCommand, default_registry};
use crate::compact::auto::{CompactError, autocompact, should_autocompact};
use crate::compact::emergency::is_at_emergency_limit;
use crate::compact::estimate::estimate_tokens_from_messages;
use crate::compact::micro::{microcompact, should_microcompact};
use crate::compact::state::CompactState;
use crate::confirm::ToolConfirmer;
use crate::error::AgentError;
use crate::orchestration::{ExecutionControl, execute_tool_calls, execute_tool_calls_with_approval};
use crate::output::OutputSink;
use crate::plan::prompt::plan_mode_instructions;
use crate::plan::state::PlanState;
use crate::session::{Session, SessionManager};
use crate::stream::StreamOutcome;
use crate::tool_call::{
    DEFAULT_MAX_TOOL_CALL_FAILURE, DEFAULT_MAX_TOOL_CALL_MALFORMED, ToolCallMalformedFingerprint, merge_tool_results,
    tool_call_malformed_fingerprint, tool_call_malformed_reason,
};
use crate::turn::{FinalizationReason, TurnGuardAction, TurnGuards, TurnKind, TurnOutcome};
use aion_compact::CompactLevel;
use aion_config::compact::CompactConfig;
use aion_config::compat::ProviderCompat;
use aion_config::config::Config;
use aion_config::hooks::HookEngine;
use aion_protocol::ToolApprovalManager;
use aion_protocol::events::ToolCategory;
use aion_protocol::writer::ProtocolEmitter;
use aion_providers::provider::{LlmProvider, create_provider};
use aion_tools::registry::ToolRegistry;
use aion_types::llm::{LlmEvent, LlmRequest, ThinkingConfig};
use aion_types::message::{ContentBlock, Message, Role, StopReason, TokenUsage};
use aion_types::skill_types::{ContextModifier, PlanModeTransition, effort_to_string};
use anyhow::{Error as AnyhowError, Result as AnyhowResult};
use chrono::Utc;
use serde_json::to_string;
use tokio::sync::mpsc::Receiver;
use tracing::{Instrument, debug, error, info, info_span, warn};

#[derive(Debug)]
pub struct AgentResult {
    pub text: String,
    pub stop_reason: StopReason,
    pub usage: TokenUsage,
    pub turns: usize,
}

pub struct AgentEngine {
    // Provider request configuration.
    /// Shared LLM provider used to issue model requests.
    provider: Arc<dyn LlmProvider>,
    /// Resolved provider compatibility and capability settings.
    compat: ProviderCompat,
    /// Optional provider-neutral thinking configuration for model requests.
    thinking: Option<ThinkingConfig>,
    /// Base system prompt sent with each model request.
    system_prompt: String,
    /// Active model identifier used for provider requests.
    model: String,
    /// Persisted reasoning effort, updated by skill context modifiers.
    /// Carried into each model turn's LlmRequest.reasoning_effort.
    reasoning_effort: Option<String>,

    // Conversation and run state.
    /// Conversation history used to build the next provider request.
    messages: Vec<Message>,
    /// Cumulative token usage across the active session/run.
    total_usage: TokenUsage,
    /// Output message ID for the currently streaming run.
    msg_id: String,
    /// Maximum output tokens requested from the provider per turn.
    max_tokens: u32,
    /// Optional cap on counted model turns within a single run.
    max_turns_per_run: Option<usize>,
    /// Consecutive malformed tool-call round limit before aborting.
    max_tool_call_malformed_turns: usize,
    /// Consecutive failed tool-call round limit before aborting.
    max_tool_call_failure_turns: usize,

    // Tool execution policy.
    /// Registry of tools available to the engine.
    tools: ToolRegistry,
    /// Shared tool confirmer used for approval policy decisions.
    confirmer: Arc<Mutex<ToolConfirmer>>,
    /// Tool names currently allowed without additional approval.
    allow_list: Vec<String>,
    /// Optional hook engine for lifecycle and tool hooks.
    hooks: Option<HookEngine>,

    // Session persistence.
    /// Optional session manager used when persistence is enabled.
    session_manager: Option<SessionManager>,
    /// Active session record updated as the conversation progresses.
    current_session: Option<Session>,

    // Output and host protocol integration.
    /// Sink for user-visible and host-visible output events.
    output: Arc<dyn OutputSink>,
    /// Optional host approval manager for JSON stream tool approvals.
    approval_manager: Option<Arc<ToolApprovalManager>>,
    /// Optional protocol emitter used to send structured host events.
    protocol_writer: Option<Arc<dyn ProtocolEmitter>>,

    // Compaction and plan-mode state.
    /// Static compaction thresholds, flags, and sizing configuration.
    compact_config: CompactConfig,
    /// Runtime compaction watermark and circuit-breaker state.
    compact_state: CompactState,
    /// Active compaction strategy level.
    compact_level: CompactLevel,
    /// Whether TOON-formatted compaction output is enabled.
    toon_enabled: bool,
    /// Runtime plan mode state and restoration data.
    plan_state: PlanState,
    /// Shared flag read by EnterPlanMode/ExitPlanMode tools to validate transitions.
    /// Updated by the engine when processing PlanModeTransition modifiers.
    plan_active_flag: Option<Arc<AtomicBool>>,

    // Diagnostics and command handling.
    /// Prompt cache break detector for diagnostics.
    cache_detector: CacheBreakDetector,
    /// Slash command registry used before normal model execution.
    commands: CommandRegistry,
}

impl AgentEngine {
    pub fn new(config: Config, tools: ToolRegistry, output: Arc<dyn OutputSink>, cwd: PathBuf) -> Self {
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
        let confirmer = ToolConfirmer::new(config.tools.auto_approve, config.tools.allow_list.clone());

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
            model: config.model,
            max_tokens: config.max_tokens,
            thinking: config.thinking,
            compat: config.compat.clone(),
            system_prompt,
            reasoning_effort: None,
            messages: Vec::new(),
            total_usage: TokenUsage::default(),
            msg_id: String::new(),
            max_turns_per_run: config.max_turns,
            max_tool_call_malformed_turns: config
                .max_tool_call_malformed_turns
                .unwrap_or(DEFAULT_MAX_TOOL_CALL_MALFORMED),
            max_tool_call_failure_turns: config
                .max_tool_call_failure_turns
                .unwrap_or(DEFAULT_MAX_TOOL_CALL_FAILURE),
            tools,
            confirmer: Arc::new(Mutex::new(confirmer)),
            allow_list,
            hooks: Some(HookEngine::new(config.hooks.clone(), cwd.clone())),
            session_manager,
            current_session: None,
            output,
            approval_manager: None,
            protocol_writer: None,
            compact_config,
            compact_state: CompactState::new(),
            compact_level: config.compact.compaction,
            toon_enabled: config.compact.toon,
            plan_state: PlanState::default(),
            plan_active_flag: None,
            cache_detector: CacheBreakDetector::new(),
            commands: default_registry(),
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
        let confirmer = ToolConfirmer::new(config.tools.auto_approve, config.tools.allow_list.clone());

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
            model: config.model.clone(),
            max_tokens: config.max_tokens,
            thinking: config.thinking,
            compat: config.compat.clone(),
            system_prompt,
            reasoning_effort: None,
            messages: session.messages.clone(),
            total_usage: session.total_usage.clone(),
            msg_id: String::new(),
            max_turns_per_run: config.max_turns,
            max_tool_call_malformed_turns: config
                .max_tool_call_malformed_turns
                .unwrap_or(DEFAULT_MAX_TOOL_CALL_MALFORMED),
            max_tool_call_failure_turns: config
                .max_tool_call_failure_turns
                .unwrap_or(DEFAULT_MAX_TOOL_CALL_FAILURE),
            tools,
            confirmer: Arc::new(Mutex::new(confirmer)),
            allow_list,
            hooks: Some(HookEngine::new(config.hooks.clone(), cwd)),
            session_manager,
            current_session: Some(session),
            output,
            approval_manager: None,
            protocol_writer: None,
            compact_config,
            compact_state: CompactState::new(),
            compact_level: config.compact.compaction,
            toon_enabled: config.compact.toon,
            plan_state: PlanState::default(),
            plan_active_flag: None,
            cache_detector: CacheBreakDetector::new(),
            commands: default_registry(),
        }
    }

    pub fn compaction_level(&self) -> CompactLevel {
        self.compact_level
    }

    /// Get a reference to the shared provider
    pub fn provider(&self) -> &Arc<dyn LlmProvider> {
        &self.provider
    }

    /// Get a reference to the resolved compat settings
    pub fn compat(&self) -> &ProviderCompat {
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

    pub fn set_approval_manager(&mut self, mgr: Arc<ToolApprovalManager>) {
        self.approval_manager = Some(mgr);
    }

    pub fn set_protocol_writer(&mut self, writer: Arc<dyn ProtocolEmitter>) {
        self.protocol_writer = Some(writer);
    }

    /// Set the initial reasoning effort override (used by sub-agents spawned with an effort override).
    pub fn set_initial_reasoning_effort(&mut self, effort: Option<String>) {
        self.reasoning_effort = effort;
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
        let session_id = self.current_session.as_ref().map(|s| s.id.clone()).unwrap_or_default();
        let span = info_span!(
            target: "aion_agent",
            "agent_run",
            session_id = %session_id,
            msg_id = %msg_id,
        );
        self.run_inner(user_input, msg_id).instrument(span).await
    }

    async fn run_inner(&mut self, user_input: &str, msg_id: &str) -> Result<AgentResult, AgentError> {
        // Slash command interception — before any LLM call
        if let Some(result) = self.handle_command(user_input).await? {
            return Ok(result);
        }

        self.msg_id = msg_id.to_string();
        self.output.emit_stream_start(msg_id);
        self.messages.push(Message::now(
            Role::User,
            vec![ContentBlock::Text {
                text: user_input.to_string(),
            }],
        ));

        let mut guards = TurnGuards::new(
            self.max_turns_per_run,
            self.max_tool_call_malformed_turns,
            self.max_tool_call_failure_turns,
        );
        loop {
            if let Some(limit) = guards.turn_budget_reached() {
                self.save_session();
                let message = format!(
                    "Stopped after reaching the turn budget (max_turns={limit}); the task did not converge. Try adjusting the request or retrying."
                );
                warn!(target: "aion_agent", limit, "stopping agent run at turn budget");
                self.output.emit_error(&message);
                return Ok(AgentResult {
                    text: String::new(),
                    stop_reason: StopReason::MaxTurns,
                    usage: self.total_usage.clone(),
                    turns: guards.counted_turns(),
                });
            }

            let outcome = self.run_turn(TurnKind::Normal).await?;
            guards.record_counted_turn();

            let (assistant_text, tool_calls) = match TurnOutcome::from_stream(outcome) {
                TurnOutcome::ToolRound(outcome) => {
                    let assistant_content = build_assistant_content(&outcome);
                    self.messages.push(Message::now(Role::Assistant, assistant_content));
                    (outcome.assistant_text, outcome.tool_calls)
                }
                TurnOutcome::Final(outcome) => {
                    let assistant_content = build_assistant_content(&outcome);
                    self.messages.push(Message::now(Role::Assistant, assistant_content));
                    self.save_session();
                    return Ok(AgentResult {
                        text: outcome.assistant_text,
                        stop_reason: outcome.stop_reason,
                        usage: self.total_usage.clone(),
                        turns: guards.counted_turns(),
                    });
                }
                TurnOutcome::Truncated(outcome) => {
                    let assistant_content = build_assistant_content(&outcome);
                    self.messages.push(Message::now(Role::Assistant, assistant_content));
                    return self
                        .finalize_once(
                            FinalizationReason::MaxTokens,
                            outcome.assistant_text,
                            guards.counted_turns(),
                            StopReason::MaxTokens,
                        )
                        .await;
                }
                TurnOutcome::EmptyFinal(outcome) => {
                    return self
                        .finalize_once(
                            FinalizationReason::EmptyFinal,
                            outcome.assistant_text,
                            guards.counted_turns(),
                            StopReason::EndTurn,
                        )
                        .await;
                }
            };

            // need to execute tool calls before the next turn
            let ToolRoundOutput {
                tool_results,
                tool_modifiers,
                tool_call_malformed_fingerprint,
                tool_call_failure_round,
            } = self.execute_tool_round(&tool_calls, &assistant_text).await?;

            // Apply any context modifiers from skill executions before the next turn.
            self.apply_context_modifiers(&tool_modifiers);

            self.emit_tool_results(&tool_calls, &tool_results);

            self.messages.push(Message::now(Role::User, tool_results));

            // Save session after each tool round.
            self.save_session();

            match guards.after_tool_round(tool_call_malformed_fingerprint, tool_call_failure_round) {
                TurnGuardAction::Continue => {}
                TurnGuardAction::Finalize => {
                    return self
                        .finalize_once(
                            FinalizationReason::TurnBudget,
                            String::new(),
                            guards.counted_turns(),
                            StopReason::MaxTurns,
                        )
                        .await;
                }
                TurnGuardAction::Stop(err) => return Err(err),
            }
        }
    }

    /// Build the next provider request, applying plan-mode tool/system filtering
    /// and recording the prompt state for cache diagnostics.
    fn build_request(&mut self, kind: TurnKind) -> LlmRequest {
        // Build tool list: filter based on plan mode state
        let tools = if kind.disable_tools() {
            Vec::new()
        } else if self.plan_state.is_active {
            // Plan mode: only Info-category tools (excluding EnterPlanMode)
            self.tools
                .to_tool_defs_filtered(|t| t.category() == ToolCategory::Info && t.name() != "EnterPlanMode")
        } else {
            // Normal mode: all tools except ExitPlanMode
            self.tools.to_tool_defs_filtered(|t| t.name() != "ExitPlanMode")
        };

        // Build system prompt: append plan mode instructions when active
        let system = if self.plan_state.is_active {
            format!("{}\n\n{}", self.system_prompt, plan_mode_instructions())
        } else {
            self.system_prompt.clone()
        };

        // Record prompt state for cache diagnostics
        self.cache_detector.record_request(&system, &tools);

        let mut messages = self.messages.clone();
        if let Some(prompt) = kind.control_prompt() {
            messages.push(Message::now(
                Role::User,
                vec![ContentBlock::Text {
                    text: prompt.to_string(),
                }],
            ));
        }

        LlmRequest {
            model: self.model.clone(),
            system,
            messages,
            tools,
            max_tokens: self.max_tokens,
            thinking: self.thinking.clone(),
            reasoning_effort: self.reasoning_effort.clone(),
        }
    }

    /// Classify, execute and re-merge one model turn's tool calls.
    ///
    /// Malformed calls get synthetic error results; the rest are executed via
    /// the approval (JSON stream) or interactive (terminal) path. Results and
    /// skill modifiers are interleaved back into the original call order.
    /// `assistant_text` is the visible text from the same turn, used only to
    /// classify an all-error round for the consecutive-failure breaker.
    ///
    /// A `Quit` from tool execution is surfaced as `AgentError::UserAborted`
    /// after saving the session.
    async fn execute_tool_round(
        &mut self,
        tool_calls: &[ContentBlock],
        assistant_text: &str,
    ) -> Result<ToolRoundOutput, AgentError> {
        let tool_call_malformed_reasons: Vec<_> = tool_calls
            .iter()
            .map(|call| {
                let ContentBlock::ToolUse { id, name, .. } = call else {
                    return None;
                };
                tool_call_malformed_reason(id, name)
            })
            .collect();
        let tool_call_malformed_fingerprint = tool_call_malformed_fingerprint(tool_calls, &tool_call_malformed_reasons);
        let executable_tool_calls: Vec<_> = tool_calls
            .iter()
            .zip(&tool_call_malformed_reasons)
            .filter(|(_, reason)| reason.is_none())
            .map(|(call, _)| call.clone())
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
                &self.msg_id,
                auto_approve,
                &self.allow_list,
                self.hooks.as_mut(),
                self.compact_level,
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
                self.compact_level,
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

        let (tool_results, tool_modifiers) = merge_tool_results(
            tool_calls,
            &tool_call_malformed_reasons,
            executable_results,
            executable_modifiers,
        );

        let tool_call_failure_round = tool_call_malformed_fingerprint.is_none()
            && assistant_text.trim().is_empty()
            && !tool_results.is_empty()
            && tool_results
                .iter()
                .all(|result| matches!(result, ContentBlock::ToolResult { is_error: true, .. }));

        Ok(ToolRoundOutput {
            tool_results,
            tool_modifiers,
            tool_call_malformed_fingerprint,
            tool_call_failure_round,
        })
    }

    /// Emit each tool result to the output sink, resolving the tool name from
    /// the originating `tool_calls` for display and logging.
    fn emit_tool_results(&self, tool_calls: &[ContentBlock], tool_results: &[ContentBlock]) {
        for result in tool_results {
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
                    error!(
                        target: "aion_agent",
                        tool = %tool_name,
                        status,
                        "tool result has empty tool_use_id"
                    );
                } else {
                    debug!(
                        target: "aion_agent",
                        tool_use_id = %tool_use_id,
                        tool = %tool_name,
                        status,
                        "tool result emitted"
                    );
                }
                self.output.emit_tool_result(tool_use_id, tool_name, *is_error, content);
            }
        }
    }

    async fn run_turn(&mut self, kind: TurnKind) -> Result<StreamOutcome, AgentError> {
        // Run multi-level compaction before each API call.
        // On the first model turn last_input_tokens is 0 so neither
        // autocompact nor emergency will fire.
        self.run_compaction().await?;
        let request = self.build_request(kind);
        let mut rx = self.provider.stream(&request).await?;
        let outcome = self.consume_stream(&mut rx).await?;
        self.record_turn_usage(&outcome.usage);
        Ok(outcome)
    }

    async fn finalize_once(
        &mut self,
        reason: FinalizationReason,
        prefix_text: String,
        counted_turns: usize,
        fallback_stop_reason: StopReason,
    ) -> Result<AgentResult, AgentError> {
        let outcome = self.run_turn(TurnKind::Finalization(reason)).await?;
        let combined_text = format!("{}{}", prefix_text, outcome.assistant_text);
        let is_success = outcome.tool_calls.is_empty()
            && outcome.stop_reason == StopReason::EndTurn
            && !outcome.assistant_text.trim().is_empty();

        if is_success {
            let assistant_content = build_assistant_content(&outcome);
            self.messages.push(Message::now(Role::Assistant, assistant_content));
            self.save_session();
            return Ok(AgentResult {
                text: combined_text,
                stop_reason: StopReason::EndTurn,
                usage: self.total_usage.clone(),
                turns: counted_turns,
            });
        }

        let fallback = reason.fallback_prompt();
        self.output.emit_error(fallback);
        let fallback_text = if combined_text.trim().is_empty() {
            fallback.to_string()
        } else {
            combined_text
        };

        self.messages.push(Message::now(
            Role::Assistant,
            vec![ContentBlock::Text {
                text: fallback_text.clone(),
            }],
        ));
        self.save_session();
        Ok(AgentResult {
            text: fallback_text,
            stop_reason: fallback_stop_reason,
            usage: self.total_usage.clone(),
            turns: counted_turns,
        })
    }

    /// Drain one provider stream into a [`StreamOutcome`].
    ///
    /// Emits text/thinking/tool-call events to the output sink as they arrive
    /// and accumulates the assistant text, thinking block, tool calls, stop
    /// reason and usage for the caller. Returns early on `LlmEvent::Error`.
    async fn consume_stream(&self, rx: &mut Receiver<LlmEvent>) -> Result<StreamOutcome, AgentError> {
        let mut assistant_text = String::new();
        let mut thinking_text = String::new();
        let mut thinking_signature: Option<String> = None;
        let mut tool_calls: Vec<ContentBlock> = Vec::new();
        let mut stop_reason = StopReason::EndTurn;
        let mut usage = TokenUsage::default();

        while let Some(event) = rx.recv().await {
            match event {
                LlmEvent::TextDelta(text) => {
                    self.output.emit_text_delta(&text, &self.msg_id);
                    assistant_text.push_str(&text);
                }
                LlmEvent::ToolUse { id, name, input, extra } => {
                    if id.trim().is_empty() {
                        error!(
                            target: "aion_agent",
                            tool = %name,
                            "provider emitted tool call with empty tool_use_id"
                        );
                    } else {
                        debug!(
                            target: "aion_agent",
                            tool_use_id = %id,
                            tool = %name,
                            "provider tool call received"
                        );
                    }
                    let input_str = to_string(&input).unwrap_or_default();
                    self.output.emit_tool_call(&id, &name, &input_str);
                    tool_calls.push(ContentBlock::ToolUse { id, name, input, extra });
                }
                LlmEvent::ThinkingDelta(text) => {
                    self.output.emit_thinking(&text, &self.msg_id);
                    thinking_text.push_str(&text);
                }
                LlmEvent::ThinkingSignature(signature) => {
                    thinking_signature = Some(signature);
                }
                LlmEvent::Done {
                    stop_reason: sr,
                    usage: u,
                } => {
                    stop_reason = sr;
                    usage = u;
                }
                LlmEvent::Error(e) => {
                    return Err(AgentError::ApiError(e));
                }
            }
        }

        Ok(StreamOutcome {
            assistant_text,
            thinking_text,
            thinking_signature,
            tool_calls,
            stop_reason,
            usage,
        })
    }

    /// Fold one turn's token usage into the running totals and update the
    /// compaction watermark and cache-break diagnostics.
    fn record_turn_usage(&mut self, turn_usage: &TokenUsage) {
        self.total_usage.input_tokens += turn_usage.input_tokens;
        self.total_usage.output_tokens += turn_usage.output_tokens;
        self.total_usage.cache_creation_tokens += turn_usage.cache_creation_tokens;
        self.total_usage.cache_read_tokens += turn_usage.cache_read_tokens;

        // Track per-turn input tokens for compaction watermark.
        // Use max(provider_reported, local_estimate) as a safety net:
        // some providers (e.g. DeepSeek with prefix caching) underreport
        // prompt_tokens, causing compaction to never trigger.
        let local_estimate = estimate_tokens_from_messages(&self.messages);
        let effective_watermark = turn_usage.input_tokens.max(local_estimate);

        if local_estimate > turn_usage.input_tokens && local_estimate.saturating_sub(turn_usage.input_tokens) > 10_000 {
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
                    self.output.emit_error(&format!("Cache full miss: {cause:?}"));
                }
                CacheDiagnostic::PartialMiss { hit_rate, cause } => {
                    if self.compact_config.cache_diagnostics {
                        self.output
                            .emit_info(&format!("Cache: {:.0}% hit rate (cause: {cause:?})", hit_rate * 100.0));
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
    }

    /// Run the multi-level compaction pipeline before each API call.
    ///
    /// Execution order: microcompact → autocompact → emergency check.
    /// After a successful autocompact the emergency check is skipped
    /// because the context has been significantly reduced.
    async fn run_compaction(&mut self) -> Result<(), AgentError> {
        // 1. Microcompact (lightweight, no LLM call)
        if should_microcompact(&self.messages, &self.compact_config) {
            let result = microcompact(&mut self.messages, &self.compact_config);
            if result.cleared_count > 0 {
                self.output.emit_info(&format!(
                    "Microcompact: cleared {} tool results (~{} tokens freed)",
                    result.cleared_count, result.estimated_tokens_freed
                ));
            }
        }

        // 2. Autocompact (LLM summarization)
        let mut compacted = false;
        let should_compact = should_autocompact(self.compact_state.last_input_tokens, &self.compact_config);
        if should_compact {
            info!(target: "aion_agent", last_input_tokens = self.compact_state.last_input_tokens, "context compaction triggered");
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
            match autocompact(
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
                Err(CompactError::CircuitBroken { .. }) => {
                    // Already tripped; logged at circuit-breaker level
                }
                Err(e) => {
                    self.output.emit_error(&format!("Autocompact failed: {}", e));
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
        if !compacted && is_at_emergency_limit(self.compact_state.last_input_tokens, &self.compact_config) {
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
    pub fn init_session(&mut self, provider_name: &str, cwd: &str, session_id: Option<&str>) -> AnyhowResult<()> {
        if let Some(mgr) = &self.session_manager {
            let session = mgr.create(provider_name, &self.model, cwd, session_id)?;
            info!(target: "aion_agent", session_id = %session.id, provider = %provider_name, model = %self.model, "session started");
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
            let old = replace(&mut self.model, new_model.clone());
            changes.push(format!("model: {old} → {new_model}"));
        }

        if let Some(thinking_str) = thinking {
            if !self.compat.supports_thinking() {
                changes.push("thinking: not supported by current provider".to_string());
            } else {
                match thinking_str.as_str() {
                    "enabled" => {
                        let budget = thinking_budget.unwrap_or(Self::DEFAULT_THINKING_BUDGET);
                        self.thinking = Some(ThinkingConfig::Enabled { budget_tokens: budget });
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
                self.reasoning_effort = None;
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
                        .reasoning_effort
                        .replace(new_effort.clone())
                        .unwrap_or_else(|| "none".to_string());
                    changes.push(format!("effort: {old} → {new_effort}"));
                }
            }
        }

        if let Some(ref level_str) = compaction {
            match level_str.parse::<CompactLevel>() {
                Ok(new_level) => {
                    let old = self.compact_level.to_string();
                    self.compact_level = new_level;
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
    async fn handle_command(&mut self, input: &str) -> Result<Option<AgentResult>, AgentError> {
        let Some(command) = parse_command_input(input) else {
            return Ok(None);
        };
        let Some(result) = self.execute_command(command).await else {
            return Ok(None);
        };

        match result {
            Ok(CommandResult::Continue) => {
                info!(command = command.display_name, "Slash command executed");
                Ok(Some(AgentResult {
                    text: String::new(),
                    stop_reason: StopReason::EndTurn,
                    usage: TokenUsage::default(),
                    turns: 0,
                }))
            }
            Ok(CommandResult::Exit) => {
                info!(command = command.display_name, "Slash command executed: exit");
                Err(AgentError::UserAborted)
            }
            Err(e) => {
                error!(command = command.display_name, error = %e, "Slash command failed");
                Err(AgentError::ApiError(e.to_string()))
            }
        }
    }

    async fn execute_command(&mut self, command: ParsedSlashCommand<'_>) -> Option<Result<CommandResult, AnyhowError>> {
        let cmd = self.commands.find(command.name)?;

        // We need to borrow self mutably for CommandContext while also
        // borrowing self.commands immutably (already done above via find()).
        // Use a raw pointer to break the borrow conflict — safe because
        // the command is not modified during execution.
        let cmd_ptr = cmd as *const dyn SlashCommand;

        let mut ctx = CommandContext {
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
        let result = unsafe { &*cmd_ptr }.execute(&mut ctx, command.args).await;
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
                self.reasoning_effort = Some(effort_to_string(effort));
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
            session.updated_at = Utc::now();
            if let Err(e) = mgr.save(session) {
                self.output.emit_error(&format!("Failed to save session: {}", e));
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
                info!(
                    target: "aion_agent",
                    tool_use_id = %tool_use_id,
                    tool = %name,
                    "closing pending tool_use after abort"
                );
                self.output.emit_tool_result(&tool_use_id, &name, true, reason);
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
                info!(target: "aion_agent", hook_message = %msg, "stop hook output");
            }
        }
    }
}

/// Result of running one model turn's tool calls: the per-call results and
/// skill modifiers (aligned 1:1 with the originating `tool_calls`), plus the
/// loop-guard signals derived from this round.
struct ToolRoundOutput {
    tool_results: Vec<ContentBlock>,
    tool_modifiers: Vec<Option<ContextModifier>>,
    /// `Some` only when every tool call in the round was malformed; feeds the
    /// tool-call-malformed breaker.
    tool_call_malformed_fingerprint: Option<ToolCallMalformedFingerprint>,
    /// True when this round produced executable (non-malformed) tool calls
    /// that all errored and the model emitted no visible text; feeds the
    /// consecutive-tool-call-failure breaker.
    tool_call_failure_round: bool,
}

/// Assemble the assistant message content blocks (thinking, text, tool calls)
/// from a completed [`StreamOutcome`], preserving the canonical block order.
fn build_assistant_content(outcome: &StreamOutcome) -> Vec<ContentBlock> {
    let mut content: Vec<ContentBlock> = Vec::new();
    if !outcome.thinking_text.is_empty() || outcome.thinking_signature.is_some() {
        content.push(ContentBlock::Thinking {
            thinking: outcome.thinking_text.clone(),
            signature: outcome.thinking_signature.clone(),
        });
    }
    if !outcome.assistant_text.is_empty() {
        content.push(ContentBlock::Text {
            text: outcome.assistant_text.clone(),
        });
    }
    content.extend(outcome.tool_calls.iter().cloned());
    content
}

#[derive(Debug, Clone, Copy)]
struct ParsedSlashCommand<'a> {
    display_name: &'a str,
    name: &'a str,
    args: &'a str,
}

fn parse_command_input(input: &str) -> Option<ParsedSlashCommand<'_>> {
    let input = input.trim();
    let display_name = input.split_whitespace().next().unwrap_or(input);
    let without_slash = input.strip_prefix('/')?;
    let (name, args) = match without_slash.split_once(|c: char| c.is_whitespace()) {
        Some((name, rest)) => (name, rest.trim()),
        None => (without_slash, ""),
    };

    Some(ParsedSlashCommand {
        display_name,
        name,
        args,
    })
}

#[cfg(test)]
#[path = "engine.test.rs"]
mod engine_test;
