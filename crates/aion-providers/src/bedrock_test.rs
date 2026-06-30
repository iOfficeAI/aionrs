use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use aion_types::message::{ContentBlock, Message, Role};
    use aion_types::tool::ToolDef;
    use serde_json::json;

    // --- Golden body snapshots (baseline for compat-split / seam-extraction refactors) ---

    fn bedrock_test_provider() -> BedrockProvider {
        BedrockProvider::new(
            "us-east-1",
            AwsCredentials::Explicit {
                access_key_id: "test-key".to_string(),
                secret_access_key: "test-secret".to_string(),
                session_token: None,
            },
            false,
            ProviderCompat::bedrock_defaults(),
        )
    }

    fn bedrock_req(messages: Vec<Message>, tools: Vec<ToolDef>) -> LlmRequest {
        LlmRequest {
            model: "test-model".to_string(),
            system: "You are a test assistant.".to_string(),
            messages,
            tools,
            max_tokens: 8192,
            thinking: None,
            reasoning_effort: None,
        }
    }

    fn bedrock_tools() -> Vec<ToolDef> {
        vec![ToolDef {
            name: "read".to_string(),
            description: "Read".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {"path": {"type": ["string", "null"]}},
                "additionalProperties": false
            }),
            deferred: false,
        }]
    }

    macro_rules! assert_bedrock_json_snapshot {
        ($name:literal, $value:expr) => {
            insta::with_settings!({ prepend_module_to_snapshot => false }, {
                insta::assert_json_snapshot!(
                    concat!("aion_providers__bedrock__tests__", $name),
                    $value
                );
            });
        };
    }

    #[test]
    fn golden_bedrock_basic() {
        let p = bedrock_test_provider();
        let r = bedrock_req(
            vec![Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "Hello".to_string(),
                }],
            )],
            vec![],
        );
        assert_bedrock_json_snapshot!(
            "bedrock_basic",
            p.build_request_body(&r)
                .expect("request body projection should succeed")
        );
    }

    #[test]
    fn golden_bedrock_with_tools() {
        let p = bedrock_test_provider();
        let r = bedrock_req(
            vec![Message::new(
                Role::User,
                vec![ContentBlock::Text { text: "go".to_string() }],
            )],
            bedrock_tools(),
        );
        assert_bedrock_json_snapshot!(
            "bedrock_with_tools",
            p.build_request_body(&r)
                .expect("request body projection should succeed")
        );
    }
}
