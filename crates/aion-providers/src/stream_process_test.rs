use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use serde_json::{Value, json};
    use tokio::sync::mpsc;
    use tracing::{Level, subscriber};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::test_support::SharedLogWriter;

    fn aws_event_message(payload: &[u8]) -> Vec<u8> {
        let total_len = 12 + payload.len() + 4;
        let mut message = Vec::with_capacity(total_len);
        message.extend_from_slice(&(total_len as u32).to_be_bytes());
        message.extend_from_slice(&0u32.to_be_bytes());
        message.extend_from_slice(&0u32.to_be_bytes());
        message.extend_from_slice(payload);
        message.extend_from_slice(&0u32.to_be_bytes());
        message
    }

    fn bedrock_event_payload(inner: &str) -> Vec<u8> {
        json!({
            "bytes": STANDARD.encode(inner)
        })
        .to_string()
        .into_bytes()
    }

    async fn mock_response(body: Vec<u8>) -> reqwest::Response {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/stream"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .mount(&server)
            .await;

        reqwest::get(format!("{}/stream", server.uri()))
            .await
            .expect("mock response should be available")
    }

    async fn collect_events(mut rx: mpsc::Receiver<LlmEvent>) -> Vec<LlmEvent> {
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        events
    }

    fn openai_sse_body(include_done: bool) -> Vec<u8> {
        let content = json!({
            "choices": [{
                "delta": {"content": "Hello"},
                "finish_reason": null
            }]
        });
        let finish = json!({
            "choices": [{
                "delta": {},
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 12,
                "completion_tokens": 3
            }
        });
        let mut body = format!("data: {content}\n\ndata: {finish}\n\n");
        if include_done {
            body.push_str("data: [DONE]\n\n");
        }
        body.into_bytes()
    }

    fn provider_stream_summary(writer: &SharedLogWriter) -> Value {
        writer
            .contents()
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|event| event["fields"]["diagnostic_event"] == "provider_stream_summary")
            .expect("provider stream summary should be logged")
    }

    #[test]
    fn parse_aws_event_waits_for_complete_message_and_extracts_payload() {
        let payload = b"payload";
        let message = aws_event_message(payload);

        assert!(parse_aws_event(&message[..message.len() - 1]).is_none());

        let (event_data, consumed) = parse_aws_event(&message).expect("complete event should parse");
        assert_eq!(event_data, Some(payload.to_vec()));
        assert_eq!(consumed, message.len());
    }

    #[tokio::test]
    async fn openai_sse_stream_logs_done_termination_from_real_processing_path() {
        let response = mock_response(openai_sse_body(true)).await;
        let (tx, rx) = mpsc::channel(8);
        let writer = SharedLogWriter::default();
        let log_subscriber = tracing_subscriber::fmt()
            .json()
            .with_max_level(Level::TRACE)
            .with_writer(writer.clone())
            .finish();
        let _guard = subscriber::set_default(log_subscriber);

        let outcome = process_openai_sse_stream(response, &tx, false).await;
        drop(tx);
        let events = collect_events(rx).await;
        let summary = provider_stream_summary(&writer);

        assert!(matches!(outcome, StreamOutcome::Ok));
        assert_eq!(events.len(), 2);
        assert_eq!(summary["level"], "DEBUG");
        assert_eq!(summary["fields"]["termination"], "done");
        assert_eq!(summary["fields"]["done_seen"], true);
        assert_eq!(summary["fields"]["incomplete_stream"], false);
        assert_eq!(summary["fields"]["parsed_text_event_count"], 1);
        assert_eq!(summary["fields"]["parsed_done_event_count"], 1);
    }

    #[tokio::test]
    async fn openai_sse_stream_logs_eof_termination_from_real_processing_path() {
        let response = mock_response(openai_sse_body(false)).await;
        let (tx, rx) = mpsc::channel(8);
        let writer = SharedLogWriter::default();
        let log_subscriber = tracing_subscriber::fmt()
            .json()
            .with_max_level(Level::TRACE)
            .with_writer(writer.clone())
            .finish();
        let _guard = subscriber::set_default(log_subscriber);

        let outcome = process_openai_sse_stream(response, &tx, false).await;
        drop(tx);
        let events = collect_events(rx).await;
        let summary = provider_stream_summary(&writer);

        assert!(matches!(outcome, StreamOutcome::Ok));
        assert_eq!(events.len(), 1);
        assert_eq!(summary["level"], "WARN");
        assert_eq!(summary["fields"]["termination"], "eof");
        assert_eq!(summary["fields"]["done_seen"], false);
        assert_eq!(summary["fields"]["incomplete_stream"], true);
        assert_eq!(summary["fields"]["empty_answer"], false);
        assert_eq!(summary["fields"]["malformed_json"], false);
        assert_eq!(summary["fields"]["unexpected_finish_reason"], false);
    }

    #[tokio::test]
    async fn bedrock_event_stream_decodes_payloads_into_llm_events() {
        let mut body = Vec::new();
        for inner in [
            r#"{"type":"message_start","message":{"usage":{"input_tokens":12}}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":7}}"#,
        ] {
            body.extend(aws_event_message(&bedrock_event_payload(inner)));
        }

        let response = mock_response(body).await;
        let (tx, rx) = mpsc::channel(8);

        let outcome = process_bedrock_aws_event_stream(response, &tx).await;
        drop(tx);
        let events = collect_events(rx).await;

        assert!(matches!(outcome, StreamOutcome::Ok));
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], LlmEvent::TextDelta(text) if text == "Hello"));
        match &events[1] {
            LlmEvent::Done { stop_reason, usage } => {
                assert_eq!(*stop_reason, StopReason::EndTurn);
                assert_eq!(usage.input_tokens, 12);
                assert_eq!(usage.output_tokens, 7);
            }
            event => panic!("expected Done event, got {event:?}"),
        }
    }

    #[tokio::test]
    async fn bedrock_event_stream_synthesizes_done_when_message_delta_is_missing() {
        let mut body = Vec::new();
        for inner in [
            r#"{"type":"message_start","message":{"usage":{"input_tokens":12}}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
        ] {
            body.extend(aws_event_message(&bedrock_event_payload(inner)));
        }

        let response = mock_response(body).await;
        let (tx, rx) = mpsc::channel(8);

        let outcome = process_bedrock_aws_event_stream(response, &tx).await;
        drop(tx);
        let events = collect_events(rx).await;

        assert!(matches!(outcome, StreamOutcome::Ok));
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], LlmEvent::TextDelta(text) if text == "Hello"));
        match &events[1] {
            LlmEvent::Done { stop_reason, usage } => {
                assert_eq!(*stop_reason, StopReason::EndTurn);
                assert_eq!(usage.input_tokens, 12);
                assert_eq!(usage.output_tokens, 0);
            }
            event => panic!("expected synthesized Done event, got {event:?}"),
        }
    }

    #[tokio::test]
    async fn openai_responses_stream_decodes_typed_events_until_completed() {
        let body = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello\"}\n\n",
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"id\":\"fc_1\",\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"read\",\"arguments\":\"{}\"}}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":8,\"output_tokens\":3}}}\n\n"
        );
        let response = mock_response(body.as_bytes().to_vec()).await;
        let (tx, rx) = mpsc::channel(8);

        let outcome = process_openai_responses_sse_stream(response, &tx).await;
        drop(tx);
        let events = collect_events(rx).await;

        assert!(matches!(outcome, StreamOutcome::Ok));
        assert_eq!(events.len(), 3);
        assert!(matches!(&events[0], LlmEvent::TextDelta(text) if text == "Hello"));
        assert!(matches!(&events[1], LlmEvent::ToolUse { id, .. } if id == "call_1"));
        assert!(matches!(
            &events[2],
            LlmEvent::Done { stop_reason: StopReason::ToolUse, usage }
                if usage.input_tokens == 8 && usage.output_tokens == 3
        ));
    }

    #[tokio::test]
    async fn openai_responses_stream_without_terminal_event_fails_partial() {
        let body = b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n".to_vec();
        let response = mock_response(body).await;
        let (tx, rx) = mpsc::channel(8);

        let outcome = process_openai_responses_sse_stream(response, &tx).await;
        drop(tx);
        let events = collect_events(rx).await;

        assert!(matches!(outcome, StreamOutcome::FailedPartial(_)));
        assert!(matches!(&events[0], LlmEvent::TextDelta(text) if text == "partial"));
    }
}
