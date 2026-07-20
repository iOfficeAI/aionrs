use super::*;

#[cfg(test)]
mod tests {
    use aion_types::tool::ToolDef;
    use serde_json::json;

    use super::*;

    fn request(messages: Vec<Message>, tools: Vec<ToolDef>) -> LlmRequest {
        LlmRequest {
            model: "gpt-5.6-sol".to_string(),
            system: "Be helpful".to_string(),
            messages,
            tools,
            max_tokens: Some(4096),
            thinking: None,
            reasoning_effort: Some("high".to_string()),
        }
    }

    fn tool() -> ToolDef {
        ToolDef {
            name: "get_weather".to_string(),
            description: "Get weather".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {"city": {"type": "string"}},
                "required": ["city"]
            }),
            deferred: false,
        }
    }

    #[test]
    fn projects_responses_request_fields_and_flat_function_tools() {
        let messages = vec![Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "Weather?".to_string(),
            }],
        )];

        let body =
            OpenAiResponsesProjector::project(&request(messages, vec![tool()]), &ProviderCompat::openai_defaults())
                .expect("Responses body should project");

        assert_eq!(body["model"], "gpt-5.6-sol");
        assert_eq!(body["instructions"], "Be helpful");
        assert_eq!(body["max_output_tokens"], 4096);
        assert_eq!(body["reasoning"]["effort"], "high");
        assert_eq!(body["store"], false);
        assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
        assert_eq!(
            body["input"][0]["content"][0],
            json!({"type": "input_text", "text": "Weather?"})
        );
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["name"], "get_weather");
        assert_eq!(body["tools"][0]["strict"], false);
        assert!(body["tools"][0].get("function").is_none());
        assert!(body.get("messages").is_none());
        assert!(body.get("max_tokens").is_none());
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn replays_reasoning_function_call_and_function_output_items() {
        let reasoning = json!({
            "id": "rs_1",
            "type": "reasoning",
            "encrypted_content": "encrypted",
            "summary": []
        });
        let function_call = json!({
            "id": "fc_1",
            "type": "function_call",
            "status": "completed",
            "call_id": "call_1",
            "name": "get_weather",
            "arguments": "{\"city\":\"Hangzhou\"}"
        });
        let extra = json!({
            (PROVIDER_ITEM_OWNER): {
                "function_call": function_call.clone()
            }
        });
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::ProviderItem {
                        provider: PROVIDER_ITEM_OWNER.to_string(),
                        item: reasoning.clone(),
                    },
                    ContentBlock::ToolUse {
                        id: "call_1".to_string(),
                        name: "get_weather".to_string(),
                        input: json!({"city": "Hangzhou"}),
                        extra: Some(extra),
                    },
                ],
            ),
            Message::new(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "call_1".to_string(),
                    content: "sunny".to_string(),
                    is_error: false,
                }],
            ),
        ];

        let body =
            OpenAiResponsesProjector::project(&request(messages, vec![tool()]), &ProviderCompat::openai_defaults())
                .expect("Responses body should project");

        assert_eq!(body["input"][0], reasoning);
        assert_eq!(body["input"][1], function_call);
        assert_eq!(
            body["input"][2],
            json!({"type": "function_call_output", "call_id": "call_1", "output": "sunny"})
        );
    }

    #[test]
    fn ignores_opaque_items_owned_by_other_providers() {
        let messages = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::ProviderItem {
                provider: "other".to_string(),
                item: json!({"type": "reasoning", "id": "rs_other"}),
            }],
        )];

        let body =
            OpenAiResponsesProjector::project(&request(messages, Vec::new()), &ProviderCompat::openai_defaults())
                .expect("Responses body should project");

        assert_eq!(body["input"], json!([]));
    }
}
