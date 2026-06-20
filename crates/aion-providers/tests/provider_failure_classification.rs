use aion_providers::error::{
    ProviderError, ProviderFailure, ProviderFailureKind, ProviderFailurePhase,
    ProviderRawSignalSource, ProviderRawSignals, ToolCallFailureReason, ToolSchemaSource,
    classify_provider_failure, provider_failure_from_error,
};

fn raw_signals(status: Option<u16>, message: impl Into<String>) -> ProviderRawSignals {
    ProviderRawSignals {
        provider: "openai".to_string(),
        model: Some("gpt-4o-mini".to_string()),
        status,
        raw_code: None,
        raw_type: None,
        message: Some(message.into()),
        request_id: Some("req_123".to_string()),
        retry_after_ms: None,
        source: ProviderRawSignalSource::Http,
        phase: ProviderFailurePhase::BeforeFirstDelta,
    }
}

#[test]
fn classifies_401_as_auth_failed_and_preserves_provider_status() {
    let mut raw = raw_signals(Some(401), "Incorrect API key provided");
    raw.raw_code = Some("invalid_api_key".to_string());

    let failure = classify_provider_failure(raw);

    assert_eq!(failure.kind, ProviderFailureKind::AuthFailed);
    assert_eq!(failure.meta.provider, "openai");
    assert_eq!(failure.meta.model.as_deref(), Some("gpt-4o-mini"));
    assert_eq!(failure.meta.status, Some(401));
    assert_eq!(failure.meta.request_id.as_deref(), Some("req_123"));
    assert_eq!(
        failure.meta.raw.raw_code.as_deref(),
        Some("invalid_api_key")
    );
    assert_eq!(
        failure.meta.raw.message.as_deref(),
        Some("Incorrect API key provided")
    );
}

#[test]
fn classifies_429_as_rate_limited_and_preserves_retry_after() {
    let mut raw = raw_signals(Some(429), "rate limit exceeded");
    raw.retry_after_ms = Some(2_500);

    let failure = classify_provider_failure(raw);

    assert_eq!(failure.kind, ProviderFailureKind::RateLimited);
    assert_eq!(failure.meta.status, Some(429));
    assert_eq!(failure.meta.retry_after_ms, Some(2_500));
}

#[tokio::test]
async fn provider_error_http_without_status_is_transport_failure() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let error = reqwest::Client::new()
        .post(format!("http://{addr}/v1/messages"))
        .send()
        .await
        .unwrap_err();
    assert_eq!(error.status(), None);

    let failure = provider_failure_from_error(
        "anthropic",
        Some("claude-3-5-sonnet".to_string()),
        ProviderError::Http(error),
        ProviderFailurePhase::BeforeFirstDelta,
    );

    assert_eq!(failure.kind, ProviderFailureKind::Transport);
    assert_eq!(failure.meta.phase, ProviderFailurePhase::BeforeFirstDelta);
}

#[test]
fn classifies_context_window_messages_as_context_too_large() {
    for message in [
        "maximum context length is 128000 tokens",
        "too many tokens requested",
        "prompt too long for the selected model",
    ] {
        let failure = classify_provider_failure(raw_signals(Some(400), message));

        assert_eq!(
            failure.kind,
            ProviderFailureKind::ContextTooLarge,
            "message should classify as ContextTooLarge: {message}"
        );
    }
}

#[test]
fn classifies_json_source_400_as_response_parse_when_no_specific_rule() {
    let mut raw = raw_signals(Some(400), "malformed provider payload");
    raw.source = ProviderRawSignalSource::Json;

    let failure = classify_provider_failure(raw);

    assert_eq!(failure.kind, ProviderFailureKind::ResponseParse);
}

#[test]
fn classifies_404_with_model_context_but_no_model_signal_as_endpoint_not_found() {
    let failure = classify_provider_failure(raw_signals(Some(404), "not found"));

    assert_eq!(failure.kind, ProviderFailureKind::EndpointNotFound);
}

#[test]
fn classifies_404_with_model_not_found_raw_code_as_model_not_found() {
    let mut raw = raw_signals(Some(404), "not found");
    raw.raw_code = Some("model_not_found".to_string());

    let failure = classify_provider_failure(raw);

    assert_eq!(failure.kind, ProviderFailureKind::ModelNotFound);
}

#[test]
fn classifies_404_with_model_message_text_as_model_not_found() {
    let failure = classify_provider_failure(raw_signals(Some(404), "model gpt-x not found"));

    assert_eq!(failure.kind, ProviderFailureKind::ModelNotFound);
}

#[test]
fn tool_call_invalid_summarizes_and_redacts_detail_like_meta_raw_message() {
    let secret = "sk-this-secret-value-should-not-survive";
    let reason = format!("malformed tool call {secret} {}", "x".repeat(900));

    let failure = ProviderFailure::tool_call_invalid(
        "anthropic".to_string(),
        Some("claude-3-5-sonnet".to_string()),
        reason,
        ProviderFailurePhase::AfterToolCall,
    );

    let detail = match &failure.kind {
        ProviderFailureKind::ToolCall(tool_call) => tool_call.detail.as_deref().unwrap(),
        other => panic!("expected ToolCall failure, got {other:?}"),
    };
    let raw_message = failure.meta.raw.message.as_deref().unwrap();

    assert_eq!(detail, raw_message);
    assert!(detail.chars().count() <= 512);
    assert!(detail.contains("[redacted]"));
    assert!(!detail.contains(secret));
}

#[test]
fn redacts_aws_access_key_shapes_in_raw_message_summary() {
    let akia_key = "AKIAIOSFODNN7EXAMPLE";
    let asia_key = "ASIAIOSFODNN7EXAMPLE";
    let failure = classify_provider_failure(raw_signals(
        None,
        format!("credentials leaked: {akia_key} and {asia_key}"),
    ));

    let summary = failure.meta.raw.message.as_deref().unwrap();

    assert!(!summary.contains(akia_key));
    assert!(!summary.contains(asia_key));
    assert_eq!(summary.matches("[redacted]").count(), 2);
}

#[test]
fn provider_error_conversion_defaults_phase_to_before_first_delta() {
    for error in [
        ProviderError::Api {
            status: 500,
            message: "server error".to_string(),
        },
        ProviderError::Parse("bad sse".to_string()),
        ProviderError::RateLimited {
            retry_after_ms: 1_000,
        },
        ProviderError::PromptTooLong("prompt too long".to_string()),
        ProviderError::Connection("disconnect".to_string()),
    ] {
        let failure = ProviderFailure::from(error);

        assert_eq!(failure.meta.phase, ProviderFailurePhase::BeforeFirstDelta);
    }
}

#[test]
fn classifies_provider_tool_call_invalid_without_mcp_tool_naming() {
    let failure = ProviderFailure::tool_call_invalid(
        "anthropic".to_string(),
        Some("claude-3-5-sonnet".to_string()),
        "missing tool call id".to_string(),
        ProviderFailurePhase::AfterToolCall,
    );

    match &failure.kind {
        ProviderFailureKind::ToolCall(tool_call) => {
            assert_eq!(
                tool_call.reason,
                ToolCallFailureReason::InvalidProviderToolCall
            );
            assert_eq!(tool_call.detail.as_deref(), Some("missing tool call id"));
        }
        other => panic!("expected ToolCall failure, got {other:?}"),
    }

    assert_eq!(failure.meta.provider, "anthropic");
    assert_eq!(failure.meta.model.as_deref(), Some("claude-3-5-sonnet"));
    assert_eq!(failure.meta.phase, ProviderFailurePhase::AfterToolCall);
    assert_eq!(
        format!("{:?}", ToolSchemaSource::McpCapability),
        "McpCapability"
    );
    let forbidden_mcp_tool_name = ["Mcp", "Tool"].join("");
    assert!(!format!("{failure:?}").contains(&forbidden_mcp_tool_name));
}
