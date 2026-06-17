use aion_config::compat::ProviderCompat;
use aion_providers::LlmProvider;
use aion_providers::anthropic_shared;
use aion_providers::openai::OpenAIProvider;
use aion_types::llm::{LlmEvent, LlmRequest};
use aion_types::message::{ContentBlock, Message, Role};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn malformed_history() -> Vec<Message> {
    vec![
        Message::new(
            Role::Assistant,
            vec![
                ContentBlock::Text {
                    text: "writing".into(),
                },
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
    ]
}

fn openai_request(messages: Vec<Message>) -> LlmRequest {
    LlmRequest {
        model: "gpt-4o".into(),
        system: "".into(),
        messages,
        tools: vec![],
        max_tokens: 128,
        thinking: None,
        reasoning_effort: None,
    }
}

async fn collect_events(mut rx: tokio::sync::mpsc::Receiver<LlmEvent>) -> Vec<LlmEvent> {
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    events
}

fn openai_sse_body() -> &'static str {
    concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":null}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1}}\n\n",
        "data: [DONE]\n\n",
    )
}

async fn openai_projected_messages(request: &LlmRequest) -> Vec<Value> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(openai_sse_body(), "text/event-stream"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let provider =
        OpenAIProvider::new("test-key", &server.uri(), ProviderCompat::openai_defaults());
    let rx = provider.stream(request).await.unwrap();
    let _ = collect_events(rx).await;

    server.verify().await;
    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1, "expected exactly one OpenAI request");
    let body: Value = received[0].body_json().unwrap();
    body["messages"].as_array().cloned().unwrap()
}

// F1-12
#[tokio::test]
async fn test_projection_does_not_mutate_history() {
    let request = openai_request(malformed_history());
    let before = serde_json::to_string(&request.messages).unwrap();

    let _ = openai_projected_messages(&request).await;
    let _ =
        anthropic_shared::build_messages(&request.messages, &ProviderCompat::anthropic_defaults());

    let after = serde_json::to_string(&request.messages).unwrap();
    assert_eq!(
        before, after,
        "history MUST be byte-identical after projection"
    );
}

// F1-12
#[tokio::test]
async fn test_both_providers_produce_no_empty_name_and_no_orphan() {
    let messages = malformed_history();
    let request = openai_request(messages.clone());

    let oa = openai_projected_messages(&request).await;
    assert!(oa.iter().all(|m| m["role"] != "tool"));
    let any_openai_empty = oa
        .iter()
        .flat_map(|m| m["tool_calls"].as_array().cloned().unwrap_or_default())
        .any(|tc| tc["function"]["name"] == "");
    assert!(!any_openai_empty);
    let openai_assistant_content = oa
        .iter()
        .find(|m| m["role"] == "assistant")
        .and_then(|m| m["content"].as_str())
        .expect("expected OpenAI assistant content");
    assert!(openai_assistant_content.contains("[tool call skipped:"));
    assert!(openai_assistant_content.contains("arguments={}"));

    let an = anthropic_shared::build_messages(&messages, &ProviderCompat::anthropic_defaults());
    let any_empty = an
        .iter()
        .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
        .any(|b| b["type"] == "tool_use" && b["name"] == "");
    assert!(!any_empty);
    let any_orphan = an
        .iter()
        .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
        .any(|b| b["type"] == "tool_result" && b["tool_use_id"] == "call_x");
    assert!(!any_orphan);
}
