use super::*;

#[cfg(test)]
mod tests {
    use super::{AnthropicParser, OpenAiParser, ResponseParser};
    use crate::framing::{Frame, FrameKind};
    use aion_types::llm::LlmEvent;
    use aion_types::message::StopReason;

    #[test]
    fn openai_done_frame_flushes_empty_state_to_no_events() {
        let parser = OpenAiParser { auto_tool_id: false };
        let mut state = parser.new_state();
        let frame = Frame {
            event: None,
            data: "[DONE]".to_string(),
            kind: FrameKind::Done,
        };

        let events = parser.parse_frame(&frame, &mut state);

        assert!(events.is_empty());
    }

    #[test]
    fn openai_done_frame_flushes_pending_done_with_usage() {
        let parser = OpenAiParser { auto_tool_id: false };
        let mut state = parser.new_state();

        let finish_frame = Frame {
            event: None,
            data: r#"{"choices":[{"delta":{"content":"hi"},"finish_reason":"stop"}]}"#.to_string(),
            kind: FrameKind::Data,
        };
        let finish_events = parser.parse_frame(&finish_frame, &mut state);
        assert_eq!(finish_events.len(), 1);
        assert!(matches!(&finish_events[0], LlmEvent::TextDelta(text) if text == "hi"));

        let usage_frame = Frame {
            event: None,
            data: r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#.to_string(),
            kind: FrameKind::Data,
        };
        assert!(parser.parse_frame(&usage_frame, &mut state).is_empty());

        let done_frame = Frame {
            event: None,
            data: "[DONE]".to_string(),
            kind: FrameKind::Done,
        };
        let done_events = parser.parse_frame(&done_frame, &mut state);
        assert_eq!(done_events.len(), 1);
        match &done_events[0] {
            LlmEvent::Done { stop_reason, usage } => {
                assert_eq!(*stop_reason, StopReason::EndTurn);
                assert_eq!(usage.input_tokens, 10);
                assert_eq!(usage.output_tokens, 5);
            }
            event => panic!("expected Done, got {event:?}"),
        }
    }

    #[test]
    fn anthropic_data_frame_routes_event_type_to_text_delta() {
        let parser = AnthropicParser;
        let mut state = parser.new_state();
        let frame = Frame {
            event: Some("content_block_delta".to_string()),
            data: r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#
                .to_string(),
            kind: FrameKind::Data,
        };

        let events = parser.parse_frame(&frame, &mut state);

        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], LlmEvent::TextDelta(text) if text == "Hello"));
    }

    #[test]
    fn anthropic_invalid_json_frame_returns_no_events() {
        let parser = AnthropicParser;
        let mut state = parser.new_state();
        let frame = Frame {
            event: Some("content_block_delta".to_string()),
            data: "not json".to_string(),
            kind: FrameKind::Data,
        };

        let events = parser.parse_frame(&frame, &mut state);

        assert!(events.is_empty());
    }
}
