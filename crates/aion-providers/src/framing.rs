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
    let event_type = inner_json.get("type").and_then(Value::as_str).unwrap_or("").to_string();

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
#[path = "framing_test.rs"]
mod framing_test;
