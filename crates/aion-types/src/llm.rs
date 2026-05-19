use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::message::{StopReason, TokenUsage, ToolUseId};
use crate::tool::ToolDef;

/// A request to the LLM provider
#[derive(Debug, Clone)]
pub struct LlmRequest {
    /// Optional stable conversation/session identifier for providers that use
    /// request identity for routing, prompt caching, or concurrency accounting.
    pub session_id: Option<String>,
    pub model: String,
    pub system: String,
    pub messages: Vec<crate::message::Message>,
    pub tools: Vec<ToolDef>,
    pub max_tokens: u32,
    /// Optional: thinking config (Anthropic extended thinking)
    pub thinking: Option<ThinkingConfig>,
    /// Optional: reasoning effort for OpenAI reasoning models (low/medium/high)
    pub reasoning_effort: Option<String>,
}

/// Provider metadata surfaced to higher layers for model pickers and status UIs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProviderMetadata {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ProviderModelInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_limits: Option<AccountLimitsInfo>,
}

/// Metadata for one model exposed by a provider.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderModelInfo {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effort_levels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
}

/// Account-level limits and quota metadata for a provider.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AccountLimitsInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_type: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limits: Vec<AccountLimitInfo>,
}

/// Rate-limit metadata for one limit bucket.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AccountLimitInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<AccountLimitWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary: Option<AccountLimitWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credits: Option<AccountCreditsInfo>,
}

/// Snapshot for one rate-limit window.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AccountLimitWindow {
    pub used_percent: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_minutes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
}

/// Credit/quota balance metadata when the provider exposes it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountCreditsInfo {
    pub has_credits: bool,
    pub unlimited: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub balance: Option<String>,
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
    use crate::message::{StopReason, TokenUsage};
    use serde_json::json;

    #[test]
    fn test_thinking_config_enabled_stores_budget() {
        let config = ThinkingConfig::Enabled {
            budget_tokens: 4096,
        };
        match config {
            ThinkingConfig::Enabled { budget_tokens } => assert_eq!(budget_tokens, 4096),
            ThinkingConfig::Disabled => panic!("expected Enabled"),
        }
    }

    #[test]
    fn test_llm_event_text_delta_carries_content() {
        let event = LlmEvent::TextDelta("hello".to_string());
        match event {
            LlmEvent::TextDelta(text) => assert_eq!(text, "hello"),
            _ => panic!("expected TextDelta"),
        }
    }

    #[test]
    fn test_llm_event_done_carries_stop_reason_and_usage() {
        let usage = TokenUsage {
            input_tokens: 10,
            output_tokens: 20,
            cache_creation_tokens: 0,
            cache_read_tokens: 5,
        };
        let event = LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            usage,
        };
        match event {
            LlmEvent::Done { stop_reason, usage } => {
                assert_eq!(stop_reason, StopReason::EndTurn);
                assert_eq!(usage.input_tokens, 10);
            }
            _ => panic!("expected Done"),
        }
    }

    #[test]
    fn test_llm_event_tool_use_fields() {
        let event = LlmEvent::ToolUse {
            id: "call_1".to_string(),
            name: "bash".to_string(),
            input: json!({"cmd": "ls"}),
            extra: None,
        };
        match &event {
            LlmEvent::ToolUse {
                id, name, input, ..
            } => {
                assert_eq!(id, "call_1");
                assert_eq!(name, "bash");
                assert_eq!(input["cmd"], "ls");
            }
            _ => panic!("expected ToolUse"),
        }
    }

    #[test]
    fn test_provider_metadata_serialization_omits_empty_fields() {
        let metadata = ProviderMetadata::default();
        let json = serde_json::to_value(&metadata).unwrap();

        assert!(json.get("models").is_none());
        assert!(json.get("account_limits").is_none());
    }

    #[test]
    fn test_provider_model_info_serialization_includes_context_and_effort() {
        let metadata = ProviderMetadata {
            models: vec![ProviderModelInfo {
                id: "gpt-5-codex".to_string(),
                display_name: Some("GPT-5 Codex".to_string()),
                context_window: Some(272_000),
                effort_levels: vec!["low".to_string(), "medium".to_string()],
                default_effort: Some("medium".to_string()),
            }],
            account_limits: Some(AccountLimitsInfo {
                plan_type: Some("pro".to_string()),
                limits: vec![AccountLimitInfo {
                    limit_id: Some("codex".to_string()),
                    limit_name: None,
                    primary: Some(AccountLimitWindow {
                        used_percent: 42.0,
                        window_minutes: Some(5),
                        resets_at: Some(123),
                    }),
                    secondary: None,
                    credits: Some(AccountCreditsInfo {
                        has_credits: true,
                        unlimited: false,
                        balance: Some("9.99".to_string()),
                    }),
                }],
            }),
        };

        let json = serde_json::to_value(&metadata).unwrap();
        assert_eq!(json["models"][0]["id"], "gpt-5-codex");
        assert_eq!(json["models"][0]["context_window"], 272_000);
        assert_eq!(json["models"][0]["default_effort"], "medium");
        assert_eq!(json["account_limits"]["plan_type"], "pro");
        assert_eq!(
            json["account_limits"]["limits"][0]["credits"]["balance"],
            "9.99"
        );
    }
}
