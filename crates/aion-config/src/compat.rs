// Configuration-driven provider compatibility layer.
// Each provider type has default presets; users can override any field via config.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Provider-level compatibility settings.
/// Each child struct is flattened so on-disk TOML remains backward-compatible.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ProviderCompat {
    #[serde(flatten)]
    pub transport: TransportCompat,
    #[serde(flatten)]
    pub messages: MessageCompat,
    #[serde(flatten)]
    pub tools: ToolCompat,
    #[serde(flatten)]
    pub schema: SchemaCompat,
    #[serde(flatten)]
    pub reasoning: ReasoningCompat,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct TransportCompat {
    /// Field name for max tokens in request body.
    /// Default: "max_tokens" for all providers.
    pub max_tokens_field: Option<String>,

    /// Custom API path appended to base_url for chat completions.
    /// Default: "/v1/chat/completions" for OpenAI provider.
    pub api_path: Option<String>,

    /// Maximum serialized provider request body size in bytes.
    /// Default: None (no local preflight limit).
    pub max_request_body_bytes: Option<usize>,

    /// Whether OpenAI-compatible requests include stream_options.
    /// Default: true for OpenAI-compatible providers.
    pub include_stream_options: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct MessageCompat {
    /// Merge consecutive assistant messages (text concat + tool_calls merge).
    /// Default: true for openai.
    pub merge_assistant_messages: Option<bool>,

    /// Remove tool_result blocks that have no corresponding tool_use.
    /// Default: true for provider families that support tool results.
    pub clean_orphan_tool_results: Option<bool>,

    /// Deduplicate tool results with same tool_call_id (keep last).
    /// Default: true for openai.
    pub dedup_tool_results: Option<bool>,

    /// Ensure messages alternate user/assistant (insert filler if needed).
    /// Default: true for anthropic/bedrock/vertex.
    pub ensure_alternation: Option<bool>,

    /// Merge consecutive same-role messages into one.
    /// Default: true for anthropic/bedrock/vertex.
    pub merge_same_role: Option<bool>,

    /// Text patterns to strip from message history before sending.
    /// Default: empty.
    pub strip_patterns: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ToolCompat {
    /// Remove tool_use blocks that have no corresponding tool_result.
    /// Default: true for openai.
    pub clean_orphan_tool_calls: Option<bool>,

    /// Downgrade malformed tool_calls in the projected request body.
    /// Default: true for all providers.
    pub sanitize_malformed_tool_calls: Option<bool>,

    /// Auto-generate tool IDs when missing.
    /// Default: true for anthropic/bedrock/vertex.
    pub auto_tool_id: Option<bool>,

    /// Maximum number of tools allowed in the projected provider request.
    /// Default: None (no local preflight limit).
    pub max_tool_count: Option<usize>,

    /// Whether OpenAI-compatible requests include outgoing tools.
    /// Default: true for OpenAI-compatible providers.
    pub emit_tools: Option<bool>,

    /// Explicit tools declaration wire shape.
    /// Default: native provider path shape.
    pub tool_wire_shape: Option<ToolWireShape>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, Eq, PartialEq)]
pub enum ToolWireShape {
    #[serde(rename = "native")]
    Native,
    #[serde(rename = "openai_function")]
    OpenAiFunction,
    #[serde(rename = "anthropic_input_schema")]
    AnthropicInputSchema,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct SchemaCompat {
    /// Sanitize JSON schemas for strict providers.
    /// Default: true for bedrock.
    pub sanitize_schema: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ReasoningCompat {
    /// Whether this provider supports extended thinking.
    /// Default: true for anthropic/bedrock/vertex, false for openai.
    pub supports_thinking: Option<bool>,

    /// Whether this provider supports reasoning_effort.
    /// Default: false for anthropic/bedrock/vertex, true for openai.
    pub supports_effort: Option<bool>,

    /// Available effort levels for this provider.
    /// Only meaningful when supports_effort is true.
    pub effort_levels: Option<Vec<String>>,
}

impl TransportCompat {
    fn merge(defaults: Self, user: Self) -> Self {
        Self {
            max_tokens_field: user.max_tokens_field.or(defaults.max_tokens_field),
            api_path: user.api_path.or(defaults.api_path),
            max_request_body_bytes: user.max_request_body_bytes.or(defaults.max_request_body_bytes),
            include_stream_options: user.include_stream_options.or(defaults.include_stream_options),
        }
    }
}

impl MessageCompat {
    fn merge(defaults: Self, user: Self) -> Self {
        Self {
            merge_assistant_messages: user.merge_assistant_messages.or(defaults.merge_assistant_messages),
            clean_orphan_tool_results: user.clean_orphan_tool_results.or(defaults.clean_orphan_tool_results),
            dedup_tool_results: user.dedup_tool_results.or(defaults.dedup_tool_results),
            ensure_alternation: user.ensure_alternation.or(defaults.ensure_alternation),
            merge_same_role: user.merge_same_role.or(defaults.merge_same_role),
            strip_patterns: user.strip_patterns.or(defaults.strip_patterns),
        }
    }
}

impl ToolCompat {
    fn merge(defaults: Self, user: Self) -> Self {
        Self {
            clean_orphan_tool_calls: user.clean_orphan_tool_calls.or(defaults.clean_orphan_tool_calls),
            sanitize_malformed_tool_calls: user
                .sanitize_malformed_tool_calls
                .or(defaults.sanitize_malformed_tool_calls),
            auto_tool_id: user.auto_tool_id.or(defaults.auto_tool_id),
            max_tool_count: user.max_tool_count.or(defaults.max_tool_count),
            emit_tools: user.emit_tools.or(defaults.emit_tools),
            tool_wire_shape: user.tool_wire_shape.or(defaults.tool_wire_shape),
        }
    }
}

impl SchemaCompat {
    fn merge(defaults: Self, user: Self) -> Self {
        Self {
            sanitize_schema: user.sanitize_schema.or(defaults.sanitize_schema),
        }
    }
}

impl ReasoningCompat {
    fn merge(defaults: Self, user: Self) -> Self {
        Self {
            supports_thinking: user.supports_thinking.or(defaults.supports_thinking),
            supports_effort: user.supports_effort.or(defaults.supports_effort),
            effort_levels: user.effort_levels.or(defaults.effort_levels),
        }
    }
}

impl ProviderCompat {
    /// Defaults for Anthropic-family providers (Anthropic, Vertex)
    pub fn anthropic_defaults() -> Self {
        Self {
            messages: MessageCompat {
                ensure_alternation: Some(true),
                merge_same_role: Some(true),
                clean_orphan_tool_results: Some(true),
                ..Default::default()
            },
            tools: ToolCompat {
                auto_tool_id: Some(true),
                sanitize_malformed_tool_calls: Some(true),
                ..Default::default()
            },
            reasoning: ReasoningCompat {
                supports_thinking: Some(true),
                supports_effort: Some(false),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Defaults for Bedrock (Anthropic + schema sanitization)
    pub fn bedrock_defaults() -> Self {
        Self {
            messages: MessageCompat {
                ensure_alternation: Some(true),
                merge_same_role: Some(true),
                clean_orphan_tool_results: Some(true),
                ..Default::default()
            },
            tools: ToolCompat {
                auto_tool_id: Some(true),
                sanitize_malformed_tool_calls: Some(true),
                ..Default::default()
            },
            schema: SchemaCompat {
                sanitize_schema: Some(true),
            },
            reasoning: ReasoningCompat {
                supports_thinking: Some(true),
                supports_effort: Some(false),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Defaults for OpenAI-compatible providers
    pub fn openai_defaults() -> Self {
        Self {
            transport: TransportCompat {
                max_tokens_field: Some("max_tokens".into()),
                include_stream_options: Some(true),
                ..Default::default()
            },
            messages: MessageCompat {
                merge_assistant_messages: Some(true),
                clean_orphan_tool_results: Some(true),
                dedup_tool_results: Some(true),
                ..Default::default()
            },
            tools: ToolCompat {
                clean_orphan_tool_calls: Some(true),
                sanitize_malformed_tool_calls: Some(true),
                auto_tool_id: Some(true),
                emit_tools: Some(true),
                ..Default::default()
            },
            reasoning: ReasoningCompat {
                supports_thinking: Some(false),
                supports_effort: Some(true),
                effort_levels: Some(vec!["low".into(), "medium".into(), "high".into()]),
            },
            ..Default::default()
        }
    }

    /// Merge user config over defaults (user wins on non-None fields)
    pub fn merge(defaults: Self, user: Self) -> Self {
        Self {
            transport: TransportCompat::merge(defaults.transport, user.transport),
            messages: MessageCompat::merge(defaults.messages, user.messages),
            tools: ToolCompat::merge(defaults.tools, user.tools),
            schema: SchemaCompat::merge(defaults.schema, user.schema),
            reasoning: ReasoningCompat::merge(defaults.reasoning, user.reasoning),
        }
    }

    // --- Resolved accessors (Option<bool> → bool with false default) ---

    pub fn max_tokens_field(&self) -> &str {
        self.transport.max_tokens_field.as_deref().unwrap_or("max_tokens")
    }

    pub fn max_request_body_bytes(&self) -> Option<usize> {
        self.transport.max_request_body_bytes
    }

    pub fn max_tool_count(&self) -> Option<usize> {
        self.tools.max_tool_count
    }

    pub fn include_stream_options(&self) -> bool {
        self.transport.include_stream_options.unwrap_or(true)
    }

    pub fn emit_tools(&self) -> bool {
        self.tools.emit_tools.unwrap_or(true)
    }

    pub fn tool_wire_shape(&self) -> ToolWireShape {
        self.tools.tool_wire_shape.unwrap_or(ToolWireShape::Native)
    }

    pub fn merge_assistant_messages(&self) -> bool {
        self.messages.merge_assistant_messages.unwrap_or(false)
    }

    pub fn clean_orphan_tool_calls(&self) -> bool {
        self.tools.clean_orphan_tool_calls.unwrap_or(false)
    }

    pub fn clean_orphan_tool_results(&self) -> bool {
        self.messages.clean_orphan_tool_results.unwrap_or(false)
    }

    pub fn dedup_tool_results(&self) -> bool {
        self.messages.dedup_tool_results.unwrap_or(false)
    }

    pub fn sanitize_malformed_tool_calls(&self) -> bool {
        self.tools.sanitize_malformed_tool_calls.unwrap_or(false)
    }

    pub fn ensure_alternation(&self) -> bool {
        self.messages.ensure_alternation.unwrap_or(false)
    }

    pub fn merge_same_role(&self) -> bool {
        self.messages.merge_same_role.unwrap_or(false)
    }

    pub fn sanitize_schema(&self) -> bool {
        self.schema.sanitize_schema.unwrap_or(false)
    }

    pub fn auto_tool_id(&self) -> bool {
        self.tools.auto_tool_id.unwrap_or(false)
    }

    pub fn api_path(&self) -> &str {
        self.transport.api_path.as_deref().unwrap_or("/v1/chat/completions")
    }

    pub fn supports_thinking(&self) -> bool {
        self.reasoning.supports_thinking.unwrap_or(false)
    }

    pub fn supports_effort(&self) -> bool {
        self.reasoning.supports_effort.unwrap_or(false)
    }

    pub fn effort_levels(&self) -> &[String] {
        self.reasoning.effort_levels.as_deref().unwrap_or(&[])
    }
}

/// Sanitize a JSON Schema for strict providers (e.g., Bedrock).
/// - Root type must be "object" (wrap if not)
/// - Recursively remove "additionalProperties"
/// - Normalize array types: ["string", "null"] → "string"
pub fn sanitize_json_schema(schema: &Value) -> Value {
    let mut schema = schema.clone();

    // Ensure root type is "object"
    if schema.get("type").and_then(|t| t.as_str()) != Some("object") {
        schema = serde_json::json!({
            "type": "object",
            "properties": {
                "value": schema
            },
            "required": ["value"]
        });
    }

    strip_additional_properties(&mut schema);
    normalize_array_types(&mut schema);
    schema
}

fn strip_additional_properties(val: &mut Value) {
    if let Some(obj) = val.as_object_mut() {
        obj.remove("additionalProperties");
        for v in obj.values_mut() {
            strip_additional_properties(v);
        }
    } else if let Some(arr) = val.as_array_mut() {
        for v in arr.iter_mut() {
            strip_additional_properties(v);
        }
    }
}

fn normalize_array_types(val: &mut Value) {
    if let Some(obj) = val.as_object_mut() {
        // Normalize ["string", "null"] → "string"
        if let Some(arr) = obj.get("type").and_then(Value::as_array) {
            let non_null: Vec<&Value> = arr.iter().filter(|v| v.as_str() != Some("null")).collect();
            if non_null.len() == 1 {
                obj.insert("type".to_string(), non_null[0].clone());
            }
        }
        for v in obj.values_mut() {
            normalize_array_types(v);
        }
    } else if let Some(arr) = val.as_array_mut() {
        for v in arr.iter_mut() {
            normalize_array_types(v);
        }
    }
}

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
