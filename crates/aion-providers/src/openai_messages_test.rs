use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use aion_types::message::{ContentBlock, Message, Role};

    fn no_compat() -> ProviderCompat {
        ProviderCompat::default()
    }

    fn openai_compat() -> ProviderCompat {
        ProviderCompat::openai_defaults()
    }

    #[test]
    fn test_merge_assistant_messages_enabled() {
        let messages = vec![
            Message::new(Role::Assistant, vec![ContentBlock::Text { text: "hello".into() }]),
            Message::new(Role::Assistant, vec![ContentBlock::Text { text: " world".into() }]),
        ];
        let result = build_messages(&messages, "", &openai_compat());
        let assistant_msgs: Vec<_> = result.iter().filter(|m| m["role"] == "assistant").collect();
        assert_eq!(assistant_msgs.len(), 1);
        assert_eq!(assistant_msgs[0]["content"], "hello world");
    }

    #[test]
    fn test_merge_assistant_messages_disabled() {
        let messages = vec![
            Message::new(Role::Assistant, vec![ContentBlock::Text { text: "hello".into() }]),
            Message::new(Role::Assistant, vec![ContentBlock::Text { text: " world".into() }]),
        ];
        let result = build_messages(&messages, "", &no_compat());
        let assistant_msgs: Vec<_> = result.iter().filter(|m| m["role"] == "assistant").collect();
        assert_eq!(assistant_msgs.len(), 2);
    }

    #[test]
    fn test_reasoning_content_projects_only_message_thinking() {
        let messages = vec![
            Message::new(Role::User, vec![ContentBlock::Text { text: "q1".into() }]),
            Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::Thinking {
                        thinking: "private chain".into(),
                        signature: None,
                    },
                    ContentBlock::Text {
                        text: "first answer".into(),
                    },
                ],
            ),
            Message::new(Role::User, vec![ContentBlock::Text { text: "q2".into() }]),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::Text {
                    text: "second answer".into(),
                }],
            ),
        ];

        let result = build_messages(&messages, "", &openai_compat());
        let assistant_msgs: Vec<_> = result.iter().filter(|m| m["role"] == "assistant").collect();

        assert_eq!(assistant_msgs.len(), 2);
        assert_eq!(assistant_msgs[0]["reasoning_content"], "private chain");
        assert!(
            assistant_msgs[1].get("reasoning_content").is_none(),
            "assistant messages without Thinking blocks must not receive empty reasoning_content"
        );
    }

    #[test]
    fn test_reasoning_content_merge_drops_empty_replay_values() {
        let mut messages = vec![
            json!({
                "role": "assistant",
                "content": "first",
                "reasoning_content": ""
            }),
            json!({
                "role": "assistant",
                "content": " second"
            }),
        ];

        merge_consecutive_assistant(&mut messages);

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["content"], "first second");
        assert!(
            messages[0].get("reasoning_content").is_none(),
            "merged assistant message should not retain an empty reasoning_content field"
        );
    }

    #[test]
    fn test_forward_and_reverse_orphan_cleanup_do_not_conflict() {
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::ToolUse {
                        id: "matched".into(),
                        name: "Bash".into(),
                        input: json!({"command":"pwd"}),
                        extra: None,
                    },
                    ContentBlock::ToolUse {
                        id: "forward_orphan".into(),
                        name: "Read".into(),
                        input: json!({"file_path":"x"}),
                        extra: None,
                    },
                ],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "matched".into(),
                    content: "ok".into(),
                    is_error: false,
                }],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "reverse_orphan".into(),
                    content: "bad".into(),
                    is_error: true,
                }],
            ),
        ];

        let result = build_messages(&messages, "", &openai_compat());
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        let tcs = assistant["tool_calls"].as_array().unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0]["id"], "matched");
        let tool_msgs: Vec<_> = result.iter().filter(|m| m["role"] == "tool").collect();
        assert_eq!(tool_msgs.len(), 1);
        assert_eq!(tool_msgs[0]["tool_call_id"], "matched");
    }

    #[test]
    fn test_clean_orphan_tool_calls_enabled() {
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::ToolUse {
                        id: "tc1".into(),
                        name: "bash".into(),
                        input: json!({}),
                        extra: None,
                    },
                    ContentBlock::ToolUse {
                        id: "tc2".into(),
                        name: "read".into(),
                        input: json!({}),
                        extra: None,
                    },
                ],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "tc1".into(),
                    content: "ok".into(),
                    is_error: false,
                }],
            ),
            // tc2 has no result -> orphan
        ];
        let result = build_messages(&messages, "", &openai_compat());
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        let tcs = assistant["tool_calls"].as_array().unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0]["id"], "tc1");
    }

    #[test]
    fn test_clean_orphan_tool_calls_disabled() {
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::ToolUse {
                        id: "tc1".into(),
                        name: "bash".into(),
                        input: json!({}),
                        extra: None,
                    },
                    ContentBlock::ToolUse {
                        id: "tc2".into(),
                        name: "read".into(),
                        input: json!({}),
                        extra: None,
                    },
                ],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "tc1".into(),
                    content: "ok".into(),
                    is_error: false,
                }],
            ),
        ];
        let result = build_messages(&messages, "", &no_compat());
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        let tcs = assistant["tool_calls"].as_array().unwrap();
        assert_eq!(tcs.len(), 2);
    }

    #[test]
    fn test_reverse_orphan_tool_result_dropped() {
        let messages = vec![Message::new(
            Role::Tool,
            vec![ContentBlock::ToolResult {
                tool_use_id: "missing".into(),
                content: "orphan".into(),
                is_error: true,
            }],
        )];
        let result = build_messages(&messages, "", &openai_compat());
        assert!(result.iter().all(|m| m["role"] != "tool"));
    }

    #[test]
    fn test_reverse_orphan_tool_result_kept_when_disabled() {
        let mut compat = openai_compat();
        compat.messages.clean_orphan_tool_results = Some(false);
        let messages = vec![Message::new(
            Role::Tool,
            vec![ContentBlock::ToolResult {
                tool_use_id: "missing".into(),
                content: "orphan".into(),
                is_error: true,
            }],
        )];
        let result = build_messages(&messages, "", &compat);
        assert!(
            result
                .iter()
                .any(|m| { m["role"] == "tool" && m["tool_call_id"] == "missing" && m["content"] == "orphan" })
        );
    }

    #[test]
    fn test_matched_tool_result_not_dropped_by_reverse_cleanup() {
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "call_x".into(),
                    name: "Bash".into(),
                    input: json!({"command":"ls"}),
                    extra: None,
                }],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "call_x".into(),
                    content: "ok".into(),
                    is_error: false,
                }],
            ),
        ];
        let result = build_messages(&messages, "", &openai_compat());
        assert!(
            result
                .iter()
                .any(|m| m["role"] == "tool" && m["tool_call_id"] == "call_x")
        );
    }

    #[test]
    fn test_empty_id_toolcall_downgraded_when_auto_id_disabled() {
        let mut compat = openai_compat();
        compat.tools.auto_tool_id = Some(false);
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "".into(),
                    name: "Bash".into(),
                    input: json!({"command":"ls"}),
                    extra: None,
                }],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "".into(),
                    content: "orphan".into(),
                    is_error: true,
                }],
            ),
        ];
        let result = build_messages(&messages, "", &compat);
        assert!(result.iter().all(|m| m["role"] != "tool"));
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        assert!(assistant.get("tool_calls").is_none());
        let content = assistant["content"].as_str().unwrap();
        assert!(content.contains("[tool call skipped:"));
        assert!(content.contains("empty tool call id"));
        assert!(content.contains("arguments={\"command\":\"ls\"}"));
    }

    #[test]
    fn test_empty_id_toolcall_generates_id_when_auto_id_enabled() {
        let mut compat = openai_compat();
        compat.tools.auto_tool_id = Some(true);
        compat.tools.clean_orphan_tool_calls = Some(false);
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "".into(),
                name: "Bash".into(),
                input: json!({"command":"ls"}),
                extra: None,
            }],
        )];
        let result = build_messages(&messages, "", &compat);
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        let tc = &assistant["tool_calls"][0];
        assert_eq!(tc["function"]["name"], "Bash");
        assert!(tc["id"].as_str().unwrap().starts_with("call_"));
        assert_ne!(tc["id"], "");
    }

    #[test]
    fn test_empty_id_toolcall_rewrites_paired_result_when_auto_id_enabled() {
        let mut compat = openai_compat();
        compat.tools.auto_tool_id = Some(true);
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "".into(),
                    name: "Bash".into(),
                    input: json!({"command":"ls"}),
                    extra: None,
                }],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "".into(),
                    content: "ok".into(),
                    is_error: false,
                }],
            ),
        ];
        let result = build_messages(&messages, "", &compat);
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        let generated_id = assistant["tool_calls"][0]["id"].as_str().unwrap();
        assert!(generated_id.starts_with("call_"));
        let tool = result.iter().find(|m| m["role"] == "tool").unwrap();
        assert_eq!(tool["tool_call_id"], generated_id);
        assert_eq!(tool["content"], "ok");
    }

    #[test]
    fn test_result_before_matching_call_is_dropped() {
        let messages = vec![
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "late".into(),
                    content: "too early".into(),
                    is_error: true,
                }],
            ),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "late".into(),
                    name: "Bash".into(),
                    input: json!({"command":"ls"}),
                    extra: None,
                }],
            ),
        ];
        let result = build_messages(&messages, "", &openai_compat());
        assert!(result.iter().all(|m| m["role"] != "tool"));
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        assert!(assistant.get("tool_calls").is_none());
        assert_eq!(assistant["content"], "");
    }

    #[test]
    fn test_dropped_empty_id_does_not_consume_later_generated_empty_id_result() {
        let mut compat = openai_compat();
        compat.tools.auto_tool_id = Some(true);
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "".into(),
                    name: "".into(),
                    input: json!({"bad":true}),
                    extra: None,
                }],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "".into(),
                    content: "bad result".into(),
                    is_error: true,
                }],
            ),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "".into(),
                    name: "Bash".into(),
                    input: json!({"command":"ls"}),
                    extra: None,
                }],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "".into(),
                    content: "ok".into(),
                    is_error: false,
                }],
            ),
        ];
        let result = build_messages(&messages, "", &compat);
        let assistant_with_call = result
            .iter()
            .find(|m| m["tool_calls"].as_array().is_some_and(|calls| !calls.is_empty()))
            .unwrap();
        let generated_id = assistant_with_call["tool_calls"][0]["id"].as_str().unwrap();
        let tool_msgs: Vec<_> = result.iter().filter(|m| m["role"] == "tool").collect();
        assert_eq!(tool_msgs.len(), 1);
        assert_eq!(tool_msgs[0]["tool_call_id"], generated_id);
        assert_eq!(tool_msgs[0]["content"], "ok");
    }

    #[test]
    fn test_empty_name_toolcall_downgraded_and_paired_result_dropped() {
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::Text { text: "writing".into() },
                    ContentBlock::ToolUse {
                        id: "call_x".into(),
                        name: "".into(),
                        input: json!({}),
                        extra: None,
                    },
                ],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "call_x".into(),
                    content: "Unknown tool: ".into(),
                    is_error: true,
                }],
            ),
        ];
        let result = build_messages(&messages, "", &openai_compat());
        assert!(
            result.iter().all(|m| m["role"] != "tool"),
            "paired tool result must be dropped"
        );
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        let has_empty = assistant
            .get("tool_calls")
            .and_then(|t| t.as_array())
            .map(|a| a.iter().any(|tc| tc["function"]["name"] == ""))
            .unwrap_or(false);
        assert!(!has_empty, "no empty-name tool_call in projection");
        assert!(assistant["content"].as_str().unwrap().contains("[tool call skipped:"));
        assert!(assistant["content"].as_str().unwrap().contains("writing"));
    }

    #[test]
    fn test_mixed_valid_and_empty_name() {
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::ToolUse {
                        id: "ok".into(),
                        name: "Bash".into(),
                        input: json!({"command":"ls"}),
                        extra: None,
                    },
                    ContentBlock::ToolUse {
                        id: "bad".into(),
                        name: "".into(),
                        input: json!({}),
                        extra: None,
                    },
                ],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "ok".into(),
                    content: "ok".into(),
                    is_error: false,
                }],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "bad".into(),
                    content: "Unknown tool: ".into(),
                    is_error: true,
                }],
            ),
        ];
        let result = build_messages(&messages, "", &openai_compat());
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        let tcs = assistant["tool_calls"].as_array().unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0]["function"]["name"], "Bash");
        let tool_msgs: Vec<_> = result.iter().filter(|m| m["role"] == "tool").collect();
        assert_eq!(tool_msgs.len(), 1);
        assert_eq!(tool_msgs[0]["tool_call_id"], "ok");
    }

    #[test]
    fn test_only_empty_name_yields_placeholder_content() {
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "call_x".into(),
                name: "".into(),
                input: json!({}),
                extra: None,
            }],
        )];
        let result = build_messages(&messages, "", &openai_compat());
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        assert!(assistant.get("tool_calls").is_none());
        let content = assistant["content"].as_str().unwrap();
        assert!(content.contains("[tool call skipped:"));
        assert!(content.contains("arguments={}"));
    }

    #[test]
    fn test_thinking_only_assistant_keeps_empty_content() {
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::Thinking {
                thinking: "internal reasoning".into(),
                signature: None,
            }],
        )];
        let result = build_messages(&messages, "", &openai_compat());
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        assert_eq!(assistant["content"], "");
        assert!(!assistant["content"].as_str().unwrap().contains("malformed"));
    }

    #[test]
    fn test_empty_name_toolcall_with_user_tool_result_dropped() {
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "call_x".into(),
                    name: "".into(),
                    input: json!({}),
                    extra: None,
                }],
            ),
            Message::new(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "call_x".into(),
                    content: "Unknown tool: ".into(),
                    is_error: true,
                }],
            ),
        ];
        let result = build_messages(&messages, "", &openai_compat());
        assert!(result.iter().all(|m| m["role"] != "tool"));
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        assert!(assistant["content"].as_str().unwrap().contains("[tool call skipped:"));
    }

    #[test]
    fn test_two_empty_name_calls_produce_two_lines() {
        let messages = vec![Message::new(
            Role::Assistant,
            vec![
                ContentBlock::ToolUse {
                    id: "a".into(),
                    name: "".into(),
                    input: json!({"x":1}),
                    extra: None,
                },
                ContentBlock::ToolUse {
                    id: "b".into(),
                    name: "".into(),
                    input: json!({"y":2}),
                    extra: None,
                },
            ],
        )];
        let result = build_messages(&messages, "", &openai_compat());
        let content = result.iter().find(|m| m["role"] == "assistant").unwrap()["content"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(content.matches("[tool call skipped:").count(), 2);
        assert!(content.contains("{\"x\":1}") && content.contains("{\"y\":2}"));
    }

    #[test]
    fn test_sanitize_disabled_keeps_empty_name() {
        let mut compat = openai_compat();
        compat.tools.sanitize_malformed_tool_calls = Some(false);
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "call_x".into(),
                name: "".into(),
                input: json!({}),
                extra: None,
            }],
        )];
        let result = build_messages(&messages, "", &compat);
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        assert_eq!(assistant["tool_calls"][0]["function"]["name"], "");
    }

    #[test]
    fn test_normal_toolcall_unaffected() {
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "call_x".into(),
                    name: "Bash".into(),
                    input: json!({"command":"ls"}),
                    extra: None,
                }],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "call_x".into(),
                    content: "ok".into(),
                    is_error: false,
                }],
            ),
        ];
        let result = build_messages(&messages, "", &openai_compat());
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        assert_eq!(assistant["tool_calls"][0]["function"]["name"], "Bash");
        assert!(
            result
                .iter()
                .any(|m| m["role"] == "tool" && m["tool_call_id"] == "call_x")
        );
    }

    #[test]
    fn test_dedup_tool_results_enabled() {
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "tc1".into(),
                    name: "bash".into(),
                    input: json!({}),
                    extra: None,
                }],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "tc1".into(),
                    content: "first".into(),
                    is_error: false,
                }],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "tc1".into(),
                    content: "second".into(),
                    is_error: false,
                }],
            ),
        ];
        let result = build_messages(&messages, "", &openai_compat());
        let tool_msgs: Vec<_> = result.iter().filter(|m| m["role"] == "tool").collect();
        assert_eq!(tool_msgs.len(), 1);
        assert_eq!(tool_msgs[0]["content"], "second");
    }
}
