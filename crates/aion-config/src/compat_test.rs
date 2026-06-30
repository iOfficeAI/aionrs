use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_flattened_compat_deserializes_legacy_toml_keys() {
        let toml_str = r#"
max_tokens_field = "max_completion_tokens"
api_path = "/chat/completions"
max_request_body_bytes = 1048576
include_stream_options = false
merge_assistant_messages = true
clean_orphan_tool_results = false
dedup_tool_results = true
clean_orphan_tool_calls = true
sanitize_malformed_tool_calls = false
max_tool_count = 512
emit_tools = false
tool_wire_shape = "openai_function"
ensure_alternation = true
merge_same_role = true
sanitize_schema = true
strip_patterns = ["__REASONING__"]
auto_tool_id = true
supports_thinking = true
supports_effort = false
effort_levels = ["low", "medium"]
"#;

        let compat: ProviderCompat = toml::from_str(toml_str).unwrap();

        assert_eq!(
            compat.transport.max_tokens_field.as_deref(),
            Some("max_completion_tokens")
        );
        assert_eq!(compat.transport.api_path.as_deref(), Some("/chat/completions"));
        assert_eq!(compat.max_request_body_bytes(), Some(1_048_576));
        assert_eq!(compat.transport.include_stream_options, Some(false));
        assert!(!compat.include_stream_options());
        assert_eq!(compat.messages.merge_assistant_messages, Some(true));
        assert_eq!(compat.messages.clean_orphan_tool_results, Some(false));
        assert_eq!(compat.messages.dedup_tool_results, Some(true));
        assert_eq!(compat.messages.ensure_alternation, Some(true));
        assert_eq!(compat.messages.merge_same_role, Some(true));
        assert_eq!(compat.messages.strip_patterns, Some(vec!["__REASONING__".to_string()]));
        assert_eq!(compat.tools.clean_orphan_tool_calls, Some(true));
        assert_eq!(compat.tools.sanitize_malformed_tool_calls, Some(false));
        assert_eq!(compat.tools.auto_tool_id, Some(true));
        assert_eq!(compat.max_tool_count(), Some(512));
        assert_eq!(compat.tools.emit_tools, Some(false));
        assert!(!compat.emit_tools());
        assert_eq!(compat.tool_wire_shape(), ToolWireShape::OpenAiFunction);
        assert_eq!(compat.schema.sanitize_schema, Some(true));
        assert_eq!(compat.reasoning.supports_thinking, Some(true));
        assert_eq!(compat.reasoning.supports_effort, Some(false));
        assert_eq!(
            compat.reasoning.effort_levels,
            Some(vec!["low".to_string(), "medium".to_string()])
        );
    }

    #[test]
    fn test_flattened_compat_serializes_to_legacy_toml_keys() {
        let compat = ProviderCompat {
            transport: TransportCompat {
                max_tokens_field: Some("max_completion_tokens".to_string()),
                api_path: Some("/chat/completions".to_string()),
                max_request_body_bytes: Some(1_048_576),
                include_stream_options: Some(false),
            },
            messages: MessageCompat {
                merge_assistant_messages: Some(true),
                clean_orphan_tool_results: Some(false),
                dedup_tool_results: Some(true),
                ensure_alternation: Some(true),
                merge_same_role: Some(true),
                strip_patterns: Some(vec!["__REASONING__".to_string()]),
            },
            tools: ToolCompat {
                clean_orphan_tool_calls: Some(true),
                sanitize_malformed_tool_calls: Some(false),
                auto_tool_id: Some(true),
                max_tool_count: Some(512),
                emit_tools: Some(false),
                tool_wire_shape: Some(ToolWireShape::OpenAiFunction),
            },
            schema: SchemaCompat {
                sanitize_schema: Some(true),
            },
            reasoning: ReasoningCompat {
                supports_thinking: Some(true),
                supports_effort: Some(false),
                effort_levels: Some(vec!["low".to_string(), "medium".to_string()]),
            },
        };

        let toml = toml::to_string(&compat).unwrap();

        assert!(toml.contains("max_tokens_field = \"max_completion_tokens\""));
        assert!(toml.contains("api_path = \"/chat/completions\""));
        assert!(toml.contains("max_request_body_bytes = 1048576"));
        assert!(toml.contains("include_stream_options = false"));
        assert!(toml.contains("merge_assistant_messages = true"));
        assert!(toml.contains("clean_orphan_tool_results = false"));
        assert!(toml.contains("dedup_tool_results = true"));
        assert!(toml.contains("clean_orphan_tool_calls = true"));
        assert!(toml.contains("sanitize_malformed_tool_calls = false"));
        assert!(toml.contains("max_tool_count = 512"));
        assert!(toml.contains("emit_tools = false"));
        assert!(toml.contains("tool_wire_shape = \"openai_function\""));
        assert!(toml.contains("ensure_alternation = true"));
        assert!(toml.contains("merge_same_role = true"));
        assert!(toml.contains("sanitize_schema = true"));
        assert!(toml.contains("strip_patterns = [\"__REASONING__\"]"));
        assert!(toml.contains("auto_tool_id = true"));
        assert!(toml.contains("supports_thinking = true"));
        assert!(toml.contains("supports_effort = false"));
        assert!(toml.contains("effort_levels = [\"low\", \"medium\"]"));
        assert!(!toml.contains("[transport]"));
        assert!(!toml.contains("[messages]"));
        assert!(!toml.contains("[tools]"));
        assert!(!toml.contains("[schema]"));
        assert!(!toml.contains("[reasoning]"));
    }

    #[test]
    fn test_domain_merge_preserves_user_overrides_and_defaults() {
        let defaults = ProviderCompat::openai_defaults();
        let user = ProviderCompat {
            transport: TransportCompat {
                max_tokens_field: Some("max_completion_tokens".to_string()),
                api_path: Some("/chat/completions".to_string()),
                max_request_body_bytes: Some(2_048),
                include_stream_options: None,
            },
            messages: MessageCompat {
                merge_assistant_messages: Some(false),
                clean_orphan_tool_results: Some(false),
                dedup_tool_results: None,
                ensure_alternation: None,
                merge_same_role: None,
                strip_patterns: Some(vec!["strip-me".to_string()]),
            },
            tools: ToolCompat {
                clean_orphan_tool_calls: Some(false),
                sanitize_malformed_tool_calls: Some(false),
                auto_tool_id: Some(false),
                max_tool_count: Some(42),
                emit_tools: None,
                tool_wire_shape: None,
            },
            schema: SchemaCompat {
                sanitize_schema: Some(true),
            },
            reasoning: ReasoningCompat {
                supports_thinking: Some(true),
                supports_effort: None,
                effort_levels: Some(vec!["custom".to_string()]),
            },
        };

        let merged = ProviderCompat::merge(defaults, user);

        assert_eq!(
            merged.transport.max_tokens_field.as_deref(),
            Some("max_completion_tokens")
        );
        assert_eq!(merged.transport.api_path.as_deref(), Some("/chat/completions"));
        assert_eq!(merged.max_request_body_bytes(), Some(2_048));
        assert!(merged.include_stream_options());
        assert!(!merged.merge_assistant_messages());
        assert!(!merged.clean_orphan_tool_calls());
        assert!(!merged.clean_orphan_tool_results());
        assert!(merged.dedup_tool_results());
        assert!(!merged.sanitize_malformed_tool_calls());
        assert_eq!(merged.max_tool_count(), Some(42));
        assert!(merged.emit_tools());
        assert_eq!(merged.tool_wire_shape(), ToolWireShape::Native);
        assert!(merged.sanitize_schema());
        assert!(!merged.auto_tool_id());
        assert!(merged.supports_thinking());
        assert!(merged.supports_effort());
        assert_eq!(merged.effort_levels(), &["custom"]);
        assert_eq!(merged.messages.strip_patterns, Some(vec!["strip-me".to_string()]));
        assert_eq!(merged.reasoning.supports_thinking, Some(true));
        assert_eq!(merged.reasoning.supports_effort, Some(true));
        assert_eq!(merged.reasoning.effort_levels, Some(vec!["custom".to_string()]));
    }

    #[test]
    fn test_domain_merge_empty_user_keeps_all_defaults() {
        let merged = ProviderCompat::merge(ProviderCompat::anthropic_defaults(), ProviderCompat::default());

        assert!(merged.ensure_alternation());
        assert!(merged.merge_same_role());
        assert!(merged.auto_tool_id());
        assert!(merged.sanitize_malformed_tool_calls());
        assert!(merged.clean_orphan_tool_results());
        assert!(merged.supports_thinking());
        assert!(!merged.supports_effort());
        assert!(merged.effort_levels().is_empty());
    }

    #[test]
    fn test_anthropic_defaults() {
        let compat = ProviderCompat::anthropic_defaults();
        assert!(compat.ensure_alternation());
        assert!(compat.merge_same_role());
        assert!(compat.auto_tool_id());
        assert!(!compat.sanitize_schema());
        assert!(!compat.merge_assistant_messages());
        assert!(!compat.clean_orphan_tool_calls());
    }

    #[test]
    fn test_bedrock_defaults() {
        let compat = ProviderCompat::bedrock_defaults();
        assert!(compat.ensure_alternation());
        assert!(compat.merge_same_role());
        assert!(compat.auto_tool_id());
        assert!(compat.sanitize_schema());
    }

    #[test]
    fn test_openai_defaults() {
        let compat = ProviderCompat::openai_defaults();
        assert!(compat.merge_assistant_messages());
        assert!(compat.clean_orphan_tool_calls());
        assert!(compat.clean_orphan_tool_results());
        assert!(compat.dedup_tool_results());
        assert_eq!(compat.transport.max_tokens_field.as_deref(), Some("max_tokens"));
        assert!(!compat.ensure_alternation());
        assert_eq!(compat.transport.include_stream_options, Some(true));
        assert_eq!(compat.tools.emit_tools, Some(true));
        assert!(compat.include_stream_options());
        assert!(compat.emit_tools());
    }

    #[test]
    fn test_openai_field_controls_parse_flattened_compat() {
        let toml_str = r#"
include_stream_options = false
emit_tools = false
supports_effort = false
"#;

        let compat: ProviderCompat = toml::from_str(toml_str).unwrap();

        assert_eq!(compat.transport.include_stream_options, Some(false));
        assert_eq!(compat.tools.emit_tools, Some(false));
        assert_eq!(compat.reasoning.supports_effort, Some(false));
        assert!(!compat.include_stream_options());
        assert!(!compat.emit_tools());
        assert!(!compat.supports_effort());
    }

    #[test]
    fn test_openai_field_controls_merge_user_overrides_defaults() {
        let defaults = ProviderCompat::openai_defaults();
        let user = ProviderCompat {
            transport: TransportCompat {
                include_stream_options: Some(false),
                ..Default::default()
            },
            tools: ToolCompat {
                emit_tools: Some(false),
                ..Default::default()
            },
            reasoning: ReasoningCompat {
                supports_effort: Some(false),
                ..Default::default()
            },
            ..Default::default()
        };

        let merged = ProviderCompat::merge(defaults, user);

        assert_eq!(merged.transport.include_stream_options, Some(false));
        assert_eq!(merged.tools.emit_tools, Some(false));
        assert_eq!(merged.reasoning.supports_effort, Some(false));
        assert!(!merged.include_stream_options());
        assert!(!merged.emit_tools());
        assert!(!merged.supports_effort());
    }

    #[test]
    fn test_tool_wire_shape_toml_round_trips_supported_values() {
        for value in ["native", "openai_function", "anthropic_input_schema"] {
            let compat: ProviderCompat = toml::from_str(&format!("tool_wire_shape = \"{value}\"")).unwrap();

            let toml = toml::to_string(&compat).unwrap();

            assert!(toml.contains(&format!("tool_wire_shape = \"{value}\"")));
        }
    }

    #[test]
    fn test_tool_wire_shape_toml_rejects_unknown_value() {
        let result = toml::from_str::<ProviderCompat>(
            r#"
tool_wire_shape = "provider_guess"
"#,
        );

        assert!(result.is_err());
    }

    #[test]
    fn test_clean_orphan_tool_results_defaults() {
        assert_eq!(
            ProviderCompat::openai_defaults().messages.clean_orphan_tool_results,
            Some(true)
        );
        assert_eq!(
            ProviderCompat::anthropic_defaults().messages.clean_orphan_tool_results,
            Some(true)
        );
        assert_eq!(
            ProviderCompat::bedrock_defaults().messages.clean_orphan_tool_results,
            Some(true)
        );
    }

    #[test]
    fn test_clean_orphan_tool_results_accessor_defaults_false() {
        let compat = ProviderCompat::default();
        assert!(!compat.clean_orphan_tool_results());
    }

    #[test]
    fn test_merge_user_overrides_defaults() {
        let defaults = ProviderCompat::openai_defaults();
        let user = ProviderCompat {
            transport: TransportCompat {
                max_tokens_field: Some("max_completion_tokens".into()),
                ..Default::default()
            },
            messages: MessageCompat {
                merge_assistant_messages: Some(false),
                ..Default::default()
            },
            ..Default::default()
        };

        let merged = ProviderCompat::merge(defaults, user);
        assert_eq!(
            merged.transport.max_tokens_field.as_deref(),
            Some("max_completion_tokens")
        );
        assert!(!merged.merge_assistant_messages());
        // Non-overridden fields keep defaults
        assert!(merged.clean_orphan_tool_calls());
        assert!(merged.clean_orphan_tool_results());
        assert!(merged.dedup_tool_results());
    }

    #[test]
    fn test_merge_clean_orphan_tool_results_user_overrides() {
        let defaults = ProviderCompat::openai_defaults();
        let user = ProviderCompat {
            messages: MessageCompat {
                clean_orphan_tool_results: Some(false),
                ..Default::default()
            },
            ..Default::default()
        };

        let merged = ProviderCompat::merge(defaults, user);
        assert_eq!(merged.messages.clean_orphan_tool_results, Some(false));
        assert!(!merged.clean_orphan_tool_results());
    }

    #[test]
    fn test_merge_empty_user_keeps_defaults() {
        let defaults = ProviderCompat::anthropic_defaults();
        let user = ProviderCompat::default();

        let merged = ProviderCompat::merge(defaults, user);
        assert!(merged.ensure_alternation());
        assert!(merged.merge_same_role());
        assert!(merged.auto_tool_id());
    }

    #[test]
    fn test_sanitize_schema_wraps_non_object_root() {
        let schema = json!({"type": "string"});
        let sanitized = sanitize_json_schema(&schema);

        assert_eq!(sanitized["type"], "object");
        assert_eq!(sanitized["properties"]["value"]["type"], "string");
    }

    #[test]
    fn test_sanitize_schema_removes_additional_properties() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "additionalProperties": false}
            },
            "additionalProperties": false
        });
        let sanitized = sanitize_json_schema(&schema);

        assert!(sanitized.get("additionalProperties").is_none());
        assert!(sanitized["properties"]["name"].get("additionalProperties").is_none());
    }

    #[test]
    fn test_sanitize_schema_normalizes_array_types() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": {"type": ["string", "null"]}
            }
        });
        let sanitized = sanitize_json_schema(&schema);

        assert_eq!(sanitized["properties"]["name"]["type"], "string");
    }

    #[test]
    fn test_sanitize_schema_no_change_for_valid_object() {
        let schema = json!({
            "type": "object",
            "properties": {
                "cmd": {"type": "string"}
            },
            "required": ["cmd"]
        });
        let sanitized = sanitize_json_schema(&schema);

        assert_eq!(sanitized["type"], "object");
        assert_eq!(sanitized["properties"]["cmd"]["type"], "string");
    }

    #[test]
    fn test_anthropic_defaults_capability_fields() {
        let compat = ProviderCompat::anthropic_defaults();
        assert_eq!(compat.reasoning.supports_thinking, Some(true));
        assert_eq!(compat.reasoning.supports_effort, Some(false));
        assert!(compat.reasoning.effort_levels.is_none());
    }

    #[test]
    fn test_openai_defaults_capability_fields() {
        let compat = ProviderCompat::openai_defaults();
        assert_eq!(compat.reasoning.supports_thinking, Some(false));
        assert_eq!(compat.reasoning.supports_effort, Some(true));
        assert_eq!(
            compat.reasoning.effort_levels,
            Some(vec!["low".to_string(), "medium".to_string(), "high".to_string()])
        );
    }

    #[test]
    fn test_bedrock_defaults_capability_fields() {
        let compat = ProviderCompat::bedrock_defaults();
        assert_eq!(compat.reasoning.supports_thinking, Some(true));
        assert_eq!(compat.reasoning.supports_effort, Some(false));
    }

    #[test]
    fn test_merge_capability_fields_user_overrides() {
        let defaults = ProviderCompat::openai_defaults();
        let user = ProviderCompat {
            reasoning: ReasoningCompat {
                supports_thinking: Some(true),
                ..Default::default()
            },
            ..Default::default()
        };
        let merged = ProviderCompat::merge(defaults, user);
        assert_eq!(merged.reasoning.supports_thinking, Some(true));
        assert_eq!(merged.reasoning.supports_effort, Some(true));
    }

    #[test]
    fn test_capability_accessors() {
        let compat = ProviderCompat::anthropic_defaults();
        assert!(compat.supports_thinking());
        assert!(!compat.supports_effort());
        assert!(compat.effort_levels().is_empty());

        let compat2 = ProviderCompat::openai_defaults();
        assert!(!compat2.supports_thinking());
        assert!(compat2.supports_effort());
        assert_eq!(compat2.effort_levels(), &["low", "medium", "high"]);
    }

    // D-1
    #[test]
    fn test_defaults_enable_sanitize_malformed_tool_calls() {
        assert_eq!(
            ProviderCompat::openai_defaults().tools.sanitize_malformed_tool_calls,
            Some(true)
        );
        assert_eq!(
            ProviderCompat::anthropic_defaults().tools.sanitize_malformed_tool_calls,
            Some(true)
        );
        assert_eq!(
            ProviderCompat::bedrock_defaults().tools.sanitize_malformed_tool_calls,
            Some(true)
        );
    }

    // D-2
    #[test]
    fn test_merge_preserves_sanitize_field() {
        let defaults = ProviderCompat::openai_defaults();
        let user = ProviderCompat {
            tools: ToolCompat {
                sanitize_malformed_tool_calls: Some(false),
                ..Default::default()
            },
            ..Default::default()
        };
        let merged = ProviderCompat::merge(defaults, user);
        assert_eq!(merged.tools.sanitize_malformed_tool_calls, Some(false)); // user wins
        assert_eq!(merged.tools.clean_orphan_tool_calls, Some(true)); // default preserved
    }

    #[test]
    fn test_deserialize_from_toml() {
        let toml_str = r#"
max_tokens_field = "max_completion_tokens"
merge_assistant_messages = true
clean_orphan_tool_results = false
strip_patterns = ["__REASONING__"]
"#;
        let compat: ProviderCompat = toml::from_str(toml_str).unwrap();
        assert_eq!(
            compat.transport.max_tokens_field.as_deref(),
            Some("max_completion_tokens")
        );
        assert_eq!(compat.messages.merge_assistant_messages, Some(true));
        assert_eq!(compat.messages.clean_orphan_tool_results, Some(false));
        assert_eq!(compat.messages.strip_patterns, Some(vec!["__REASONING__".to_string()]));
        assert!(compat.tools.clean_orphan_tool_calls.is_none());
    }
}
