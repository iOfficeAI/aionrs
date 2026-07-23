use std::mem::replace;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::cache_diagnostics::{CacheBreakDetector, CacheDiagnostic, CacheStats};
use crate::commands::{CommandContext, CommandRegistry, CommandResult, SlashCommand, default_registry};
use crate::compact::auto::{CompactError, autocompact, should_autocompact};
use crate::compact::emergency::is_at_emergency_limit;
use crate::compact::estimate::{estimate_tokens_from_tool_image, estimate_tokens_from_tool_result};
use crate::compact::micro::{microcompact, should_microcompact};
use crate::compact::state::CompactState;
use crate::confirm::ToolConfirmer;
use crate::context_usage::{
    ContextState, ContextStatus, PromptUsage, estimate_content_tokens, estimate_messages_tokens, estimate_text_tokens,
    estimate_tool_definitions_tokens,
};
use crate::error::AgentError;
use crate::orchestration::{ExecutionControl, execute_tool_calls, execute_tool_calls_with_approval};
use crate::output::OutputSink;
use crate::plan::prompt::plan_mode_instructions;
use crate::plan::state::PlanState;
use crate::session::{Session, SessionManager};
use crate::stream::StreamOutcome;
use crate::tool_call::{
    DEFAULT_MAX_TOOL_CALL_FAILURE, DEFAULT_MAX_TOOL_CALL_MALFORMED, ToolCallFailureFingerprint,
    ToolCallMalformedFingerprint, merge_tool_results, tool_call_failure_fingerprint, tool_call_malformed_fingerprint,
    tool_call_malformed_reason,
};
use crate::tool_policy::ToolPolicy;
use crate::turn::{FinalizationReason, ToolLoopWarning, TurnGuardAction, TurnGuards, TurnKind, TurnOutcome};
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
use aion_types::message::{ContentBlock, ImageInputCapability, Message, Role, StopReason, TokenUsage};
use aion_types::skill_types::{ContextModifier, PlanModeTransition, effort_to_string};
use aion_types::tool::ToolDef;
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
    max_tokens: Option<u32>,
    /// Optional cap on counted model turns within a single run.
    max_turns_per_run: Option<usize>,
    /// Consecutive malformed tool-call round limit before aborting.
    max_tool_call_malformed_turns: usize,
    /// Consecutive failed tool-call round limit before aborting.
    max_tool_call_failure_turns: usize,

    // Tool execution policy.
    /// Registry of tools available to the engine.
    tools: ToolRegistry,
    /// Runtime authorization applied to tool advertisement and execution.
    tool_policy: ToolPolicy,
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
    /// Runtime context-size and compaction circuit-breaker state.
    compact_state: CompactState,
    /// Persisted usage, source, and successful compaction counters.
    context_state: ContextState,
    /// Estimated category metadata for the system prompt.
    prompt_usage: PromptUsage,
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
        Self::new_with_provider_and_env(provider, config, tools, output, cwd, Vec::new())
    }

    pub fn new_with_provider_and_env(
        provider: Arc<dyn LlmProvider>,
        config: Config,
        tools: ToolRegistry,
        output: Arc<dyn OutputSink>,
        cwd: PathBuf,
        runtime_env: Vec<(String, String)>,
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

        let prompt_usage = PromptUsage::from_system_prompt(&system_prompt);
        let mut engine = Self {
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
            tool_policy: ToolPolicy::default(),
            confirmer: Arc::new(Mutex::new(confirmer)),
            allow_list,
            hooks: Some(HookEngine::new_with_env(config.hooks.clone(), cwd.clone(), runtime_env)),
            session_manager,
            current_session: None,
            output,
            approval_manager: None,
            protocol_writer: None,
            compact_config,
            compact_state: CompactState::new(),
            context_state: ContextState::default(),
            prompt_usage,
            compact_level: config.compact.compaction,
            toon_enabled: config.compact.toon,
            plan_state: PlanState::default(),
            plan_active_flag: None,
            cache_detector: CacheBreakDetector::new(),
            commands: default_registry(),
        };
        engine.refresh_local_context_estimate();
        engine
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
        Self::resume_with_provider_and_env(provider, config, tools, output, session, cwd, Vec::new())
    }

    pub fn resume_with_provider_and_env(
        provider: Arc<dyn LlmProvider>,
        config: Config,
        tools: ToolRegistry,
        output: Arc<dyn OutputSink>,
        session: Session,
        cwd: PathBuf,
        runtime_env: Vec<(String, String)>,
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

        let prompt_usage = PromptUsage::from_system_prompt(&system_prompt);
        let context_state = session.context_state.clone();
        let mut compact_state = CompactState::new();
        compact_state.last_input_tokens = context_state.context_usage;
        let mut engine = Self {
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
            tool_policy: ToolPolicy::default(),
            confirmer: Arc::new(Mutex::new(confirmer)),
            allow_list,
            hooks: Some(HookEngine::new_with_env(config.hooks.clone(), cwd, runtime_env)),
            session_manager,
            current_session: Some(session),
            output,
            approval_manager: None,
            protocol_writer: None,
            compact_config,
            compact_state,
            context_state,
            prompt_usage,
            compact_level: config.compact.compaction,
            toon_enabled: config.compact.toon,
            plan_state: PlanState::default(),
            plan_active_flag: None,
            cache_detector: CacheBreakDetector::new(),
            commands: default_registry(),
        };
        if engine.context_state.context_usage == 0 {
            engine.refresh_local_context_estimate();
        }
        engine
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

    /// Replace the runtime authorization policy for registered tools.
    pub(crate) fn set_tool_policy(&mut self, tool_policy: ToolPolicy) {
        let changed = self.tool_policy != tool_policy;
        self.tool_policy = tool_policy;
        if changed {
            self.refresh_local_context_estimate();
        }
    }

    /// Replace bootstrap-derived prompt category metadata.
    pub(crate) fn set_prompt_usage(&mut self, prompt_usage: PromptUsage) {
        self.prompt_usage = prompt_usage;
    }

    /// Get the current session ID (if sessions are enabled and initialized)
    pub fn current_session_id(&self) -> Option<String> {
        self.current_session.as_ref().map(|s| s.id.clone())
    }

    /// Return the current context accounting status for SDK hosts.
    pub fn context_status(&self) -> ContextStatus {
        ContextStatus {
            schema_version: self.context_state.schema_version,
            model: self.model.clone(),
            context_window: self.compact_config.context_window as u64,
            context_usage: self.context_state.context_usage,
            source: self.context_state.source,
            compact_count: self.context_state.compact_count,
            microcompact_count: self.context_state.microcompact_count,
            updated_at: self.context_state.updated_at,
        }
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
    /// Run the agent loop with plain user input.
    ///
    /// This entry point is text-only: the input is wrapped in a single
    /// `ContentBlock::Text` and no markers are parsed. Slash commands are
    /// intercepted before any LLM call.
    pub async fn run(&mut self, user_input: &str, msg_id: &str) -> Result<AgentResult, AgentError> {
        // Slash command interception — before any LLM call.
        if let Some(result) = self.handle_command(user_input).await? {
            return Ok(result);
        }
        let blocks = vec![ContentBlock::Text {
            text: user_input.to_string(),
        }];
        self.run_with_blocks(blocks, msg_id).await
    }

    /// Run the agent loop with structured content blocks.
    ///
    /// This is the host-integration entry point for multimodal input such as
    /// text and images. A single text block containing a recognized slash
    /// command is handled locally; multimodal inputs are never interpreted as
    /// commands.
    pub async fn run_with_blocks(
        &mut self,
        content_blocks: Vec<ContentBlock>,
        msg_id: &str,
    ) -> Result<AgentResult, AgentError> {
        let command_input = match content_blocks.as_slice() {
            [ContentBlock::Text { text }] => Some(text.clone()),
            _ => None,
        };
        if let Some(input) = command_input
            && let Some(result) = self.handle_command(&input).await?
        {
            return Ok(result);
        }

        let session_id = self.current_session.as_ref().map(|s| s.id.clone()).unwrap_or_default();
        let span = info_span!(
            target: "aion_agent",
            "agent_run",
            session_id = %session_id,
            msg_id = %msg_id,
        );
        self.run_inner(content_blocks, msg_id).instrument(span).await
    }

    async fn run_inner(&mut self, content_blocks: Vec<ContentBlock>, msg_id: &str) -> Result<AgentResult, AgentError> {
        self.msg_id = msg_id.to_string();
        self.output.emit_stream_start(msg_id);

        let user_tokens = estimate_content_tokens(&content_blocks);
        self.messages.push(Message::now(Role::User, content_blocks));
        self.record_local_context_addition(user_tokens);
        self.save_session();

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

            let tool_calls = match TurnOutcome::from_stream(outcome) {
                TurnOutcome::ToolRound(outcome) => {
                    let assistant_content = build_assistant_content(&outcome);
                    self.messages.push(Message::now(Role::Assistant, assistant_content));
                    self.save_session();
                    outcome.tool_calls
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
                    self.save_session();
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
                    warn!(
                        target: "aion_agent",
                        stop_reason = ?outcome.stop_reason,
                        assistant_text_bytes = outcome.assistant_text.len(),
                        thinking_text_bytes = outcome.thinking_text.len(),
                        tool_call_count = outcome.tool_calls.len(),
                        "provider turn produced no valid final answer; retrying finalization"
                    );
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
                mut tool_results,
                tool_modifiers,
                follow_up_blocks,
                tool_call_malformed_fingerprint,
                tool_call_failure_fingerprint,
                all_tool_results_error,
            } = self.execute_tool_round(&tool_calls).await?;

            // Apply any context modifiers from skill executions before the next turn.
            self.apply_context_modifiers(&tool_modifiers);

            let guard_action = guards.after_tool_round(
                tool_call_malformed_fingerprint,
                tool_call_failure_fingerprint,
                all_tool_results_error,
            );
            if let TurnGuardAction::Warn(warning) = guard_action {
                append_tool_loop_warning(&mut tool_results, warning);
            }

            self.emit_tool_results(&tool_calls, &tool_results);
            self.record_tool_context_estimate(&tool_results, &follow_up_blocks);

            self.messages.push(Message::now(Role::User, tool_results));
            if !follow_up_blocks.is_empty() {
                self.messages.push(Message::now(Role::User, follow_up_blocks));
            }

            // Save session after each tool round.
            self.save_session();

            match guard_action {
                TurnGuardAction::Continue | TurnGuardAction::Warn(_) => {}
                TurnGuardAction::Finalize(reason) => {
                    return self
                        .finalize_once(reason, String::new(), guards.counted_turns(), StopReason::MaxTurns)
                        .await;
                }
                TurnGuardAction::Stop(err) => return Err(err),
            }
        }
    }

    /// Build the next provider request, applying plan-mode tool/system filtering
    /// and recording the prompt state for cache diagnostics.
    fn build_request(&mut self, kind: TurnKind) -> LlmRequest {
        let image_input = self.compat.image_input();
        let tools = self.tool_definitions_for_turn(kind);

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
        project_image_input(&mut messages, image_input, &self.model);

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

    fn tool_definitions_for_turn(&self, kind: TurnKind) -> Vec<ToolDef> {
        let image_input = self.compat.image_input();
        if kind.disable_tools() {
            Vec::new()
        } else if self.plan_state.is_active {
            self.tools.to_tool_defs_filtered(|tool| {
                self.tool_policy.allows(tool.name())
                    && (!tool.requires_image_input() || image_input.supports_images())
                    && tool.category() == ToolCategory::Info
                    && tool.name() != "EnterPlanMode"
            })
        } else {
            self.tools.to_tool_defs_filtered(|tool| {
                self.tool_policy.allows(tool.name())
                    && (!tool.requires_image_input() || image_input.supports_images())
                    && tool.name() != "ExitPlanMode"
            })
        }
    }

    /// Classify, execute and re-merge one model turn's tool calls.
    ///
    /// Malformed calls get synthetic error results; the rest are executed via
    /// the approval (JSON stream) or interactive (terminal) path. Results and
    /// skill modifiers are interleaved back into the original call order.
    ///
    /// A `Quit` from tool execution is surfaced as `AgentError::UserAborted`
    /// after saving the session.
    async fn execute_tool_round(&mut self, tool_calls: &[ContentBlock]) -> Result<ToolRoundOutput, AgentError> {
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
        let policy_denied_tool_names: Vec<_> = tool_calls
            .iter()
            .zip(&tool_call_malformed_reasons)
            .map(|(call, malformed_reason)| {
                if malformed_reason.is_some() {
                    return None;
                }
                let ContentBlock::ToolUse { name, .. } = call else {
                    return None;
                };
                let capability_denied = self
                    .tools
                    .get(name)
                    .is_some_and(|tool| tool.requires_image_input() && !self.compat.image_input().supports_images());
                (!self.tool_policy.allows(name) || capability_denied).then(|| name.clone())
            })
            .collect();
        let executable_tool_calls: Vec<_> = tool_calls
            .iter()
            .zip(&tool_call_malformed_reasons)
            .zip(&policy_denied_tool_names)
            .filter(|((_, malformed_reason), denied_name)| malformed_reason.is_none() && denied_name.is_none())
            .map(|((call, _), _)| call.clone())
            .collect();

        let (executable_results, executable_modifiers, follow_up_blocks) = if executable_tool_calls.is_empty() {
            (Vec::new(), Vec::new(), Vec::new())
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
                Ok(o) => (o.results, o.modifiers, o.follow_up_blocks),
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
                Ok(o) => (o.results, o.modifiers, o.follow_up_blocks),
                Err(ExecutionControl::Quit) => {
                    self.save_session();
                    return Err(AgentError::UserAborted);
                }
            }
        };

        let (tool_results, tool_modifiers) = merge_tool_results(
            tool_calls,
            &tool_call_malformed_reasons,
            &policy_denied_tool_names,
            executable_results,
            executable_modifiers,
        );

        let failed_tool_calls: Vec<_> = tool_calls
            .iter()
            .zip(&tool_call_malformed_reasons)
            .zip(&tool_results)
            .filter(|((_, malformed_reason), result)| {
                malformed_reason.is_none() && matches!(result, ContentBlock::ToolResult { is_error: true, .. })
            })
            .map(|((call, _), _)| call.clone())
            .collect();
        let tool_call_failure_fingerprint = tool_call_failure_fingerprint(&failed_tool_calls);
        let all_tool_results_error = tool_call_failure_fingerprint.is_some()
            && tool_results
                .iter()
                .all(|result| matches!(result, ContentBlock::ToolResult { is_error: true, .. }));

        Ok(ToolRoundOutput {
            tool_results,
            tool_modifiers,
            follow_up_blocks,
            tool_call_malformed_fingerprint,
            tool_call_failure_fingerprint,
            all_tool_results_error,
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
        let span = info_span!(
            target: "aion_agent",
            "llm_generation",
            generation_phase = kind.diagnostic_phase(),
            tools_disabled = kind.disable_tools(),
        );
        async {
            // Run multi-level compaction before each API call.
            // On the first model turn context_tokens is 0 so neither
            // autocompact nor emergency will fire.
            self.run_compaction().await?;
            let request = self.build_request(kind);
            let mut rx = self.provider.stream(&request).await?;
            let outcome = self.consume_stream(&mut rx).await?;
            let has_provider_usage = self.record_turn_usage(&outcome.usage);
            if !has_provider_usage {
                let assistant_content = build_assistant_content(&outcome);
                self.record_local_context_addition(estimate_content_tokens(&assistant_content));
            }
            Ok(outcome)
        }
        .instrument(span)
        .await
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

        warn!(
            target: "aion_agent",
            finalization_reason = ?reason,
            stop_reason = ?outcome.stop_reason,
            assistant_text_bytes = outcome.assistant_text.len(),
            thinking_text_bytes = outcome.thinking_text.len(),
            tool_call_count = outcome.tool_calls.len(),
            "provider finalization did not produce a valid final answer"
        );
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
        let mut provider_items: Vec<ContentBlock> = Vec::new();
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
                LlmEvent::ProviderItem { provider, item } => {
                    provider_items.push(ContentBlock::ProviderItem { provider, item });
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
            provider_items,
            tool_calls,
            stop_reason,
            usage,
        })
    }

    /// Fold one turn's token usage into the running totals and replace the
    /// best-known context size with the provider's exact turn total.
    fn record_turn_usage(&mut self, turn_usage: &TokenUsage) -> bool {
        self.total_usage.input_tokens += turn_usage.input_tokens;
        self.total_usage.output_tokens += turn_usage.output_tokens;
        self.total_usage.cache_creation_tokens += turn_usage.cache_creation_tokens;
        self.total_usage.cache_read_tokens += turn_usage.cache_read_tokens;

        let context_usage = turn_usage.input_tokens.saturating_add(turn_usage.output_tokens);
        if context_usage == 0 {
            debug!(target: "aion_agent", "provider omitted turn usage; retaining local context projection");
            return false;
        }

        self.context_state.replace_with_provider_usage(context_usage);
        self.sync_compact_watermark();

        // Cache break detection
        let cache_stats = CacheStats {
            input_tokens: turn_usage.input_tokens,
            cache_read_tokens: turn_usage.cache_read_tokens,
            cache_creation_tokens: turn_usage.cache_creation_tokens,
        };
        if let Some(diagnostic) = self.cache_detector.check_response(cache_stats) {
            match &diagnostic {
                CacheDiagnostic::FullMiss { cause } => {
                    self.output.emit_info(&format!("Cache full miss: {cause:?}"));
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
        true
    }

    /// Add content produced after the provider usage snapshot. Every final
    /// tool result is estimated once; tool-emitted images are counted because
    /// they are also sent in the next provider request.
    fn record_tool_context_estimate(&mut self, tool_results: &[ContentBlock], tool_images: &[ContentBlock]) {
        let tool_result_tokens = tool_results.iter().fold(0_u64, |total, result| {
            total.saturating_add(estimate_tokens_from_tool_result(result))
        });
        let tool_image_tokens = tool_images.iter().fold(0_u64, |total, image| {
            total.saturating_add(estimate_tokens_from_tool_image(image))
        });
        let added_tokens = tool_result_tokens.saturating_add(tool_image_tokens);

        self.record_local_context_addition(added_tokens);
        debug!(
            target: "aion_agent",
            tool_result_tokens,
            tool_image_tokens,
            context_tokens = self.compact_state.last_input_tokens,
            "tool results added to context token estimate"
        );
    }

    fn record_local_context_addition(&mut self, tokens: u64) {
        self.context_state.add_local_estimate(tokens);
        self.sync_compact_watermark();
    }

    fn sync_compact_watermark(&mut self) {
        self.compact_state.last_input_tokens = self.context_state.context_usage;
    }

    fn refresh_local_context_estimate(&mut self) {
        let tools = self.tool_definitions_for_turn(TurnKind::Normal);
        let dynamic_system_tokens = self.dynamic_system_tokens();
        let context_usage = self
            .prompt_usage
            .total_tokens()
            .saturating_add(dynamic_system_tokens)
            .saturating_add(estimate_tool_definitions_tokens(&tools))
            .saturating_add(estimate_messages_tokens(&self.messages));
        self.context_state.replace_with_local_estimate(context_usage);
        self.sync_compact_watermark();
    }

    fn dynamic_system_tokens(&self) -> u64 {
        if self.plan_state.is_active {
            estimate_text_tokens(plan_mode_instructions())
        } else {
            0
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
                self.context_state
                    .record_microcompact(result.estimated_tokens_freed as u64);
                self.sync_compact_watermark();
                self.save_session();
            }
        }

        // 2. Autocompact (LLM summarization)
        let mut compacted = false;
        let should_compact = should_autocompact(self.compact_state.last_input_tokens, &self.compact_config);
        if should_compact {
            info!(target: "aion_agent", context_tokens = self.compact_state.last_input_tokens, "context compaction triggered");
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
            let mut compact_messages = self.messages.clone();
            project_image_input(&mut compact_messages, self.compat.image_input(), &self.model);
            match autocompact(
                provider.as_ref(),
                &compact_messages,
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
                    self.context_state.record_compact();
                    self.refresh_local_context_estimate();
                    self.save_session();
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
                 context_tokens={})",
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
                     context_tokens={}, threshold={})",
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

fn project_image_input(messages: &mut [Message], capability: ImageInputCapability, model: &str) {
    for message in messages {
        let mut removed_image = false;
        message.content.retain(|block| {
            let ContentBlock::Image { image_url } = block else {
                return true;
            };

            if !capability.supports_images() {
                removed_image = true;
                return false;
            }
            if let Err(error) = image_url.validate() {
                warn!(
                    target: "aion_agent",
                    model,
                    error = %error,
                    "omitting invalid historical image from provider request"
                );
                removed_image = true;
                return false;
            }
            true
        });

        if removed_image {
            let text = match capability {
                ImageInputCapability::Supported => "[Image unavailable: the stored image payload is invalid.]",
                ImageInputCapability::Unsupported => "[Image omitted: the selected model does not support vision.]",
                ImageInputCapability::Unknown => {
                    "[Image omitted: image-input support is unknown for the selected model.]"
                }
            };
            message.content.push(ContentBlock::Text { text: text.to_owned() });
        }
    }
}

impl AgentEngine {
    /// Initialize a new session for this engine run
    pub fn init_session(&mut self, provider_name: &str, cwd: &str, session_id: Option<&str>) -> AnyhowResult<()> {
        if let Some(mgr) = &self.session_manager {
            let mut session = mgr.create(provider_name, &self.model, cwd, session_id)?;
            session.context_state = self.context_state.clone();
            mgr.save(&session)?;
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
        image_input: Option<ImageInputCapability>,
        thinking: Option<String>,
        thinking_budget: Option<u32>,
        effort: Option<String>,
        compaction: Option<String>,
    ) -> Vec<String> {
        let mut changes = Vec::new();
        let model_changed = model.is_some();

        if let Some(new_model) = model {
            let old = replace(&mut self.model, new_model.clone());
            changes.push(format!("model: {old} → {new_model}"));
        }

        if let Some(new_capability) = image_input {
            let old = self.compat.image_input();
            self.compat.image_input = Some(new_capability);
            changes.push(format!("image input: {old:?} → {new_capability:?}"));
        } else if model_changed {
            let old = self.compat.image_input();
            self.compat.image_input = None;
            if old != ImageInputCapability::Unknown {
                changes.push(format!("image input: {old:?} → Unknown"));
            }
        }

        if let Some(thinking_str) = thinking {
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
            Ok(CommandResult::ContextChanged) => {
                self.refresh_local_context_estimate();
                self.save_session();
                info!(
                    command = command.display_name,
                    "Slash command executed and context persisted"
                );
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
        let context_tools = self.tool_definitions_for_turn(TurnKind::Normal);
        let dynamic_system_tokens = self.dynamic_system_tokens();

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
            context_state: &mut self.context_state,
            prompt_usage: &self.prompt_usage,
            context_tools: &context_tools,
            dynamic_system_tokens,
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
        let mut request_shape_changed = false;
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
                        request_shape_changed = true;
                    }
                    PlanModeTransition::Exit { .. } => {
                        self.plan_state.is_active = false;
                        self.allow_list = self.plan_state.pre_plan_allow_list.clone();
                        if let Some(ref flag) = self.plan_active_flag {
                            flag.store(false, Ordering::Release);
                        }
                        request_shape_changed = true;
                    }
                }
            }
        }
        if request_shape_changed {
            self.refresh_local_context_estimate();
        }
    }

    fn save_session(&mut self) {
        if let (Some(mgr), Some(session)) = (&self.session_manager, &mut self.current_session) {
            session.messages = self.messages.clone();
            session.total_usage = self.total_usage.clone();
            session.context_state = self.context_state.clone();
            session.updated_at = Utc::now();
            if let Err(e) = mgr.save(session) {
                self.output.emit_error(&format!("Failed to save session: {}", e));
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

        let result_blocks: Vec<ContentBlock> = pending_results
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

        self.record_tool_context_estimate(&result_blocks, &[]);
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
    follow_up_blocks: Vec<ContentBlock>,
    /// `Some` only when every tool call in the round was malformed; feeds the
    /// tool-call-malformed breaker.
    tool_call_malformed_fingerprint: Option<ToolCallMalformedFingerprint>,
    /// `Some` when at least one non-malformed tool call failed. The fingerprint
    /// contains only failed calls, so successful sibling calls do not reset the
    /// exact-call or cycle breakers.
    tool_call_failure_fingerprint: Option<ToolCallFailureFingerprint>,
    /// Whether the round had a non-malformed call and every result was an error.
    all_tool_results_error: bool,
}

fn append_tool_loop_warning(tool_results: &mut [ContentBlock], warning: ToolLoopWarning) {
    let Some(content) = tool_results.iter_mut().rev().find_map(|result| {
        let ContentBlock::ToolResult {
            content,
            is_error: true,
            ..
        } = result
        else {
            return None;
        };
        Some(content)
    }) else {
        return;
    };

    content.push_str("\n\n");
    content.push_str(&warning.guidance());
}

/// Assemble the assistant message content blocks (thinking, text, tool calls)
/// from a completed [`StreamOutcome`], preserving the canonical block order.
fn build_assistant_content(outcome: &StreamOutcome) -> Vec<ContentBlock> {
    let mut content = outcome.provider_items.clone();
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
#[path = "engine_test.rs"]
mod engine_test;
