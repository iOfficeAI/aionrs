use serde_json::Value;

use crate::message::{StopReason, TokenUsage, ToolUseId};
use crate::tool::ToolDef;

/// A request to the LLM provider
#[derive(Debug, Clone)]
pub struct LlmRequest {
    pub model: String,
    pub system: String,
    pub messages: Vec<crate::message::Message>,
    pub tools: Vec<ToolDef>,
    /// Provider-neutral tool selection policy for this model turn.
    pub tool_choice: Option<ToolChoice>,
    pub max_tokens: Option<u32>,
    /// Optional: thinking config (Anthropic extended thinking)
    pub thinking: Option<ThinkingConfig>,
    /// Optional: reasoning effort for OpenAI reasoning models (low/medium/high)
    pub reasoning_effort: Option<String>,
}

/// Controls whether the model may or must select an advertised tool.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ToolChoice {
    /// Let the model decide whether to call a tool.
    #[default]
    Auto,
    /// Require the model to call one of the advertised tools.
    Required,
}

impl ToolChoice {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Required => "required",
        }
    }
}

#[derive(Debug, Clone)]
pub enum ThinkingConfig {
    Enabled { budget_tokens: u32 },
    Disabled,
}

/// Streaming events from the LLM
#[derive(Debug, Clone)]
pub enum LlmEvent {
    /// Incremental text output
    TextDelta(String),
    /// Complete tool call (after accumulating streaming deltas)
    ToolUse {
        id: ToolUseId,
        name: String,
        input: Value,
        /// Opaque provider metadata (e.g. Gemini thought_signature) to round-trip.
        extra: Option<Value>,
    },
    /// Thinking content (Anthropic only)
    ThinkingDelta(String),
    /// Opaque provider signature for the current thinking block.
    ThinkingSignature(String),
    /// Opaque provider output item that must be persisted and replayed.
    ProviderItem { provider: String, item: Value },
    /// Response complete
    Done { stop_reason: StopReason, usage: TokenUsage },
    /// Error from the API
    Error(String),
}

#[cfg(test)]
#[path = "llm_test.rs"]
mod llm_test;
