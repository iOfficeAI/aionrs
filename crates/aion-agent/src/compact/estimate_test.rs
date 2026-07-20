use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use aion_types::message::{ImageUrl, Message, Role};
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use serde_json::json;

    #[test]
    fn empty_messages_returns_zero() {
        assert_eq!(estimate_tokens_from_messages(&[]), 0);
    }

    #[test]
    fn text_only_message() {
        let text = "a".repeat(400);
        let msg = Message::new(Role::User, vec![ContentBlock::Text { text }]);
        assert_eq!(estimate_tokens_from_messages(&[msg]), 100);
    }

    #[test]
    fn tool_use_message_uses_json_ratio() {
        let input = json!({"cmd": "ls -la"});
        let input_len = "ExecCommand".len() + input.to_string().len();
        let msg = Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "call_1".into(),
                name: "ExecCommand".into(),
                input,
                extra: None,
            }],
        );
        let result = estimate_tokens_from_messages(&[msg]);
        assert_eq!(result, (input_len / CHARS_PER_TOKEN_JSON) as u64);
    }

    #[test]
    fn tool_result_uses_text_ratio() {
        let content = "x".repeat(800);
        let msg = Message::new(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".into(),
                content,
                is_error: false,
            }],
        );
        assert_eq!(estimate_tokens_from_messages(&[msg]), 200);
    }

    #[test]
    fn mixed_conversation_accumulates() {
        let messages = vec![
            Message::new(Role::User, vec![ContentBlock::Text { text: "a".repeat(400) }]),
            Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::Text { text: "b".repeat(200) },
                    ContentBlock::ToolUse {
                        id: "c1".into(),
                        name: "Read".into(),
                        input: json!({"path": "/foo/bar.rs"}),
                        extra: None,
                    },
                ],
            ),
            Message::new(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "c1".into(),
                    content: "c".repeat(1200),
                    is_error: false,
                }],
            ),
        ];
        let estimate = estimate_tokens_from_messages(&messages);
        // text_tokens = (400 + 200 + 1200) / 4 = 450
        // json_tokens = ("Read".len() + json_string.len()) / 3
        assert!(estimate > 450);
        assert!(estimate < 600);
    }

    #[test]
    fn thinking_block_counted() {
        let thinking = "t".repeat(4000);
        let msg = Message::new(
            Role::Assistant,
            vec![ContentBlock::Thinking {
                thinking,
                signature: None,
            }],
        );
        assert_eq!(estimate_tokens_from_messages(&[msg]), 1000);
    }

    #[test]
    fn large_conversation_realistic_estimate() {
        let big_result = "x".repeat(400_000);
        let messages = vec![Message::new(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "c1".into(),
                content: big_result,
                is_error: false,
            }],
        )];
        let estimate = estimate_tokens_from_messages(&messages);
        assert_eq!(estimate, 100_000);
    }

    #[test]
    fn final_tool_result_is_estimated_directly() {
        let result = ContentBlock::ToolResult {
            tool_use_id: "c1".into(),
            content: "x".repeat(400_000),
            is_error: false,
        };

        assert_eq!(estimate_tokens_from_tool_result(&result), 100_000);
    }

    #[test]
    fn non_tool_result_is_not_counted_as_tool_output() {
        let text = ContentBlock::Text { text: "x".repeat(400) };

        assert_eq!(estimate_tokens_from_tool_result(&text), 0);
    }

    #[test]
    fn image_block_uses_decoded_size_not_base64_length() {
        // 10_000 decoded bytes -> 10_000 / 750 = 13 tokens, clamped to minimum 85.
        let image_bytes = vec![0u8; 10_000];
        let data = STANDARD.encode(&image_bytes);
        let msg = Message::new(
            Role::User,
            vec![ContentBlock::Image {
                image_url: ImageUrl {
                    url: format!("data:image/png;base64,{}", data),
                },
            }],
        );
        let estimate = estimate_tokens_from_messages(&[msg]);
        // 85 tokens * 4 chars/token = 340 chars counted as text.
        assert_eq!(estimate, 85);
        // The old base64-length heuristic would have counted ~13_333 chars -> ~3_333 tokens.
        assert!(
            estimate < 1000,
            "image estimate should be much smaller than base64 length heuristic"
        );
    }

    #[test]
    fn image_block_estimate_respects_maximum() {
        // A huge image should be capped, not grow with base64 length.
        let image_bytes = vec![0u8; 10_000_000];
        let data = STANDARD.encode(&image_bytes);
        let msg = Message::new(
            Role::User,
            vec![ContentBlock::Image {
                image_url: ImageUrl {
                    url: format!("data:image/png;base64,{}", data),
                },
            }],
        );
        let estimate = estimate_tokens_from_messages(&[msg]);
        // Maximum 2048 tokens * 4 chars/token.
        assert_eq!(estimate, 2048);
    }

    #[test]
    fn tool_image_is_estimated_directly() {
        let data = STANDARD.encode(vec![0u8; 10_000]);
        let image = ContentBlock::Image {
            image_url: ImageUrl {
                url: format!("data:image/png;base64,{}", data),
            },
        };

        assert_eq!(estimate_tokens_from_tool_image(&image), 85);
    }
}
