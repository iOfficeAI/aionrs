use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    // --- usage token parsing ---

    #[test]
    fn test_usage_from_trailing_chunk() {
        // OpenAI sends usage in a trailing chunk where choices:[] — the Done
        // event must carry the token counts from that chunk, not zeros.
        let mut state = StreamState::new();

        // chunk 1: finish_reason + text delta, no usage
        let chunk1 = r#"{"choices":[{"delta":{"content":"hi"},"finish_reason":"stop"}]}"#;
        let events = parse_sse_chunk(chunk1, &mut state, false);
        // TextDelta is emitted immediately; Done is deferred.
        assert!(
            events.iter().all(|e| !matches!(e, LlmEvent::Done { .. })),
            "Done should be deferred, not emitted with finish_reason chunk"
        );
        assert!(state.pending_done.is_some());

        // chunk 2: trailing usage-only chunk (choices:[])
        let chunk2 = r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#;
        let events2 = parse_sse_chunk(chunk2, &mut state, false);
        assert!(events2.is_empty());
        assert_eq!(state.input_tokens, 10);
        assert_eq!(state.output_tokens, 5);

        // [DONE] — flush with final counts
        let done = state.flush_done().expect("pending_done should be Some");
        match done {
            LlmEvent::Done { stop_reason, usage } => {
                assert_eq!(stop_reason, StopReason::EndTurn);
                assert_eq!(usage.input_tokens, 10);
                assert_eq!(usage.output_tokens, 5);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn test_usage_in_finish_chunk() {
        // Some providers/models include usage in the same chunk as finish_reason.
        // Counts should still be correct after flush.
        let mut state = StreamState::new();

        // No text delta here, only finish_reason + usage in the same chunk.
        let chunk =
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":8,"completion_tokens":3}}"#;
        let events = parse_sse_chunk(chunk, &mut state, false);
        assert!(
            events.iter().all(|e| !matches!(e, LlmEvent::Done { .. })),
            "Done should be deferred even when usage is in the finish chunk"
        );
        assert_eq!(state.output_tokens, 3);

        let done = state.flush_done().unwrap();
        match done {
            LlmEvent::Done { usage, .. } => {
                assert_eq!(usage.output_tokens, 3);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn deepseek_cache_hit_tokens_are_an_input_subset() {
        let mut state = StreamState::new();

        let chunk = r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":693,"completion_tokens":102,"total_tokens":795,"prompt_tokens_details":{"cached_tokens":512},"completion_tokens_details":{"reasoning_tokens":44},"prompt_cache_hit_tokens":512,"prompt_cache_miss_tokens":181}}"#;
        let _ = parse_sse_chunk(chunk, &mut state, false);

        assert_eq!(state.input_tokens, 693);
        assert_eq!(state.output_tokens, 102);
        assert_eq!(state.cache_read_tokens, 512);

        let done = state.flush_done().unwrap();
        match done {
            LlmEvent::Done { usage, .. } => {
                assert_eq!(usage.input_tokens, 693);
                assert_eq!(usage.output_tokens, 102);
                assert_eq!(usage.cache_read_tokens, 512);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn kimi_top_level_cached_tokens_are_an_input_subset() {
        let mut state = StreamState::new();

        let chunk = r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":19,"completion_tokens":21,"total_tokens":40,"cached_tokens":10}}"#;
        let _ = parse_sse_chunk(chunk, &mut state, false);

        assert_eq!(state.input_tokens, 19);
        assert_eq!(state.output_tokens, 21);
        assert_eq!(state.cache_read_tokens, 10);
    }

    #[test]
    fn usage_with_prompt_tokens_details_cached() {
        // OpenAI standard: prompt_tokens already includes cached_tokens (it's the total)
        // prompt_tokens_details.cached_tokens is informational only
        let mut state = StreamState::new();

        let chunk = r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":1000000,"completion_tokens":100,"prompt_tokens_details":{"cached_tokens":999000}}}"#;
        let _ = parse_sse_chunk(chunk, &mut state, false);

        // prompt_tokens is already the full total for OpenAI
        assert_eq!(state.input_tokens, 1_000_000);
        assert_eq!(state.output_tokens, 100);
        assert_eq!(state.cache_read_tokens, 999_000);
    }

    #[test]
    fn usage_without_cache_fields_unchanged() {
        // Provider that only sends prompt_tokens (no cache fields)
        let mut state = StreamState::new();

        let chunk = r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":50000,"completion_tokens":200}}"#;
        let _ = parse_sse_chunk(chunk, &mut state, false);

        assert_eq!(state.input_tokens, 50_000);
        assert_eq!(state.output_tokens, 200);
    }

    #[test]
    fn tool_calls_with_stop_finish_reason() {
        // Gemini uses finish_reason:"stop" even when tool_calls are present.
        // The accumulated tool calls must still be emitted.
        let mut state = StreamState::new();

        // chunk 1: tool call delta (name + partial args)
        let chunk1 = r#"{"choices":[{"delta":{"role":"assistant","tool_calls":[{"extra_content":{},"function":{"arguments":"{\"skill\":\"test\",\"args\":\"hello\"}","name":"Skill"},"id":"call_abc123","type":"function"}]},"index":0}]}"#;
        let events1 = parse_sse_chunk(chunk1, &mut state, false);
        assert!(events1.is_empty(), "no events until finish_reason");
        assert_eq!(state.tool_calls.len(), 1);
        assert_eq!(state.tool_calls[0].name, "Skill");

        // chunk 2: finish_reason:"stop" (not "tool_calls")
        let chunk2 = r#"{"choices":[{"delta":{"role":"assistant"},"finish_reason":"stop","index":0}],"usage":{"prompt_tokens":100,"completion_tokens":20,"total_tokens":120}}"#;
        let events2 = parse_sse_chunk(chunk2, &mut state, false);

        // Tool call should be emitted
        let tool_events: Vec<_> = events2
            .iter()
            .filter(|e| matches!(e, LlmEvent::ToolUse { .. }))
            .collect();
        assert_eq!(tool_events.len(), 1, "tool call should be emitted on stop");
        if let LlmEvent::ToolUse { id, name, input, .. } = &tool_events[0] {
            assert_eq!(id, "call_abc123");
            assert_eq!(name, "Skill");
            assert_eq!(input["skill"], "test");
        }

        // Done should be deferred with ToolUse stop reason
        let done = state.flush_done().unwrap();
        match done {
            LlmEvent::Done { stop_reason, .. } => {
                assert_eq!(stop_reason, StopReason::ToolUse);
            }
            other => panic!("expected Done with ToolUse, got {other:?}"),
        }

        assert!(state.tool_calls.is_empty(), "tool calls should be drained");
    }

    // F1-9
    #[test]
    fn test_empty_name_toolcall_still_emitted_to_history() {
        let mut state = StreamState::new();

        let chunk1 = r#"{"choices":[{"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"call_x","type":"function","function":{"name":"","arguments":"{}"}}]},"index":0}]}"#;
        let events1 = parse_sse_chunk(chunk1, &mut state, false);
        assert!(events1.is_empty(), "no events until finish_reason");

        let chunk2 = r#"{"choices":[{"delta":{},"finish_reason":"tool_calls","index":0}]}"#;
        let events2 = parse_sse_chunk(chunk2, &mut state, false);

        let tool_use_name = events2.iter().find_map(|event| match event {
            LlmEvent::ToolUse { name, .. } => Some(name.clone()),
            _ => None,
        });

        assert_eq!(
            tool_use_name,
            Some(String::new()),
            "empty-name tool_call must still be emitted and recorded as-is"
        );
    }

    #[test]
    fn stop_without_tool_calls_unchanged() {
        // Standard stop without tool calls should still produce EndTurn.
        let mut state = StreamState::new();

        let chunk = r#"{"choices":[{"delta":{"content":"done"},"finish_reason":"stop","index":0}]}"#;
        let events = parse_sse_chunk(chunk, &mut state, false);

        let text_events: Vec<_> = events.iter().filter(|e| matches!(e, LlmEvent::TextDelta(_))).collect();
        assert_eq!(text_events.len(), 1);

        let done = state.flush_done().unwrap();
        match done {
            LlmEvent::Done { stop_reason, .. } => {
                assert_eq!(stop_reason, StopReason::EndTurn);
            }
            other => panic!("expected Done with EndTurn, got {other:?}"),
        }
    }

    #[test]
    fn test_auto_tool_id_generates_id_when_empty() {
        let mut state = StreamState::new();

        // Simulate a provider that returns tool_calls without an id field
        let chunk = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"get_weather","arguments":"{\"city\":\"Beijing\"}"}}]},"finish_reason":"tool_calls","index":0}]}"#;
        let events = parse_sse_chunk(chunk, &mut state, true);

        let tool_use = events
            .iter()
            .find(|e| matches!(e, LlmEvent::ToolUse { .. }))
            .expect("should emit ToolUse event");

        if let LlmEvent::ToolUse { id, name, .. } = tool_use {
            assert!(!id.is_empty(), "id should be auto-generated, not empty");
            assert!(id.starts_with("call_"), "id should have call_ prefix");
            assert_eq!(name, "get_weather");
        }
    }

    #[test]
    fn test_auto_tool_id_preserves_existing_id() {
        let mut state = StreamState::new();

        let chunk = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_existing_123","function":{"name":"read_file","arguments":"{}"}}]},"finish_reason":"tool_calls","index":0}]}"#;
        let events = parse_sse_chunk(chunk, &mut state, true);

        let tool_use = events
            .iter()
            .find(|e| matches!(e, LlmEvent::ToolUse { .. }))
            .expect("should emit ToolUse event");

        if let LlmEvent::ToolUse { id, .. } = tool_use {
            assert_eq!(id, "call_existing_123", "existing id should be preserved");
        }
    }

    #[test]
    fn test_auto_tool_id_disabled_keeps_empty() {
        let mut state = StreamState::new();

        let chunk = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"get_weather","arguments":"{}"}}]},"finish_reason":"tool_calls","index":0}]}"#;
        let events = parse_sse_chunk(chunk, &mut state, false);

        let tool_use = events
            .iter()
            .find(|e| matches!(e, LlmEvent::ToolUse { .. }))
            .expect("should emit ToolUse event");

        if let LlmEvent::ToolUse { id, .. } = tool_use {
            assert!(id.is_empty(), "id should remain empty when auto_tool_id is disabled");
        }
    }
}
