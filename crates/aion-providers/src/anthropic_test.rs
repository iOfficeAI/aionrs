use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use aion_types::llm::ThinkingConfig;
    use aion_types::message::{ContentBlock, Message, Role};
    use aion_types::tool::ToolDef;
    use serde_json::json;

    fn anthropic_golden(cache: bool) -> AnthropicProvider {
        AnthropicProvider::new("test-key", "https://example.test", ProviderCompat::anthropic_defaults())
            .with_cache(cache)
    }

    fn areq(messages: Vec<Message>, tools: Vec<ToolDef>, thinking: Option<ThinkingConfig>) -> LlmRequest {
        LlmRequest {
            model: "test-model".to_string(),
            system: "You are a test assistant.".to_string(),
            messages,
            tools,
            max_tokens: Some(8192),
            thinking,
            reasoning_effort: None,
        }
    }

    fn atools() -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "read".to_string(),
                description: "Read".to_string(),
                input_schema: json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}),
                deferred: false,
            },
            ToolDef {
                name: "list".to_string(),
                description: "List".to_string(),
                input_schema: json!({"type":"object","properties":{}}),
                deferred: false,
            },
        ]
    }

    macro_rules! assert_anthropic_json_snapshot {
        ($name:literal, $value:expr) => {
            insta::with_settings!({ prepend_module_to_snapshot => false }, {
                insta::assert_json_snapshot!(
                    concat!("aion_providers__anthropic__tests__", $name),
                    $value
                );
            });
        };
    }

    #[test]
    fn golden_anthropic_basic() {
        let p = anthropic_golden(false);
        let r = areq(
            vec![Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "Hello".to_string(),
                }],
            )],
            vec![],
            None,
        );
        assert_anthropic_json_snapshot!(
            "anthropic_basic",
            p.build_request_body(&r)
                .expect("request body projection should succeed")
        );
    }

    #[test]
    fn golden_anthropic_with_tools_no_cache() {
        let p = anthropic_golden(false);
        let r = areq(
            vec![Message::new(
                Role::User,
                vec![ContentBlock::Text { text: "go".to_string() }],
            )],
            atools(),
            None,
        );
        assert_anthropic_json_snapshot!(
            "anthropic_with_tools_no_cache",
            p.build_request_body(&r)
                .expect("request body projection should succeed")
        );
    }

    #[test]
    fn golden_anthropic_with_cache() {
        let p = anthropic_golden(true);
        let r = areq(
            vec![Message::new(
                Role::User,
                vec![ContentBlock::Text { text: "go".to_string() }],
            )],
            atools(),
            None,
        );
        assert_anthropic_json_snapshot!(
            "anthropic_with_cache",
            p.build_request_body(&r)
                .expect("request body projection should succeed")
        );
    }

    #[test]
    fn golden_anthropic_with_thinking() {
        let p = anthropic_golden(false);
        let r = areq(
            vec![Message::new(
                Role::User,
                vec![ContentBlock::Text { text: "q".to_string() }],
            )],
            vec![],
            Some(ThinkingConfig::Enabled { budget_tokens: 4096 }),
        );
        assert_anthropic_json_snapshot!(
            "anthropic_with_thinking",
            p.build_request_body(&r)
                .expect("request body projection should succeed")
        );
    }
}
