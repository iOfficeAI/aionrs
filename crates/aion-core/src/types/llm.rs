use serde_json::Value;

use super::message::{StopReason, TokenUsage, ToolUseId};
use super::tool::ToolDef;

/// A request to the LLM provider
#[derive(Debug, Clone)]
pub struct LlmRequest {
    pub model: String,
    pub system: String,
    pub messages: Vec<super::message::Message>,
    pub tools: Vec<ToolDef>,
    pub max_tokens: u32,
    /// Optional: thinking config (Anthropic only)
    pub thinking: Option<ThinkingConfig>,
    /// Optional: reasoning effort for OpenAI reasoning models (low/medium/high)
    pub reasoning_effort: Option<String>,
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
    },

    /// Thinking content (Anthropic only)
    ThinkingDelta(String),

    /// Response complete
    Done {
        stop_reason: StopReason,
        usage: TokenUsage,
    },

    /// Error from the API
    Error(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::message::{StopReason, TokenUsage};
    use serde_json::json;

    // --- ThinkingConfig variants ---

    #[test]
    fn test_thinking_config_enabled_stores_budget() {
        // arrange + act
        let config = ThinkingConfig::Enabled { budget_tokens: 4096 };
        // assert
        match config {
            ThinkingConfig::Enabled { budget_tokens } => assert_eq!(budget_tokens, 4096),
            ThinkingConfig::Disabled => panic!("expected Enabled variant"),
        }
    }

    #[test]
    fn test_thinking_config_disabled_variant() {
        // arrange + act
        let config = ThinkingConfig::Disabled;
        // assert
        matches!(config, ThinkingConfig::Disabled);
    }

    // --- LlmEvent variants ---

    #[test]
    fn test_llm_event_text_delta_carries_content() {
        // arrange + act
        let event = LlmEvent::TextDelta("hello".to_string());
        // assert
        match event {
            LlmEvent::TextDelta(text) => assert_eq!(text, "hello"),
            _ => panic!("expected TextDelta variant"),
        }
    }

    #[test]
    fn test_llm_event_tool_use_fields() {
        // arrange + act
        let event = LlmEvent::ToolUse {
            id: "call_1".to_string(),
            name: "bash".to_string(),
            input: json!({"cmd": "ls"}),
        };
        // assert
        match &event {
            LlmEvent::ToolUse { id, name, input } => {
                assert_eq!(id, "call_1");
                assert_eq!(name, "bash");
                assert_eq!(input["cmd"], "ls");
            }
            _ => panic!("expected ToolUse variant"),
        }
    }

    #[test]
    fn test_llm_event_thinking_delta_carries_content() {
        // arrange + act
        let event = LlmEvent::ThinkingDelta("reasoning text".to_string());
        // assert
        match event {
            LlmEvent::ThinkingDelta(text) => assert_eq!(text, "reasoning text"),
            _ => panic!("expected ThinkingDelta variant"),
        }
    }

    #[test]
    fn test_llm_event_done_carries_stop_reason_and_usage() {
        // arrange
        let usage = TokenUsage {
            input_tokens: 10,
            output_tokens: 20,
            cache_creation_tokens: 0,
            cache_read_tokens: 5,
        };
        // act
        let event = LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            usage: usage.clone(),
        };
        // assert
        match event {
            LlmEvent::Done { stop_reason, usage } => {
                assert_eq!(stop_reason, StopReason::EndTurn);
                assert_eq!(usage.input_tokens, 10);
                assert_eq!(usage.output_tokens, 20);
                assert_eq!(usage.cache_read_tokens, 5);
            }
            _ => panic!("expected Done variant"),
        }
    }

    #[test]
    fn test_llm_event_done_tool_use_stop_reason() {
        // arrange + act
        let event = LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            usage: TokenUsage::default(),
        };
        // assert
        match event {
            LlmEvent::Done { stop_reason, .. } => assert_eq!(stop_reason, StopReason::ToolUse),
            _ => panic!("expected Done variant"),
        }
    }

    #[test]
    fn test_llm_event_error_carries_message() {
        // arrange + act
        let event = LlmEvent::Error("rate limit exceeded".to_string());
        // assert
        match event {
            LlmEvent::Error(msg) => assert_eq!(msg, "rate limit exceeded"),
            _ => panic!("expected Error variant"),
        }
    }
}
