use super::*;

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
            extra: None,
        };
        // assert
        match &block {
            ContentBlock::ToolUse { id, name, input, .. } => {
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
            extra: None,
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

    #[test]
    fn thinking_block_deserializes_without_signature() {
        let block: ContentBlock = serde_json::from_value(json!({
            "type": "thinking",
            "thinking": "reasoning"
        }))
        .unwrap();

        match block {
            ContentBlock::Thinking { thinking, signature } => {
                assert_eq!(thinking, "reasoning");
                assert!(signature.is_none());
            }
            _ => panic!("expected thinking block"),
        }
    }

    #[test]
    fn thinking_block_serializes_signature_when_present() {
        let block = ContentBlock::Thinking {
            thinking: "reasoning".to_string(),
            signature: Some("sig-123".to_string()),
        };

        let value = serde_json::to_value(block).unwrap();

        assert_eq!(value["type"], "thinking");
        assert_eq!(value["thinking"], "reasoning");
        assert_eq!(value["signature"], "sig-123");
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
        let content = vec![ContentBlock::Text {
            text: "Hello".to_string(),
        }];
        let msg = Message::new(Role::User, content);
        assert_eq!(msg.role, Role::User);
        assert_eq!(msg.content.len(), 1);
        assert!(msg.timestamp.is_none());
        match &msg.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "Hello"),
            _ => panic!("expected Text block"),
        }
    }

    #[test]
    fn test_message_construction_mixed_content() {
        let content = vec![
            ContentBlock::Text {
                text: "Calling tool".to_string(),
            },
            ContentBlock::ToolUse {
                id: "call_2".to_string(),
                name: "search".to_string(),
                input: json!({"query": "rust"}),
                extra: None,
            },
        ];
        let msg = Message::new(Role::Assistant, content);
        assert_eq!(msg.role, Role::Assistant);
        assert_eq!(msg.content.len(), 2);
        assert!(msg.timestamp.is_none());
    }

    #[test]
    fn test_message_now_has_timestamp() {
        let before = Utc::now();
        let msg = Message::now(Role::User, vec![ContentBlock::Text { text: "hi".to_string() }]);
        let after = Utc::now();
        let ts = msg.timestamp.expect("Message::now should set timestamp");
        assert!(ts >= before && ts <= after);
    }

    #[test]
    fn test_message_timestamp_serialization_roundtrip() {
        let msg = Message::now(
            Role::User,
            vec![ContentBlock::Text {
                text: "hello".to_string(),
            }],
        );
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("timestamp"));

        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back.timestamp, msg.timestamp);
    }

    #[test]
    fn test_message_timestamp_backward_compat_deserialization() {
        // Old JSON without timestamp field should deserialize with timestamp = None
        let json = r#"{"role":"user","content":[{"type":"text","text":"hi"}]}"#;
        let msg: Message = serde_json::from_str(json).unwrap();
        assert!(msg.timestamp.is_none());
    }

    #[test]
    fn test_message_new_skips_timestamp_in_json() {
        let msg = Message::new(Role::User, vec![ContentBlock::Text { text: "hi".to_string() }]);
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            !json.contains("timestamp"),
            "None timestamp should be omitted via skip_serializing_if"
        );
    }
}
