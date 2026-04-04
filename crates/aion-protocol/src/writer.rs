use std::io::{self, BufWriter, Stdout, Write};
use std::sync::Mutex;

use crate::events::ProtocolEvent;

/// Thread-safe JSON Lines writer to stdout
pub struct ProtocolWriter {
    writer: Mutex<BufWriter<Stdout>>,
}

impl ProtocolWriter {
    pub fn new() -> Self {
        Self {
            writer: Mutex::new(BufWriter::new(io::stdout())),
        }
    }

    /// Serialize and write a protocol event as a single JSON line to stdout
    pub fn emit(&self, event: &ProtocolEvent) -> io::Result<()> {
        let mut w = self
            .writer
            .lock()
            .map_err(|_| io::Error::other("protocol writer lock poisoned"))?;
        serde_json::to_writer(&mut *w, event)
            .map_err(|e| io::Error::other(format!("failed to serialize protocol event: {}", e)))?;
        writeln!(&mut *w)?;
        w.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{Capabilities, ProtocolEvent};

    #[test]
    fn test_writer_construction() {
        let _writer = ProtocolWriter::new();
    }

    #[test]
    fn test_writer_emit_does_not_panic() {
        // Just verify emit doesn't panic (output goes to stdout)
        let writer = ProtocolWriter::new();
        let event = ProtocolEvent::Ready {
            version: "0.1.0".to_string(),
            session_id: None,
            capabilities: Capabilities {
                tool_approval: true,
                thinking: false,
                mcp: false,
            },
        };
        let _ = writer.emit(&event);
    }
}

