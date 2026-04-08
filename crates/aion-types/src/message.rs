use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Unique identifier for a tool call
pub type ToolUseId = String;

/// A single content block within a message
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    /// Plain text content
    #[serde(rename = "text")]
    Text { text: String },

    /// A tool invocation from the assistant
    #[serde(rename = "tool_use")]
    ToolUse {
        id: ToolUseId,
        name: String,
        input: Value,
    },

    /// Result of a tool execution, sent back as user message
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: ToolUseId,
        content: String,
        is_error: bool,
    },

    /// Thinking block (Anthropic only, ignored for OpenAI)
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
}

/// A message in the conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
    Tool,
}

/// Why the model stopped generating
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// Model finished naturally
    EndTurn,
    /// Model wants to call tools
    ToolUse,
    /// Hit max_tokens limit
    MaxTokens,
}

/// Token usage statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- Role serialization / deserialization ---

    #[test]
    fn test_role_serialization_user() {
        // arrange
        let role = Role::User;
        // act
        let json = serde_json::to_string(&role).unwrap();
        // assert
        assert_eq!(json, "\"user\"");
    }

    #[test]
    fn test_role_serialization_assistant() {
        let role = Role::Assistant;
        let json = serde_json::to_string(&role).unwrap();
        assert_eq!(json, "\"assistant\"");
    }

    #[test]
    fn test_role_serialization_system() {
        let role = Role::System;
        let json = serde_json::to_string(&role).unwrap();
        assert_eq!(json, "\"system\"");
    }

    #[test]
    fn test_role_serialization_tool() {
        let role = Role::Tool;
        let json = serde_json::to_string(&role).unwrap();
        assert_eq!(json, "\"tool\"");
    }

    #[test]
    fn test_role_deserialization_roundtrip() {
        // arrange
        let variants = [
            (Role::User, "\"user\""),
            (Role::Assistant, "\"assistant\""),
            (Role::System, "\"system\""),
            (Role::Tool, "\"tool\""),
        ];
        // act + assert
        for (expected, raw) in &variants {
            let deserialized: Role = serde_json::from_str(raw).unwrap();
            assert_eq!(&deserialized, expected);
        }
    }

    // --- ContentBlock::Text ---

    #[test]
    fn test_content_block_text_construction() {
        // arrange + act
        let block = ContentBlock::Text {
            text: "hello".to_string(),
        };
        // assert
        match block {
            ContentBlock::Text { text } => assert_eq!(text, "hello"),
            _ => panic!("expected Text variant"),
        }
    }

    #[test]
    fn test_content_block_text_serialization() {
        // arrange
        let block = ContentBlock::Text {
            text: "hello world".to_string(),
        };
        // act
        let value = serde_json::to_value(&block).unwrap();
        // assert
        assert_eq!(value["type"], "text");
        assert_eq!(value["text"], "hello world");
    }

    // --- ContentBlock::ToolUse ---

    #[test]
    fn test_content_block_tool_use_construction() {
        // arrange + act
        let block = ContentBlock::ToolUse {
            id: "call_1".to_string(),
            name: "bash".to_string(),
            input: json!({"cmd": "ls"}),
        };
        // assert
        match &block {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "call_1");
                assert_eq!(name, "bash");
                assert_eq!(input["cmd"], "ls");
            }
            _ => panic!("expected ToolUse variant"),
        }
    }

    #[test]
    fn test_content_block_tool_use_serialization_type_field() {
        // arrange
        let block = ContentBlock::ToolUse {
            id: "call_1".to_string(),
            name: "bash".to_string(),
            input: json!({}),
        };
        // act
        let value = serde_json::to_value(&block).unwrap();
        // assert – the discriminant must be "tool_use"
        assert_eq!(value["type"], "tool_use");
        assert_eq!(value["id"], "call_1");
        assert_eq!(value["name"], "bash");
    }

    // --- ContentBlock::ToolResult ---

    #[test]
    fn test_content_block_tool_result_construction() {
        // arrange + act
        let block = ContentBlock::ToolResult {
            tool_use_id: "call_1".to_string(),
            content: "output text".to_string(),
            is_error: false,
        };
        // assert
        match &block {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "call_1");
                assert_eq!(content, "output text");
                assert!(!is_error);
            }
            _ => panic!("expected ToolResult variant"),
        }
    }

    #[test]
    fn test_content_block_tool_result_serialization() {
        // arrange
        let block = ContentBlock::ToolResult {
            tool_use_id: "call_1".to_string(),
            content: "ok".to_string(),
            is_error: false,
        };
        // act
        let value = serde_json::to_value(&block).unwrap();
        // assert
        assert_eq!(value["type"], "tool_result");
        assert_eq!(value["tool_use_id"], "call_1");
        assert_eq!(value["is_error"], false);
    }

    // --- StopReason variants ---

    #[test]
    fn test_stop_reason_end_turn_variant() {
        let reason = StopReason::EndTurn;
        assert_eq!(reason, StopReason::EndTurn);
    }

    #[test]
    fn test_stop_reason_tool_use_variant() {
        let reason = StopReason::ToolUse;
        assert_eq!(reason, StopReason::ToolUse);
    }

    #[test]
    fn test_stop_reason_max_tokens_variant() {
        let reason = StopReason::MaxTokens;
        assert_eq!(reason, StopReason::MaxTokens);
    }

    // --- TokenUsage default ---

    #[test]
    fn test_token_usage_default_all_zero() {
        // act
        let usage = TokenUsage::default();
        // assert
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
        assert_eq!(usage.cache_creation_tokens, 0);
        assert_eq!(usage.cache_read_tokens, 0);
    }

    // --- Message construction ---

    #[test]
    fn test_message_construction_text_content() {
        // arrange
        let content = vec![ContentBlock::Text {
            text: "Hello".to_string(),
        }];
        // act
        let msg = Message {
            role: Role::User,
            content,
        };
        // assert
        assert_eq!(msg.role, Role::User);
        assert_eq!(msg.content.len(), 1);
        match &msg.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "Hello"),
            _ => panic!("expected Text block"),
        }
    }

    #[test]
    fn test_message_construction_mixed_content() {
        // arrange
        let content = vec![
            ContentBlock::Text {
                text: "Calling tool".to_string(),
            },
            ContentBlock::ToolUse {
                id: "call_2".to_string(),
                name: "search".to_string(),
                input: json!({"query": "rust"}),
            },
        ];
        // act
        let msg = Message {
            role: Role::Assistant,
            content,
        };
        // assert
        assert_eq!(msg.role, Role::Assistant);
        assert_eq!(msg.content.len(), 2);
    }
}
