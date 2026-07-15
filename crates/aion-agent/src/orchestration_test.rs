use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -- truncate_display -----------------------------------------------------

    #[test]
    fn truncate_display_ascii_short_unchanged() {
        assert_eq!(truncate_display("hello", 10), "hello");
    }

    #[test]
    fn truncate_display_ascii_truncated() {
        let result = truncate_display("hello world", 5);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 20);
    }

    #[test]
    fn truncate_display_cjk_does_not_panic() {
        // 200 CJK chars: each is 3 bytes, so byte index 200 falls mid-character
        let cjk: String = "你好世界测试".chars().cycle().take(200).collect();
        let result = truncate_display(&cjk, 50);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn truncate_display_mixed_cjk_ascii_does_not_panic() {
        let mixed = "abc你好def世界ghi测试".repeat(20);
        let result = truncate_display(&mixed, 30);
        assert!(result.ends_with("..."));
    }

    // -- truncate_result ------------------------------------------------------

    #[test]
    fn truncate_result_short_unchanged() {
        let s = "short content";
        assert_eq!(truncate_result(s, 1000), s);
    }

    #[test]
    fn truncate_result_cjk_does_not_panic() {
        let cjk: String = "这是一段较长的中文内容用于测试截断功能".repeat(50);
        let result = truncate_result(&cjk, 100);
        assert!(result.contains("truncated"));
    }

    #[test]
    fn truncate_result_mixed_cjk_ascii_does_not_panic() {
        let mixed = "Hello你好World世界Test测试".repeat(100);
        let result = truncate_result(&mixed, 200);
        assert!(result.contains("truncated"));
    }

    // -- maybe_append_deferred_hint -------------------------------------------

    #[test]
    fn deferred_hint_appended_when_required_field_missing() {
        let schema = json!({
            "type": "object",
            "properties": { "tasks": { "type": "array" } },
            "required": ["tasks"]
        });
        let input = json!({});
        let result = maybe_append_deferred_hint("Missing or invalid 'tasks' array", schema, &input);
        assert!(result.contains("Missing or invalid 'tasks' array"));
        assert!(result.contains("ToolSearch"));
    }

    #[test]
    fn deferred_hint_not_appended_when_required_fields_present() {
        let schema = json!({
            "type": "object",
            "properties": { "tasks": { "type": "array" } },
            "required": ["tasks"]
        });
        let input = json!({"tasks": [{"name": "t1", "prompt": "do x"}]});
        let result = maybe_append_deferred_hint("Some runtime error", schema, &input);
        assert_eq!(result, "Some runtime error");
        assert!(!result.contains("ToolSearch"));
    }

    #[test]
    fn deferred_hint_not_appended_when_no_required_field() {
        let schema = json!({
            "type": "object",
            "properties": {}
        });
        let input = json!({});
        let result = maybe_append_deferred_hint("some error", schema, &input);
        assert_eq!(result, "some error");
    }

    #[test]
    fn deferred_hint_not_appended_when_required_is_empty() {
        let schema = json!({
            "type": "object",
            "properties": {},
            "required": []
        });
        let input = json!({});
        let result = maybe_append_deferred_hint("some error", schema, &input);
        assert_eq!(result, "some error");
    }

    #[test]
    fn deferred_hint_appended_for_partial_missing_fields() {
        let schema = json!({
            "type": "object",
            "properties": {
                "a": { "type": "string" },
                "b": { "type": "string" }
            },
            "required": ["a", "b"]
        });
        let input = json!({"a": "present"});
        let result = maybe_append_deferred_hint("validation failed", schema, &input);
        assert!(result.contains("ToolSearch"));
    }

    // -- execute_single integration tests (deferred tool hint) ----------------

    use aion_tools::Tool;
    use aion_tools::registry::ToolRegistry;

    struct MockDeferredTool {
        schema: serde_json::Value,
    }

    #[async_trait::async_trait]
    impl Tool for MockDeferredTool {
        fn name(&self) -> &str {
            "MockDeferred"
        }
        fn description(&self) -> &str {
            "A mock deferred tool for testing"
        }
        fn input_schema(&self) -> serde_json::Value {
            self.schema.clone()
        }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }
        fn is_deferred(&self) -> bool {
            true
        }
        async fn execute(&self, input: serde_json::Value) -> aion_types::tool::ToolResult {
            if input.get("tasks").is_none() {
                return aion_types::tool::ToolResult {
                    content: "Missing or invalid 'tasks' array".to_string(),
                    is_error: true,
                };
            }
            aion_types::tool::ToolResult {
                content: "ok".to_string(),
                is_error: false,
            }
        }
        fn category(&self) -> aion_protocol::events::ToolCategory {
            aion_protocol::events::ToolCategory::Exec
        }
    }

    struct MockNonDeferredTool;

    #[async_trait::async_trait]
    impl Tool for MockNonDeferredTool {
        fn name(&self) -> &str {
            "MockNonDeferred"
        }
        fn description(&self) -> &str {
            "A mock non-deferred tool"
        }
        fn input_schema(&self) -> serde_json::Value {
            json!({
                "type": "object",
                "properties": { "cmd": { "type": "string" } },
                "required": ["cmd"]
            })
        }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }
        async fn execute(&self, input: serde_json::Value) -> aion_types::tool::ToolResult {
            if input.get("cmd").is_none() {
                return aion_types::tool::ToolResult {
                    content: "Missing cmd".to_string(),
                    is_error: true,
                };
            }
            aion_types::tool::ToolResult {
                content: "ok".to_string(),
                is_error: false,
            }
        }
        fn category(&self) -> aion_protocol::events::ToolCategory {
            aion_protocol::events::ToolCategory::Exec
        }
    }

    fn make_registry_with_deferred() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(MockDeferredTool {
            schema: json!({
                "type": "object",
                "properties": { "tasks": { "type": "array" } },
                "required": ["tasks"]
            }),
        }));
        registry.register(Box::new(MockNonDeferredTool));
        registry
    }

    #[tokio::test]
    async fn execute_single_deferred_tool_error_missing_required_appends_hint() {
        let registry = make_registry_with_deferred();
        let call = ContentBlock::ToolUse {
            id: "call_1".into(),
            name: "MockDeferred".into(),
            input: json!({}),
            extra: None,
        };
        let (result, _, follow_up_blocks) =
            execute_single(&registry, &call, None, aion_compact::CompactLevel::Off, false).await;
        assert!(follow_up_blocks.is_empty());
        if let ContentBlock::ToolResult { content, is_error, .. } = &result {
            assert!(is_error);
            assert!(content.contains("Missing or invalid 'tasks' array"));
            assert!(content.contains("ToolSearch"));
        } else {
            panic!("expected ToolResult");
        }
    }

    #[tokio::test]
    async fn execute_single_deferred_tool_error_with_required_present_no_hint() {
        let registry = make_registry_with_deferred();
        // tasks is present but wrong type — tool still fails, but required field exists
        let call = ContentBlock::ToolUse {
            id: "call_2".into(),
            name: "MockDeferred".into(),
            input: json!({"tasks": "not_an_array"}),
            extra: None,
        };
        let (result, _, follow_up_blocks) =
            execute_single(&registry, &call, None, aion_compact::CompactLevel::Off, false).await;
        assert!(follow_up_blocks.is_empty());
        if let ContentBlock::ToolResult { content, is_error, .. } = &result {
            // Tool succeeds because input.get("tasks") is Some
            assert!(!is_error);
            assert!(!content.contains("ToolSearch"));
        } else {
            panic!("expected ToolResult");
        }
    }

    #[tokio::test]
    async fn execute_single_deferred_tool_success_no_hint() {
        let registry = make_registry_with_deferred();
        let call = ContentBlock::ToolUse {
            id: "call_3".into(),
            name: "MockDeferred".into(),
            input: json!({"tasks": [{"name": "t1", "prompt": "do x"}]}),
            extra: None,
        };
        let (result, _, follow_up_blocks) =
            execute_single(&registry, &call, None, aion_compact::CompactLevel::Off, false).await;
        assert!(follow_up_blocks.is_empty());
        if let ContentBlock::ToolResult { content, is_error, .. } = &result {
            assert!(!is_error);
            assert_eq!(content, "ok");
        } else {
            panic!("expected ToolResult");
        }
    }

    #[tokio::test]
    async fn execute_single_non_deferred_tool_error_no_hint() {
        let registry = make_registry_with_deferred();
        let call = ContentBlock::ToolUse {
            id: "call_4".into(),
            name: "MockNonDeferred".into(),
            input: json!({}),
            extra: None,
        };
        let (result, _, follow_up_blocks) =
            execute_single(&registry, &call, None, aion_compact::CompactLevel::Off, false).await;
        assert!(follow_up_blocks.is_empty());
        if let ContentBlock::ToolResult { content, is_error, .. } = &result {
            assert!(is_error);
            assert!(content.contains("Missing cmd"));
            assert!(!content.contains("ToolSearch"));
        } else {
            panic!("expected ToolResult");
        }
    }
}
