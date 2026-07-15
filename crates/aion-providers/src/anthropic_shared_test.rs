use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    use aion_config::compat::{MessageCompat, ToolCompat};
    use aion_config::schema::legalize_json_schema;
    use aion_types::message::ImageUrl;
    use aion_types::tool::ToolDef;
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use serde_json::json;

    /// Compat with merge but no alternation — matches pre-compat behavior
    fn default_compat() -> ProviderCompat {
        ProviderCompat {
            messages: MessageCompat {
                merge_same_role: Some(true),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn anthropic_compat() -> ProviderCompat {
        ProviderCompat::anthropic_defaults()
    }

    // --- build_messages tests ---

    #[test]
    fn test_build_messages_text_only() {
        let messages = vec![Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        )];
        let result = build_messages(&messages, &default_compat());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["role"], "user");
        let content = result[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "Hello");
    }

    #[test]
    fn test_build_messages_with_tool_use() {
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "call_1".to_string(),
                name: "bash".to_string(),
                input: json!({"cmd": "ls"}),
                extra: None,
            }],
        )];
        let result = build_messages(&messages, &default_compat());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["role"], "assistant");
        let content = result[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "tool_use");
        assert_eq!(content[0]["id"], "call_1");
        assert_eq!(content[0]["name"], "bash");
        assert_eq!(content[0]["input"]["cmd"], "ls");
    }

    #[test]
    fn test_build_messages_with_tool_result() {
        let messages = vec![Message::new(
            Role::Tool,
            vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".to_string(),
                content: "file list".to_string(),
                is_error: false,
            }],
        )];
        let result = build_messages(&messages, &default_compat());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["role"], "user"); // Tool maps to "user"
        let content = result[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "tool_result");
        assert_eq!(content[0]["tool_use_id"], "call_1");
        assert_eq!(content[0]["content"], "file list");
        assert_eq!(content[0]["is_error"], false);
    }

    #[test]
    fn test_build_messages_with_thinking() {
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::Thinking {
                thinking: "Let me think...".to_string(),
                signature: None,
            }],
        )];
        let result = build_messages(&messages, &default_compat());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["role"], "assistant");
        let content = result[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[0]["thinking"], "Let me think...");
    }

    #[test]
    fn test_build_messages_with_thinking_signature() {
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::Thinking {
                thinking: "Let me think...".to_string(),
                signature: Some("sig-123".to_string()),
            }],
        )];

        let result = build_messages(&messages, &default_compat());

        let content = result[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[0]["thinking"], "Let me think...");
        assert_eq!(content[0]["signature"], "sig-123");
    }

    // --- compat-driven behavior tests ---

    #[test]
    fn test_ensure_alternation_inserts_user_filler_before_assistant() {
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::Text { text: "hi".into() }],
        )];
        let compat = ProviderCompat {
            messages: MessageCompat {
                ensure_alternation: Some(true),
                merge_same_role: Some(true),
                ..Default::default()
            },
            ..Default::default()
        };
        let result = build_messages(&messages, &compat);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0]["role"], "user");
        assert_eq!(result[1]["role"], "assistant");
    }

    #[test]
    fn test_ensure_alternation_disabled_no_filler() {
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::Text { text: "hi".into() }],
        )];
        let compat = ProviderCompat {
            messages: MessageCompat {
                ensure_alternation: Some(false),
                ..Default::default()
            },
            ..Default::default()
        };
        let result = build_messages(&messages, &compat);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["role"], "assistant");
    }

    #[test]
    fn test_merge_same_role_enabled_merges_consecutive_user() {
        let messages = vec![
            Message::new(Role::User, vec![ContentBlock::Text { text: "a".into() }]),
            Message::new(Role::User, vec![ContentBlock::Text { text: "b".into() }]),
        ];
        let compat = ProviderCompat {
            messages: MessageCompat {
                merge_same_role: Some(true),
                ..Default::default()
            },
            ..Default::default()
        };
        let result = build_messages(&messages, &compat);
        assert_eq!(result.len(), 1);
        let content = result[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
    }

    #[test]
    fn test_merge_same_role_disabled_keeps_separate() {
        let messages = vec![
            Message::new(Role::User, vec![ContentBlock::Text { text: "a".into() }]),
            Message::new(Role::User, vec![ContentBlock::Text { text: "b".into() }]),
        ];
        let compat = ProviderCompat {
            messages: MessageCompat {
                merge_same_role: Some(false),
                ..Default::default()
            },
            ..Default::default()
        };
        let result = build_messages(&messages, &compat);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_auto_tool_id_generates_id_when_empty() {
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: String::new(),
                name: "bash".into(),
                input: json!({}),
                extra: None,
            }],
        )];
        let compat = ProviderCompat {
            tools: ToolCompat {
                auto_tool_id: Some(true),
                ..Default::default()
            },
            ..Default::default()
        };
        let result = build_messages(&messages, &compat);
        let content = result[0]["content"].as_array().unwrap();
        let id = content[0]["id"].as_str().unwrap();
        assert!(id.starts_with("toolu_"));
    }

    #[test]
    fn test_auto_tool_id_preserves_existing_id() {
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "existing_id".into(),
                name: "bash".into(),
                input: json!({}),
                extra: None,
            }],
        )];
        let compat = ProviderCompat {
            tools: ToolCompat {
                auto_tool_id: Some(true),
                ..Default::default()
            },
            ..Default::default()
        };
        let result = build_messages(&messages, &compat);
        let content = result[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["id"], "existing_id");
    }

    // F1-2
    #[test]
    fn test_anthropic_empty_name_downgraded_no_orphan() {
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
        let result = build_messages(&messages, &anthropic_compat());
        let any_empty = result
            .iter()
            .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
            .any(|b| b["type"] == "tool_use" && b["name"] == "");
        assert!(!any_empty);
        let any_orphan = result
            .iter()
            .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
            .any(|b| b["type"] == "tool_result" && b["tool_use_id"] == "call_x");
        assert!(!any_orphan);
        let any_text = result
            .iter()
            .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
            .any(|b| b["type"] == "text" && b["text"].as_str().unwrap_or("").contains("[tool call skipped:"));
        assert!(any_text);
    }

    // F1-4
    #[test]
    fn test_anthropic_only_empty_name_yields_text_content() {
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "call_x".into(),
                name: "".into(),
                input: json!({}),
                extra: None,
            }],
        )];
        let result = build_messages(&messages, &anthropic_compat());
        let content = result.iter().find(|m| m["role"] == "assistant").unwrap()["content"]
            .as_array()
            .unwrap();
        assert!(content.iter().any(|b| {
            b["type"] == "text"
                && b["text"].as_str().unwrap_or("").contains("[tool call skipped:")
                && b["text"].as_str().unwrap_or("").contains("arguments={}")
        }));
        assert!(!content.iter().any(|b| b["type"] == "tool_use" && b["name"] == ""));
    }

    #[test]
    fn test_anthropic_downgrade_text_not_stripped() {
        let mut compat = anthropic_compat();
        compat.messages.strip_patterns = Some(vec![
            "REMOVE_ME".into(),
            "[tool call skipped:".into(),
            "arguments={}".into(),
        ]);
        let messages = vec![Message::new(
            Role::Assistant,
            vec![
                ContentBlock::Text {
                    text: "ordinary REMOVE_ME text".into(),
                },
                ContentBlock::ToolUse {
                    id: "call_x".into(),
                    name: "".into(),
                    input: json!({}),
                    extra: None,
                },
            ],
        )];

        let result = build_messages(&messages, &compat);
        let content = result.iter().find(|m| m["role"] == "assistant").unwrap()["content"]
            .as_array()
            .unwrap();
        assert!(
            content
                .iter()
                .any(|b| { b["type"] == "text" && b["text"].as_str().unwrap_or("") == "ordinary  text" })
        );
        assert!(content.iter().any(|b| {
            b["type"] == "text"
                && b["text"].as_str().unwrap_or("").contains("[tool call skipped:")
                && b["text"].as_str().unwrap_or("").contains("arguments={}")
        }));
    }

    #[test]
    fn test_anthropic_sanitize_disabled_keeps_empty_name() {
        let mut compat = anthropic_compat();
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

        let result = build_messages(&messages, &compat);
        let any_empty = result
            .iter()
            .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
            .any(|b| b["type"] == "tool_use" && b["name"] == "");
        assert!(any_empty);
    }

    #[test]
    fn test_anthropic_reverse_orphan_tool_result_dropped() {
        let messages = vec![Message::new(
            Role::Tool,
            vec![ContentBlock::ToolResult {
                tool_use_id: "missing".into(),
                content: "orphan".into(),
                is_error: true,
            }],
        )];

        let result = build_messages(&messages, &anthropic_compat());
        let any_tool_result = result
            .iter()
            .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
            .any(|b| b["type"] == "tool_result" && b["tool_use_id"] == "missing");
        assert!(!any_tool_result);
        assert!(result.iter().any(|m| {
            m["role"] == "user"
                && m["content"].as_array().is_some_and(|blocks| {
                    blocks.iter().any(|b| {
                        b["type"] == "text"
                            && b["text"].as_str().unwrap_or("")
                                == "[tool call skipped: malformed (orphan tool result).]"
                    })
                })
        }));
    }

    #[test]
    fn test_anthropic_reverse_orphan_matched_result_not_dropped() {
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

        let result = build_messages(&messages, &anthropic_compat());
        let any_tool_result = result
            .iter()
            .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
            .any(|b| b["type"] == "tool_result" && b["tool_use_id"] == "call_x");
        assert!(any_tool_result);
    }

    #[test]
    fn test_anthropic_empty_id_toolcall_downgraded_when_auto_id_disabled() {
        let mut compat = anthropic_compat();
        compat.tools.auto_tool_id = Some(false);
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "".into(),
                name: "Bash".into(),
                input: json!({"command":"ls"}),
                extra: None,
            }],
        )];

        let result = build_messages(&messages, &compat);
        let blocks: Vec<_> = result
            .iter()
            .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
            .collect();
        assert!(!blocks.iter().any(|b| b["type"] == "tool_use"));
        assert!(!blocks.iter().any(|b| b["type"] == "tool_result"));
        assert!(blocks.iter().any(|b| {
            b["type"] == "text"
                && b["text"].as_str().unwrap_or("").contains("empty tool call id")
                && b["text"]
                    .as_str()
                    .unwrap_or("")
                    .contains("arguments={\"command\":\"ls\"}")
        }));
    }

    #[test]
    fn test_anthropic_empty_id_toolcall_generates_id_when_auto_id_enabled() {
        let mut compat = anthropic_compat();
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
                    content: "orphan".into(),
                    is_error: true,
                }],
            ),
        ];

        let result = build_messages(&messages, &compat);
        let blocks: Vec<_> = result
            .iter()
            .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
            .collect();
        let tool_use = blocks.iter().find(|b| b["type"] == "tool_use").unwrap();
        assert_eq!(tool_use["name"], "Bash");
        assert!(tool_use["id"].as_str().unwrap().starts_with("toolu_"));
        assert_ne!(tool_use["id"], "");
    }

    #[test]
    fn test_anthropic_empty_id_rewrites_paired_result_when_auto_id_enabled() {
        let mut compat = anthropic_compat();
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

        let result = build_messages(&messages, &compat);
        let blocks: Vec<_> = result
            .iter()
            .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
            .collect();
        let tool_use = blocks.iter().find(|b| b["type"] == "tool_use").unwrap();
        let generated_id = tool_use["id"].as_str().unwrap();
        assert!(generated_id.starts_with("toolu_"));
        let tool_result = blocks.iter().find(|b| b["type"] == "tool_result").unwrap();
        assert_eq!(tool_result["tool_use_id"], generated_id);
        assert_eq!(tool_result["content"], "ok");
    }

    #[test]
    fn test_anthropic_result_before_matching_call_is_dropped() {
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

        let result = build_messages(&messages, &anthropic_compat());
        let blocks: Vec<_> = result
            .iter()
            .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
            .collect();
        assert!(
            !blocks
                .iter()
                .any(|b| b["type"] == "tool_result" && b["tool_use_id"] == "late")
        );
    }

    #[test]
    fn test_anthropic_dropped_empty_id_does_not_consume_later_generated_empty_id_result() {
        let mut compat = anthropic_compat();
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

        let result = build_messages(&messages, &compat);
        let blocks: Vec<_> = result
            .iter()
            .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
            .collect();
        let generated_id = blocks
            .iter()
            .find(|b| b["type"] == "tool_use" && b["name"] == "Bash")
            .and_then(|b| b["id"].as_str())
            .unwrap();
        let tool_results: Vec<_> = blocks.iter().filter(|b| b["type"] == "tool_result").collect();
        assert_eq!(tool_results.len(), 1);
        assert_eq!(tool_results[0]["tool_use_id"], generated_id);
        assert_eq!(tool_results[0]["content"], "ok");
    }

    #[test]
    fn test_anthropic_dropped_tool_result_yields_placeholder_content() {
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
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "call_x".into(),
                    content: "Unknown tool: ".into(),
                    is_error: true,
                }],
            ),
        ];

        let result = build_messages(&messages, &anthropic_compat());
        let user_placeholder = result.iter().any(|m| {
            m["role"] == "user"
                && m["content"].as_array().is_some_and(|blocks| {
                    blocks.iter().any(|b| {
                        b["type"] == "text"
                            && b["text"].as_str().unwrap_or("")
                                == "[tool call skipped: malformed (empty function name).]"
                    })
                })
        });
        assert!(user_placeholder);
        let any_tool_result = result
            .iter()
            .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
            .any(|b| b["type"] == "tool_result" && b["tool_use_id"] == "call_x");
        assert!(!any_tool_result);
    }

    // H1-2
    #[test]
    fn test_anthropic_normal_toolcall_unaffected() {
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "call_x".into(),
                name: "Bash".into(),
                input: json!({"command":"ls"}),
                extra: None,
            }],
        )];
        let result = build_messages(&messages, &anthropic_compat());
        let any_bash = result
            .iter()
            .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
            .any(|b| b["type"] == "tool_use" && b["name"] == "Bash");
        assert!(any_bash);
    }

    // --- build_tools tests ---

    #[test]
    fn test_build_tools_single() {
        // arrange
        let schema = json!({
            "type": "object",
            "properties": {
                "cmd": { "type": "string" }
            },
            "required": ["cmd"]
        });
        let tools = vec![ToolDef {
            name: "bash".to_string(),
            description: "Run a shell command".to_string(),
            input_schema: schema.clone(),
            deferred: false,
        }];
        // act
        let result = build_tools(&tools);
        // assert
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["name"], "bash");
        assert_eq!(result[0]["description"], "Run a shell command");
        assert_eq!(result[0]["input_schema"], legalize_json_schema(&schema));
    }

    #[test]
    fn test_build_tools_empty() {
        // arrange
        let tools: Vec<ToolDef> = vec![];
        // act
        let result = build_tools(&tools);
        // assert
        assert!(result.is_empty());
    }

    #[test]
    fn test_build_tools_deferred_has_empty_schema() {
        let tools = vec![
            ToolDef {
                name: "Read".into(),
                description: "Read a file".into(),
                input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
                deferred: false,
            },
            ToolDef {
                name: "SpawnTool".into(),
                description: "Spawn sub-agents".into(),
                input_schema: json!({"type": "object", "properties": {"agents": {"type": "array"}}}),
                deferred: true,
            },
        ];
        let result = build_tools(&tools);

        // Core tool has full input_schema
        assert!(result[0]["input_schema"]["properties"].get("path").is_some());

        // Deferred tool has empty input_schema and modified description
        assert!(result[1]["input_schema"]["properties"].as_object().unwrap().is_empty());
        let desc = result[1]["description"].as_str().unwrap();
        assert!(desc.contains("ToolSearch"));
    }

    #[test]
    fn test_build_tools_legalizes_null_and_missing_type_schema() {
        let tools = vec![
            ToolDef {
                name: "NullSchema".into(),
                description: "Null schema".into(),
                input_schema: Value::Null,
                deferred: false,
            },
            ToolDef {
                name: "MissingType".into(),
                description: "Missing root type".into(),
                input_schema: json!({
                    "properties": {
                        "path": { "type": "string" }
                    }
                }),
                deferred: false,
            },
            ToolDef {
                name: "ArraySchema".into(),
                description: "Array schema".into(),
                input_schema: json!(["not", "object"]),
                deferred: false,
            },
            ToolDef {
                name: "StringRootType".into(),
                description: "String root type".into(),
                input_schema: json!({"type": "string"}),
                deferred: false,
            },
        ];
        let result = build_tools(&tools);

        assert_eq!(
            result[0]["input_schema"],
            json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {}
            })
        );
        assert_eq!(result[1]["input_schema"]["type"], "object");
        assert_eq!(
            result[1]["input_schema"]["$schema"],
            "https://json-schema.org/draft/2020-12/schema"
        );
        assert!(result[1]["input_schema"]["properties"]["path"].is_object());
        for tool in result.iter().skip(2) {
            assert_eq!(
                tool["input_schema"],
                json!({
                    "$schema": "https://json-schema.org/draft/2020-12/schema",
                    "type": "object",
                    "properties": {}
                })
            );
        }
    }

    // --- parse_sse_data tests ---

    #[test]
    fn test_parse_anthropic_event_text_delta() {
        // arrange
        let mut state = StreamState::new();
        let data = r#"{"delta":{"type":"text_delta","text":"Hello"}}"#;
        // act
        let events = parse_sse_data("content_block_delta", data, &mut state);
        // assert
        assert_eq!(events.len(), 1);
        match &events[0] {
            LlmEvent::TextDelta(t) => assert_eq!(t, "Hello"),
            _ => panic!("expected TextDelta"),
        }
    }

    #[test]
    fn test_parse_anthropic_event_tool_use() {
        // arrange
        let mut state = StreamState::new();
        // step 1: content_block_start with tool_use type
        let start_events = parse_sse_data(
            "content_block_start",
            r#"{"content_block":{"type":"tool_use","id":"id1","name":"bash"}}"#,
            &mut state,
        );
        assert!(start_events.is_empty());
        // step 2: content_block_delta with input_json_delta
        let delta_events = parse_sse_data(
            "content_block_delta",
            r#"{"delta":{"type":"input_json_delta","partial_json":"{\"cmd\":\"ls\"}"}}"#,
            &mut state,
        );
        assert!(delta_events.is_empty());
        // step 3: content_block_stop emits the ToolUse event
        let events = parse_sse_data("content_block_stop", r#"{}"#, &mut state);
        // assert
        assert_eq!(events.len(), 1);
        match &events[0] {
            LlmEvent::ToolUse { id, name, input, .. } => {
                assert_eq!(id, "id1");
                assert_eq!(name, "bash");
                assert_eq!(input["cmd"], "ls");
            }
            _ => panic!("expected ToolUse"),
        }
    }

    // F1-10
    #[test]
    fn test_empty_name_tool_use_still_emitted_to_history() {
        let mut state = StreamState::new();

        let start_events = parse_sse_data(
            "content_block_start",
            r#"{"content_block":{"type":"tool_use","id":"call_x","name":""}}"#,
            &mut state,
        );
        assert!(start_events.is_empty());

        let events = parse_sse_data("content_block_stop", r#"{}"#, &mut state);
        assert_eq!(events.len(), 1);

        match &events[0] {
            LlmEvent::ToolUse { id, name, .. } => {
                assert_eq!(id, "call_x");
                assert_eq!(name, "");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_anthropic_event_stop() {
        // arrange
        let mut state = StreamState::new();
        let data = r#"{"delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":42}}"#;
        // act
        let events = parse_sse_data("message_delta", data, &mut state);
        // assert
        assert_eq!(events.len(), 1);
        match &events[0] {
            LlmEvent::Done { stop_reason, usage } => {
                assert_eq!(*stop_reason, StopReason::EndTurn);
                assert_eq!(usage.output_tokens, 42);
            }
            _ => panic!("expected Done"),
        }
    }

    #[test]
    fn test_parse_anthropic_event_thinking() {
        // arrange
        let mut state = StreamState::new();
        let data = r#"{"delta":{"type":"thinking_delta","thinking":"reasoning step"}}"#;
        // act
        let events = parse_sse_data("content_block_delta", data, &mut state);
        // assert
        assert_eq!(events.len(), 1);
        match &events[0] {
            LlmEvent::ThinkingDelta(t) => assert_eq!(t, "reasoning step"),
            _ => panic!("expected ThinkingDelta"),
        }
    }

    #[test]
    fn test_parse_anthropic_event_thinking_signature() {
        let mut state = StreamState::new();
        let data = r#"{"delta":{"type":"signature_delta","signature":"sig-123"}}"#;

        let events = parse_sse_data("content_block_delta", data, &mut state);

        assert_eq!(events.len(), 1);
        match &events[0] {
            LlmEvent::ThinkingSignature(signature) => assert_eq!(signature, "sig-123"),
            _ => panic!("expected ThinkingSignature"),
        }
    }

    #[test]
    fn test_parse_anthropic_event_unknown_type() {
        // arrange
        let mut state = StreamState::new();
        let data = r#"{}"#;
        // act
        let events = parse_sse_data("unknown_event", data, &mut state);
        // assert
        assert!(events.is_empty());
    }

    // --- Image block projection ---

    #[test]
    fn test_build_messages_image_projects_to_base64_source() {
        let data = STANDARD.encode(b"fake-image");
        let messages = vec![Message::new(
            Role::User,
            vec![
                ContentBlock::Text { text: "look".into() },
                ContentBlock::Image {
                    image_url: ImageUrl {
                        url: format!("data:image/png;base64,{}", data),
                    },
                },
            ],
        )];

        let result = build_messages(&messages, &default_compat());
        let content = result[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["source"]["type"], "base64");
        assert_eq!(content[1]["source"]["media_type"], "image/png");
        assert_eq!(content[1]["source"]["data"], data);
    }

    #[test]
    fn test_build_messages_invalid_image_skipped_not_octet_stream() {
        // `application/octet-stream` is not a valid Anthropic image media type.
        // An invalid data URI must be dropped rather than projected as-is.
        let messages = vec![Message::new(
            Role::User,
            vec![
                ContentBlock::Text { text: "look".into() },
                ContentBlock::Image {
                    image_url: ImageUrl {
                        url: "data:application/octet-stream;base64,abc".to_string(),
                    },
                },
            ],
        )];

        let result = build_messages(&messages, &default_compat());
        let content = result[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1, "invalid image should be skipped");
        assert_eq!(content[0]["type"], "text");
        let any_image = content.iter().any(|b| b["type"] == "image");
        assert!(!any_image, "no image block should be emitted for invalid data URI");
    }
}
