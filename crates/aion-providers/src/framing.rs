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

/// Decodes a byte stream into UTF-8 text incrementally, carrying an incomplete
/// trailing multibyte sequence over to the next chunk.
///
/// `bytes_stream()` can split a multibyte UTF-8 character (e.g. a 3-byte CJK
/// char) across two network chunks. Lossy-decoding each chunk on its own would
/// turn both halves into U+FFFD and drop the character. This decoder only emits
/// complete UTF-8 sequences and retains the incomplete tail until more bytes
/// arrive.
#[derive(Default)]
pub(crate) struct Utf8StreamDecoder {
    pending: Vec<u8>,
}

impl Utf8StreamDecoder {
    /// Append `chunk` to the pending bytes and return the longest valid UTF-8
    /// prefix as an owned `String`, retaining any incomplete trailing sequence
    /// for the next call. The returned string may be empty.
    pub(crate) fn push(&mut self, chunk: &[u8]) -> String {
        self.pending.extend_from_slice(chunk);

        let valid_up_to = match std::str::from_utf8(&self.pending) {
            Ok(_) => self.pending.len(),
            Err(error) => error.valid_up_to(),
        };

        // A truly invalid leading byte (valid_up_to == 0) would stick forever.
        // The retained tail of an incomplete sequence is at most 3 bytes, so
        // once pending grows past that, drop one leading byte lossily to make
        // progress instead of buffering unboundedly.
        if valid_up_to == 0 {
            if self.pending.len() > 3 {
                let dropped = self.pending.remove(0);
                return String::from_utf8_lossy(&[dropped]).into_owned();
            }
            return String::new();
        }

        let decoded = String::from_utf8_lossy(&self.pending[..valid_up_to]).into_owned();
        self.pending.drain(..valid_up_to);
        decoded
    }

    /// Decode any remaining pending bytes at the true end of the stream. A
    /// genuinely-truncated trailing sequence is decoded lossily (yielding
    /// U+FFFD) since no further bytes will arrive.
    pub(crate) fn flush(&mut self) -> String {
        if self.pending.is_empty() {
            return String::new();
        }
        let decoded = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        decoded
    }
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
