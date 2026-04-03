use std::sync::{Arc, Mutex};

use crate::config::Config;
use crate::confirm::ToolConfirmer;
use crate::hooks::HookEngine;
use crate::output::OutputSink;
use crate::provider::{LlmProvider, ProviderError, create_provider};
use crate::session::{Session, SessionManager};
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

        let hooks_engine = HookEngine::new(config.hooks.clone());
        let hooks = if hooks_engine.has_hooks() {
            Some(hooks_engine)
        } else {
            None
        };

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
            hooks,
            session_manager,
            current_session: None,
            output,
            current_msg_id: String::new(),
            approval_manager: None,
            protocol_writer: None,
            allow_list,
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

        let hooks_engine = HookEngine::new(config.hooks.clone());
        let hooks = if hooks_engine.has_hooks() {
            Some(hooks_engine)
        } else {
            None
        };

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
            hooks,
            session_manager,
            current_session: Some(session),
            output,
            current_msg_id: String::new(),
            approval_manager: None,
            protocol_writer: None,
            allow_list,
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
                reasoning_effort: None,
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

            let tool_results = if let Some(ref approval_mgr) = self.approval_manager {
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
                    self.hooks.as_ref(),
                ).await {
                    Ok(results) => results,
                    Err(ExecutionControl::Quit) => {
                        self.save_session();
                        return Err(AgentError::UserAborted);
                    }
                }
            } else {
                // Terminal mode: use interactive confirmation
                match execute_tool_calls(&self.tools, &tool_calls, &self.confirmer, self.hooks.as_ref()).await {
                    Ok(results) => results,
                    Err(ExecutionControl::Quit) => {
                        self.save_session();
                        return Err(AgentError::UserAborted);
                    }
                }
            };

            // Display tool results
            for result in &tool_results {
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
                content: tool_results,
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
