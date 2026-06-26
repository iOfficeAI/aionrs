use base64::Engine as _;
use serde_json::Value;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum FrameKind {
    Data,
    Done,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Frame {
    pub event: Option<String>,
    pub data: String,
    pub kind: FrameKind,
}

#[derive(Default)]
pub(crate) struct SseLineFramer {
    buffer: String,
}

#[derive(Default)]
pub(crate) struct SseBlockFramer {
    buffer: String,
    current_event_type: Option<String>,
}

pub(crate) fn bedrock_payload_to_frame(payload: &[u8]) -> Option<Frame> {
    let wrapper = serde_json::from_slice::<Value>(payload).ok()?;
    let b64 = wrapper.get("bytes")?.as_str()?;
    let decoded = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    let inner = String::from_utf8(decoded).ok()?;
    let inner_json = serde_json::from_str::<Value>(&inner).ok()?;
    let event_type = inner_json
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    Some(Frame {
        event: Some(event_type),
        data: inner,
        kind: FrameKind::Data,
    })
}

impl SseLineFramer {
    pub(crate) fn push_text(&mut self, text: &str, done_sentinel: &str) -> Vec<Frame> {
        self.buffer.push_str(text);

        let mut frames = Vec::new();
        while let Some(line_end) = self.buffer.find('\n') {
            let line = self.buffer.drain(..=line_end).collect::<String>();
            let line = line.trim();

            if line.is_empty() || line.starts_with(':') {
                continue;
            }

            if let Some(data) = line.strip_prefix("data: ") {
                frames.push(Frame {
                    event: None,
                    data: data.to_string(),
                    kind: if data == done_sentinel {
                        FrameKind::Done
                    } else {
                        FrameKind::Data
                    },
                });
            }
        }

        frames
    }
}

impl SseBlockFramer {
    pub(crate) fn push_text(&mut self, text: &str) -> Vec<Frame> {
        self.buffer.push_str(text);

        let mut frames = Vec::new();
        while let Some(block_end) = self.buffer.find("\n\n") {
            let block = self.buffer.drain(..block_end + 2).collect::<String>();
            let block = &block[..block_end];

            for line in block.lines() {
                let line = line.strip_suffix('\r').unwrap_or(line);
                if let Some(event_type) = line.strip_prefix("event: ") {
                    self.current_event_type = Some(event_type.to_string());
                } else if let Some(data) = line.strip_prefix("data: ") {
                    frames.push(Frame {
                        event: self.current_event_type.clone(),
                        data: data.to_string(),
                        kind: FrameKind::Data,
                    });
                }
            }
        }

        frames
    }
}

#[cfg(test)]
mod tests {
    use super::{Frame, FrameKind, SseBlockFramer, SseLineFramer, bedrock_payload_to_frame};
    use base64::Engine as _;

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

        assert!(
            framer
                .push_text("event: message_delta\ndata: body")
                .is_empty()
        );

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

        assert!(
            framer
                .push_text(" event: ignored\n data: ignored\n\n")
                .is_empty()
        );
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
