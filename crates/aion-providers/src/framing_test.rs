use super::*;

#[cfg(test)]
mod tests {
    use super::{Frame, FrameKind, SseBlockFramer, SseLineFramer, Utf8StreamDecoder, bedrock_payload_to_frame};
    use base64::Engine as _;

    #[test]
    fn test_utf8_decoder_reassembles_cjk_split_across_chunks() {
        let original = "权限管理服务器";
        let bytes = original.as_bytes();

        // Split at byte offsets that fall in the MIDDLE of multibyte (3-byte
        // CJK) characters, across three push() calls.
        let split_a = 4; // inside the 2nd char (chars start at 0, 3, 6, ...)
        let split_b = 13; // inside the 5th char
        assert!(!original.is_char_boundary(split_a));
        assert!(!original.is_char_boundary(split_b));

        let mut decoder = Utf8StreamDecoder::default();
        let mut out = String::new();
        out.push_str(&decoder.push(&bytes[..split_a]));
        out.push_str(&decoder.push(&bytes[split_a..split_b]));
        out.push_str(&decoder.push(&bytes[split_b..]));
        out.push_str(&decoder.flush());

        assert_eq!(out, original);
        assert!(!out.contains('\u{FFFD}'));
    }

    #[test]
    fn test_utf8_decoder_flush_handles_truncated_tail_without_panic() {
        let mut decoder = Utf8StreamDecoder::default();

        // A single leading byte of a 3-byte sequence with no continuation.
        let head = &"权".as_bytes()[..1];
        assert_eq!(decoder.push(head), "");

        // Genuinely truncated stream end: flush decodes lossily, no panic.
        let flushed = decoder.flush();
        assert!(flushed.contains('\u{FFFD}'));

        // Decoder is drained after flush.
        assert_eq!(decoder.flush(), "");
    }

    #[test]
    fn test_utf8_decoder_passes_through_complete_ascii() {
        let mut decoder = Utf8StreamDecoder::default();
        assert_eq!(decoder.push(b"data: hello\n"), "data: hello\n");
        assert_eq!(decoder.flush(), "");
    }

    #[test]
    fn test_sse_line_framer_extracts_data_and_done() {
        let mut framer = SseLineFramer::default();

        let frames = framer.push_text(
            ": keepalive\n\nignored\ndata: {\"type\":\"chunk\"}\ndata: [DONE]\n",
            "[DONE]",
        );

        assert_eq!(
            frames,
            vec![
                Frame {
                    event: None,
                    data: "{\"type\":\"chunk\"}".to_string(),
                    kind: FrameKind::Data,
                },
                Frame {
                    event: None,
                    data: "[DONE]".to_string(),
                    kind: FrameKind::Done,
                },
            ]
        );
    }

    #[test]
    fn test_sse_line_framer_keeps_partial_line_buffered() {
        let mut framer = SseLineFramer::default();

        assert!(framer.push_text("data: partial", "[DONE]").is_empty());

        assert_eq!(
            framer.push_text(" line\n", "[DONE]"),
            vec![Frame {
                event: None,
                data: "partial line".to_string(),
                kind: FrameKind::Data,
            }]
        );
    }

    #[test]
    fn test_sse_block_framer_extracts_event_and_data() {
        let mut framer = SseBlockFramer::default();

        let frames = framer.push_text("event: content_block_delta\ndata: first\ndata: second\n\n");

        assert_eq!(
            frames,
            vec![
                Frame {
                    event: Some("content_block_delta".to_string()),
                    data: "first".to_string(),
                    kind: FrameKind::Data,
                },
                Frame {
                    event: Some("content_block_delta".to_string()),
                    data: "second".to_string(),
                    kind: FrameKind::Data,
                },
            ]
        );
    }

    #[test]
    fn test_sse_block_framer_keeps_partial_block_buffered() {
        let mut framer = SseBlockFramer::default();

        assert!(framer.push_text("event: message_delta\ndata: body").is_empty());

        assert_eq!(
            framer.push_text("\n\n"),
            vec![Frame {
                event: Some("message_delta".to_string()),
                data: "body".to_string(),
                kind: FrameKind::Data,
            }]
        );
    }

    #[test]
    fn test_sse_block_framer_preserves_payload_whitespace() {
        let mut framer = SseBlockFramer::default();

        assert_eq!(
            framer.push_text("event: message_delta\ndata: body \n\n"),
            vec![Frame {
                event: Some("message_delta".to_string()),
                data: "body ".to_string(),
                kind: FrameKind::Data,
            }]
        );
    }

    #[test]
    fn test_sse_block_framer_does_not_accept_leading_space_fields() {
        let mut framer = SseBlockFramer::default();

        assert!(framer.push_text(" event: ignored\n data: ignored\n\n").is_empty());
    }

    #[test]
    fn test_bedrock_payload_frame_decodes_base64_bytes() {
        let inner = r#"{"type":"content_block_delta","delta":{"text":"hi"}}"#;
        let encoded = base64::engine::general_purpose::STANDARD.encode(inner);
        let payload = format!(r#"{{"bytes":"{}"}}"#, encoded);

        assert_eq!(
            bedrock_payload_to_frame(payload.as_bytes()),
            Some(Frame {
                event: Some("content_block_delta".to_string()),
                data: inner.to_string(),
                kind: FrameKind::Data,
            })
        );
    }

    #[test]
    fn test_bedrock_payload_frame_ignores_invalid_payload() {
        assert_eq!(bedrock_payload_to_frame(b"not json"), None);
        assert_eq!(bedrock_payload_to_frame(br#"{"bytes":"not base64"}"#), None);

        let invalid_utf8 = base64::engine::general_purpose::STANDARD.encode([0xff]);
        let payload = format!(r#"{{"bytes":"{}"}}"#, invalid_utf8);
        assert_eq!(bedrock_payload_to_frame(payload.as_bytes()), None);

        let invalid_inner_json = base64::engine::general_purpose::STANDARD.encode("not json");
        let payload = format!(r#"{{"bytes":"{}"}}"#, invalid_inner_json);
        assert_eq!(bedrock_payload_to_frame(payload.as_bytes()), None);
    }

    #[test]
    fn test_bedrock_payload_frame_uses_empty_event_for_missing_type() {
        let inner = r#"{"delta":{"text":"hi"}}"#;
        let encoded = base64::engine::general_purpose::STANDARD.encode(inner);
        let payload = format!(r#"{{"bytes":"{}"}}"#, encoded);

        assert_eq!(
            bedrock_payload_to_frame(payload.as_bytes()),
            Some(Frame {
                event: Some(String::new()),
                data: inner.to_string(),
                kind: FrameKind::Data,
            })
        );
    }
}
