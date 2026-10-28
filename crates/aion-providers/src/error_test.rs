use super::*;

#[cfg(test)]
mod retryable_tests {
    use super::*;

    // F1-11
    #[test]
    fn test_api_400_not_retryable() {
        assert!(
            !ProviderError::Api {
                status: 400,
                message: "empty name".into(),
            }
            .is_retryable()
        );
        assert!(
            ProviderError::RateLimited {
                retry_after_ms: 1000,
                body: None,
            }
            .is_retryable()
        );
        assert!(ProviderError::Connection("x".into()).is_retryable());
    }
}

#[cfg(test)]
mod prompt_too_long_tests {
    use super::*;

    /// The exact body sglang returns for an oversized request.
    const SGLANG_BODY: &str = r#"{"object":"error","message":"Requested token count exceeds the model's maximum context length of 131077 tokens. You requested a total of 148969 tokens: 128969 tokens from the input messages and 20000 tokens for the completion. Please reduce the number of tokens in the input messages or the completion to fit within the limit.","type":"BadRequestError","param":null,"code":400}"#;

    #[test]
    fn openai_compatible_400_becomes_prompt_too_long() {
        let body: serde_json::Value = serde_json::from_str(SGLANG_BODY).expect("valid JSON");

        let error =
            provider_error_from_json_body(&body, SGLANG_BODY.as_bytes()).expect("status 400 should map to an error");
        assert!(
            matches!(error, ProviderError::PromptTooLong(_)),
            "sglang's oversized-prompt 400 must be recognized: {error:?}"
        );
    }

    #[test]
    fn anthropic_style_message_is_recognized() {
        let classified = prompt_too_long(400, "prompt is too long: 213589 tokens > 200000 maximum");
        assert!(matches!(classified, Some(ProviderError::PromptTooLong(_))));
    }

    #[test]
    fn payload_too_large_status_is_recognized() {
        let classified = prompt_too_long(413, "request body too many tokens");
        assert!(matches!(classified, Some(ProviderError::PromptTooLong(_))));
    }

    #[test]
    fn ordinary_400_stays_an_api_error() {
        for message in ["invalid request: missing model", "tool schema rejected"] {
            assert!(
                prompt_too_long(400, message).is_none(),
                "`{message}` is not an overflow"
            );
        }
    }

    #[test]
    fn window_overflow_text_with_a_500_status_is_not_reclassified() {
        // A 5xx carrying this wording is a gateway failure; reclassifying it
        // would make the turn compact instead of retrying.
        assert!(prompt_too_long(502, "maximum context length").is_none());
    }
}

#[cfg(test)]
mod json_error_body_tests {
    use super::*;

    #[test]
    fn json_top_level_error_shape_is_normalized() {
        let body_text = r#"{"code":"503","message":"upstream busy"}"#;
        let body = serde_json::from_str(body_text).expect("test body should be valid JSON");
        let error =
            provider_error_from_json_body(&body, body_text.as_bytes()).expect("status 503 should map to an error");

        assert!(matches!(
            error,
            ProviderError::Api { status: 503, message } if message == "upstream busy"
        ));
    }

    #[test]
    fn json_error_uses_http_status_when_provider_code_is_not_http() {
        let body_text = r#"{"error":{"code":1001,"message":"upstream busy"},"status":503}"#;
        let body = serde_json::from_str(body_text).expect("test body should be valid JSON");
        let error =
            provider_error_from_json_body(&body, body_text.as_bytes()).expect("status 503 should map to an error");

        assert!(matches!(
            error,
            ProviderError::Api { status: 503, message } if message == "upstream busy"
        ));
    }

    #[test]
    fn json_without_error_shape_is_not_mapped_to_an_error() {
        let body_text = r#"{"choices":[]}"#;
        let body = serde_json::from_str(body_text).expect("test body should be valid JSON");

        assert!(provider_error_from_json_body(&body, body_text.as_bytes()).is_none());
    }

    #[test]
    fn json_error_without_http_status_is_a_parse_error() {
        let body_text = r#"{"error":{"code":"resource_exhausted","message":"upstream busy"}}"#;
        let body = serde_json::from_str(body_text).expect("test body should be valid JSON");
        let error =
            provider_error_from_json_body(&body, body_text.as_bytes()).expect("explicit error should be surfaced");

        assert!(matches!(
            error,
            ProviderError::Parse(message) if message.contains("without an HTTP status")
        ));
    }

    #[test]
    fn null_error_field_is_not_mapped_to_an_error() {
        // Normal 200 bodies from some gateways carry `"error": null`; they
        // must pass through untouched on both the transport and SSE paths.
        let body_text = r#"{"error":null,"choices":[{"message":{"content":"hi"}}]}"#;
        let body = serde_json::from_str(body_text).expect("test body should be valid JSON");

        assert!(provider_error_from_json_body(&body, body_text.as_bytes()).is_none());
    }

    #[test]
    fn rate_limit_status_maps_to_rate_limited_with_body() {
        let body_text = r#"{"error":{"code":429,"message":"quota exceeded"}}"#;
        let body = serde_json::from_str(body_text).expect("test body should be valid JSON");
        let error =
            provider_error_from_json_body(&body, body_text.as_bytes()).expect("status 429 should map to an error");

        assert!(matches!(
            error,
            ProviderError::RateLimited { body: Some(body), .. } if body == body_text
        ));
    }
}

#[cfg(test)]
mod context_limit_hint_tests {
    use super::*;

    #[test]
    fn parses_openai_style_message() {
        let message =
            "This model's maximum context length is 8192 tokens. However, your messages resulted in 10000 tokens.";
        assert_eq!(parse_context_limit_hint(message), Some(8192));
    }

    #[test]
    fn parses_vllm_style_message_with_separators() {
        let message = "Requested token count exceeds the model's maximum context length of 131,077 tokens.";
        assert_eq!(parse_context_limit_hint(message), Some(131_077));
    }

    #[test]
    fn parses_anthropic_style_message() {
        let message = "prompt is too long: 213000 tokens > 200000 maximum";
        assert_eq!(parse_context_limit_hint(message), Some(200_000));
    }

    #[test]
    fn ignores_messages_without_a_plausible_limit() {
        assert_eq!(parse_context_limit_hint("invalid api key"), None);
        assert_eq!(parse_context_limit_hint("context length is 12 tokens"), None);
    }
}
