use std::sync::{Arc, Mutex};

use crate::config::Config;
use crate::confirm::ToolConfirmer;
use crate::hooks::HookEngine;
use crate::output::OutputSink;
use crate::provider::{LlmProvider, ProviderError, create_provider};
use crate::session::{Session, SessionManager};
use crate::skills::context_modifier::{ContextModifier, effort_to_string};
use crate::tools::orchestration::{ExecutionControl, execute_tool_calls, execute_tool_calls_with_approval};
use crate::tools::registry::ToolRegistry;
use crate::types::llm::{LlmEvent, LlmRequest};
use crate::types::message::{ContentBlock, Message, Role, StopReason, TokenUsage};

pub struct AgentEngine {
    provider: Arc<dyn LlmProvider>,
    tools: ToolRegistry,
    messages: Vec<Message>,
    system_prompt: String,
    model: String,
    max_tokens: u32,
    max_turns: usize,
    total_usage: TokenUsage,
    thinking: Option<crate::types::llm::ThinkingConfig>,
    confirmer: Arc<Mutex<ToolConfirmer>>,
    hooks: Option<HookEngine>,
    session_manager: Option<SessionManager>,
    current_session: Option<Session>,
    output: Arc<dyn OutputSink>,
    current_msg_id: String,
    approval_manager: Option<Arc<crate::protocol::ToolApprovalManager>>,
    protocol_writer: Option<Arc<crate::protocol::writer::ProtocolWriter>>,
    allow_list: Vec<String>,
    /// Persisted reasoning effort, updated by skill context modifiers.
    /// Carried into each turn's LlmRequest.reasoning_effort.
    current_reasoning_effort: Option<String>,
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
        let confirmer = ToolConfirmer::new(
            config.tools.auto_approve,
            config.tools.allow_list.clone(),
        );

        let session_manager = if config.session.enabled {
            Some(SessionManager::new(
                config.session.directory.clone().into(),
                config.session.max_sessions,
            ))
        } else {
            None
        };

        let allow_list = config.tools.allow_list.clone();

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
        }
    }

    /// Create from a resumed session
    pub fn resume(config: Config, tools: ToolRegistry, output: Arc<dyn OutputSink>, session: Session) -> Self {
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
        let confirmer = ToolConfirmer::new(
            config.tools.auto_approve,
            config.tools.allow_list.clone(),
        );

        let session_manager = if config.session.enabled {
            Some(SessionManager::new(
                config.session.directory.clone().into(),
                config.session.max_sessions,
            ))
        } else {
            None
        };

        let allow_list = config.tools.allow_list.clone();

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
            confirmer: Arc::new(Mutex::new(confirmer)),
            // Always initialise Some so that skill-declared hooks can be merged in
            // even when the global config has no static hooks configured.
            hooks: Some(HookEngine::new(config.hooks.clone())),
            session_manager,
            current_session: Some(session),
            output,
            current_msg_id: String::new(),
            approval_manager: None,
            protocol_writer: None,
            allow_list,
            // TODO(phase-7+): persist skill-overridden model/effort in Session
            // so they survive session resume. Currently resets to defaults.
            current_reasoning_effort: None,
        }
    }

    /// Get a reference to the shared provider
    pub fn provider(&self) -> &Arc<dyn LlmProvider> {
        &self.provider
    }

    /// Initialize a new session for this engine run
    pub fn init_session(&mut self, provider_name: &str, cwd: &str, session_id: Option<&str>) -> anyhow::Result<()> {
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

    pub fn set_approval_manager(&mut self, mgr: Arc<crate::protocol::ToolApprovalManager>) {
        self.approval_manager = Some(mgr);
    }

    pub fn set_protocol_writer(&mut self, writer: Arc<crate::protocol::writer::ProtocolWriter>) {
        self.protocol_writer = Some(writer);
    }

    /// Set the initial reasoning effort override (used by sub-agents spawned with an effort override).
    pub fn set_initial_reasoning_effort(&mut self, effort: Option<String>) {
        self.current_reasoning_effort = effort;
    }

    /// Run the agent loop with user input
    pub async fn run(&mut self, user_input: &str, msg_id: &str) -> Result<AgentResult, AgentError> {
        self.current_msg_id = msg_id.to_string();
        self.output.emit_stream_start(msg_id);
        self.messages.push(Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: user_input.to_string(),
            }],
        });

        for turn in 0..self.max_turns {
            let request = LlmRequest {
                model: self.model.clone(),
                system: self.system_prompt.clone(),
                messages: self.messages.clone(),
                tools: self.tools.to_tool_defs(),
                max_tokens: self.max_tokens,
                thinking: self.thinking.clone(),
                reasoning_effort: self.current_reasoning_effort.clone(),
            };

            let mut rx = self.provider.stream(&request).await?;
            let mut assistant_text = String::new();
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

            let mut assistant_content: Vec<ContentBlock> = Vec::new();
            if !assistant_text.is_empty() {
                assistant_content.push(ContentBlock::Text {
                    text: assistant_text.clone(),
                });
            }
            assistant_content.extend(tool_calls.clone());

            self.messages.push(Message {
                role: Role::Assistant,
                content: assistant_content,
            });

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
                let writer = self.protocol_writer.as_ref().expect("protocol writer required for approval");
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
                ).await {
                    Ok(o) => o,
                    Err(ExecutionControl::Quit) => {
                        self.save_session();
                        return Err(AgentError::UserAborted);
                    }
                }
            } else {
                // Terminal mode: use interactive confirmation
                match execute_tool_calls(&self.tools, &tool_calls, &self.confirmer, self.hooks.as_mut()).await {
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
                            if let ContentBlock::ToolUse { id, name, .. } = c {
                                if let ContentBlock::ToolResult { tool_use_id, .. } = result {
                                    if id == tool_use_id {
                                        return Some(name.as_str());
                                    }
                                }
                            }
                            None
                        })
                        .unwrap_or("unknown");
                    self.output.emit_tool_result(tool_name, *is_error, content);
                }
            }

            self.messages.push(Message {
                role: Role::User,
                content: outcome.results,
            });

            // Save session after each turn
            self.save_session();
        }

        self.save_session();
        Err(AgentError::MaxTurnsExceeded(self.max_turns))
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
    /// Called after each batch of tool results, before building the next turn's LlmRequest.
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
        }
    }

    fn save_session(&mut self) {
        if let (Some(mgr), Some(session)) = (&self.session_manager, &mut self.current_session) {
            session.messages = self.messages.clone();
            session.total_usage = self.total_usage.clone();
            session.updated_at = chrono::Utc::now();
            if let Err(e) = mgr.save(session) {
                self.output.emit_error(&format!("Failed to save session: {}", e));
            }
            if let Err(e) = mgr.update_index_for(session) {
                self.output.emit_error(&format!("Failed to update session index: {}", e));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Phase 6 tests — apply_context_modifiers()
// ---------------------------------------------------------------------------

#[cfg(test)]
mod phase6_tests {
    use std::sync::{Arc, Mutex};

    use crate::confirm::ToolConfirmer;
    use crate::output::OutputSink;
    use crate::provider::{LlmProvider, ProviderError};
    use crate::skills::context_modifier::ContextModifier;
    use crate::skills::types::EffortLevel;
    use crate::tools::registry::ToolRegistry;
    use crate::types::llm::{LlmEvent, LlmRequest};

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
        }
    }

    // TC-6.21: model override → self.model updated
    #[test]
    fn tc_6_21_model_override_applied() {
        let mut engine = make_engine("original-model", vec![]);
        let modifiers = vec![Some(ContextModifier {
            model: Some("override-model".to_string()),
            effort: None,
            allowed_tools: vec![],
        })];
        engine.apply_context_modifiers(&modifiers);
        assert_eq!(engine.model, "override-model");
    }

    // TC-6.22: effort override → current_reasoning_effort updated
    #[test]
    fn tc_6_22_effort_override_applied() {
        let mut engine = make_engine("m", vec![]);
        let modifiers = vec![Some(ContextModifier {
            model: None,
            effort: Some(EffortLevel::High),
            allowed_tools: vec![],
        })];
        engine.apply_context_modifiers(&modifiers);
        assert_eq!(engine.current_reasoning_effort.as_deref(), Some("high"));
    }

    // TC-6.22b: all EffortLevel variants map correctly
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
                model: None,
                effort: Some(level),
                allowed_tools: vec![],
            })]);
            assert_eq!(
                engine.current_reasoning_effort.as_deref(),
                Some(expected),
                "EffortLevel::{level:?} should map to {expected:?}"
            );
        }
    }

    // TC-6.23: allowed_tools added to allow_list; duplicates not added
    #[test]
    fn tc_6_23_allowed_tools_no_duplicates() {
        let mut engine = make_engine("m", vec!["Bash".to_string()]);
        let modifiers = vec![Some(ContextModifier {
            model: None,
            effort: None,
            allowed_tools: vec!["Bash".to_string(), "Read".to_string()],
        })];
        engine.apply_context_modifiers(&modifiers);

        let bash_count = engine.allow_list.iter().filter(|t| t.as_str() == "Bash").count();
        assert_eq!(bash_count, 1, "Bash should appear exactly once");
        assert!(engine.allow_list.contains(&"Read".to_string()));
    }

    // TC-6.24: None modifiers in list are skipped without panic
    #[test]
    fn tc_6_24_none_modifiers_skipped() {
        let mut engine = make_engine("original", vec![]);
        engine.apply_context_modifiers(&[None, None]);
        assert_eq!(engine.model, "original");
        assert!(engine.current_reasoning_effort.is_none());
    }

    // TC-6.25: empty modifier list leaves engine state unchanged
    #[test]
    fn tc_6_25_empty_modifiers_no_change() {
        let mut engine = make_engine("current-model", vec![]);
        engine.apply_context_modifiers(&[]);
        assert_eq!(engine.model, "current-model");
        assert!(engine.allow_list.is_empty());
    }

    // TC-6.26: modifier.model = None does not overwrite existing model
    #[test]
    fn tc_6_26_none_model_does_not_overwrite() {
        let mut engine = make_engine("current-model", vec![]);
        engine.apply_context_modifiers(&[Some(ContextModifier {
            model: None,
            effort: None,
            allowed_tools: vec!["Bash".to_string()],
        })]);
        assert_eq!(engine.model, "current-model");
        assert!(engine.allow_list.contains(&"Bash".to_string()));
    }

    // TC-6.27: multiple modifiers — model from last, allowed_tools merged
    #[test]
    fn tc_6_27_multiple_modifiers_stacked() {
        let mut engine = make_engine("initial", vec![]);
        let modifiers = vec![
            Some(ContextModifier {
                model: Some("model-a".to_string()),
                effort: None,
                allowed_tools: vec!["Bash".to_string()],
            }),
            Some(ContextModifier {
                model: Some("model-b".to_string()),
                effort: None,
                allowed_tools: vec!["Read".to_string()],
            }),
        ];
        engine.apply_context_modifiers(&modifiers);
        assert_eq!(engine.model, "model-b", "last model wins");
        assert!(engine.allow_list.contains(&"Bash".to_string()));
        assert!(engine.allow_list.contains(&"Read".to_string()));
    }

    // TC-6.28: same-turn isolation — modifier only affects state AFTER apply_context_modifiers
    // The model used to build the LlmRequest is self.model BEFORE apply_context_modifiers runs.
    // We simulate this by checking that apply_context_modifiers mutates state only after the call.
    #[test]
    fn tc_6_28_modifier_applied_after_tool_execution_not_during() {
        let mut engine = make_engine("original", vec![]);
        // Capture model before applying
        let model_before = engine.model.clone();

        // Simulate: tools have executed, modifiers collected
        let modifiers = vec![Some(ContextModifier {
            model: Some("new-model".to_string()),
            effort: None,
            allowed_tools: vec![],
        })];

        // Before apply: model unchanged
        assert_eq!(engine.model, model_before);

        // After apply: model updated (affects next turn only)
        engine.apply_context_modifiers(&modifiers);
        assert_eq!(engine.model, "new-model");
        // model_before is unchanged as a value — representing what was sent to provider this turn
        assert_eq!(model_before, "original");
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
}
