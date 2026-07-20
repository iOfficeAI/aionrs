use super::*;

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_text_reasoning_and_refusal_deltas() {
        let mut state = StreamState::new();

        let text = parse_sse_chunk(r#"{"type":"response.output_text.delta","delta":"Hello"}"#, &mut state);
        let reasoning = parse_sse_chunk(
            r#"{"type":"response.reasoning_summary_text.delta","delta":"Thinking"}"#,
            &mut state,
        );
        let refusal = parse_sse_chunk(
            r#"{"type":"response.refusal.delta","delta":"Cannot do that"}"#,
            &mut state,
        );

        assert!(matches!(&text[0], LlmEvent::TextDelta(delta) if delta == "Hello"));
        assert!(matches!(&reasoning[0], LlmEvent::ThinkingDelta(delta) if delta == "Thinking"));
        assert!(matches!(&refusal[0], LlmEvent::TextDelta(delta) if delta == "Cannot do that"));
    }

    #[test]
    fn parses_reasoning_and_function_call_items_with_call_id() {
        let mut state = StreamState::new();
        let reasoning = parse_sse_chunk(
            r#"{"type":"response.output_item.done","item":{"id":"rs_1","type":"reasoning","encrypted_content":"secret","summary":[]}}"#,
            &mut state,
        );
        let tool = parse_sse_chunk(
            r#"{"type":"response.output_item.done","item":{"id":"fc_1","type":"function_call","status":"completed","call_id":"call_1","name":"read","arguments":"{\"path\":\"a.txt\"}"}}"#,
            &mut state,
        );

        assert!(matches!(
            &reasoning[0],
            LlmEvent::ProviderItem { provider, item }
                if provider == PROVIDER_ITEM_OWNER && item["encrypted_content"] == "secret"
        ));
        match &tool[0] {
            LlmEvent::ToolUse { id, name, input, extra } => {
                assert_eq!(id, "call_1");
                assert_eq!(name, "read");
                assert_eq!(input, &json!({"path": "a.txt"}));
                assert_eq!(
                    extra.as_ref().unwrap()[PROVIDER_ITEM_OWNER]["function_call"]["id"],
                    "fc_1"
                );
            }
            event => panic!("expected ToolUse, got {event:?}"),
        }
    }

    #[test]
    fn completed_response_deduplicates_items_and_emits_usage() {
        let mut state = StreamState::new();
        let item = r#"{"type":"response.output_item.done","item":{"id":"fc_1","type":"function_call","call_id":"call_1","name":"read","arguments":"{}"}}"#;
        assert_eq!(parse_sse_chunk(item, &mut state).len(), 1);

        let completed = parse_sse_chunk(
            r#"{"type":"response.completed","response":{"status":"completed","output":[{"id":"fc_1","type":"function_call","call_id":"call_1","name":"read","arguments":"{}"}],"usage":{"input_tokens":100,"input_tokens_details":{"cached_tokens":80},"output_tokens":20}}}"#,
            &mut state,
        );

        assert_eq!(completed.len(), 1);
        match &completed[0] {
            LlmEvent::Done { stop_reason, usage } => {
                assert_eq!(*stop_reason, StopReason::ToolUse);
                assert_eq!(usage.input_tokens, 100);
                assert_eq!(usage.cache_read_tokens, 80);
                assert_eq!(usage.output_tokens, 20);
            }
            event => panic!("expected Done, got {event:?}"),
        }
        assert!(state.is_terminal());
    }

    #[test]
    fn incomplete_max_output_tokens_maps_to_max_tokens() {
        let mut state = StreamState::new();
        let events = parse_sse_chunk(
            r#"{"type":"response.incomplete","response":{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"usage":{"input_tokens":10,"output_tokens":5}}}"#,
            &mut state,
        );

        assert!(matches!(
            &events[0],
            LlmEvent::Done { stop_reason: StopReason::MaxTokens, usage }
                if usage.input_tokens == 10 && usage.output_tokens == 5
        ));
        assert!(state.is_terminal());
    }

    #[test]
    fn failed_and_error_events_surface_provider_messages() {
        let mut failed_state = StreamState::new();
        let failed = parse_sse_chunk(
            r#"{"type":"response.failed","response":{"error":{"message":"failed message"}}}"#,
            &mut failed_state,
        );
        let mut error_state = StreamState::new();
        let error = parse_sse_chunk(
            r#"{"type":"error","error":{"message":"stream message"}}"#,
            &mut error_state,
        );

        assert!(matches!(&failed[0], LlmEvent::Error(message) if message == "failed message"));
        assert!(matches!(&error[0], LlmEvent::Error(message) if message == "stream message"));
        assert!(failed_state.is_terminal());
        assert!(error_state.is_terminal());
    }
}
