use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use serde_json::json;
    use tokio::sync::mpsc;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

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
}
