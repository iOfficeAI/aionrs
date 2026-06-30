use super::*;

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
        let writer = ProtocolWriter::new();
        let event = ProtocolEvent::Ready {
            version: "0.1.0".to_string(),
            session_id: None,
            capabilities: Capabilities {
                tool_approval: true,
                thinking: false,
                effort: false,
                effort_levels: vec![],
                modes: vec!["default".into(), "auto_edit".into(), "yolo".into()],
                current_mode: "default".into(),
                mcp: false,
            },
        };
        let _ = writer.emit(&event);
    }
}
