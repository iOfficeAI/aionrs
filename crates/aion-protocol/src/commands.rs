use serde::Deserialize;

/// Commands sent from the client to the agent (Client -> Agent)
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub enum ProtocolCommand {
    Message {
        msg_id: String,
        input: String,
        #[serde(default)]
        files: Vec<String>,
    },
    Stop,
    ToolApprove {
        call_id: String,
        #[serde(default = "default_scope")]
        scope: ApprovalScope,
    },
    ToolDeny {
        call_id: String,
        #[serde(default)]
        reason: String,
    },
    InitHistory {
        text: String,
    },
    SetMode {
        mode: SessionMode,
    },
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalScope {
    #[default]
    Once,
    Always,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    Default,
    AutoEdit,
    Yolo,
}

fn default_scope() -> ApprovalScope {
    ApprovalScope::Once
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_command_deserialization() {
        let json = r#"{"type":"message","msg_id":"m1","input":"Hello"}"#;
        let cmd: ProtocolCommand = serde_json::from_str(json).unwrap();
        match cmd {
            ProtocolCommand::Message {
                msg_id,
                input,
                files,
            } => {
                assert_eq!(msg_id, "m1");
                assert_eq!(input, "Hello");
                assert!(files.is_empty());
            }
            _ => panic!("expected Message"),
        }
    }

    #[test]
    fn test_message_command_with_files() {
        let json =
            r#"{"type":"message","msg_id":"m2","input":"Read this","files":["/tmp/a.rs"]}"#;
        let cmd: ProtocolCommand = serde_json::from_str(json).unwrap();
        match cmd {
            ProtocolCommand::Message { files, .. } => {
                assert_eq!(files, vec!["/tmp/a.rs"]);
            }
            _ => panic!("expected Message"),
        }
    }

    #[test]
    fn test_stop_command_deserialization() {
        let json = r#"{"type":"stop"}"#;
        let cmd: ProtocolCommand = serde_json::from_str(json).unwrap();
        assert!(matches!(cmd, ProtocolCommand::Stop));
    }

    #[test]
    fn test_tool_approve_default_scope() {
        let json = r#"{"type":"tool_approve","call_id":"c1"}"#;
        let cmd: ProtocolCommand = serde_json::from_str(json).unwrap();
        match cmd {
            ProtocolCommand::ToolApprove { call_id, scope } => {
                assert_eq!(call_id, "c1");
                assert!(matches!(scope, ApprovalScope::Once));
            }
            _ => panic!("expected ToolApprove"),
        }
    }

    #[test]
    fn test_tool_approve_always_scope() {
        let json = r#"{"type":"tool_approve","call_id":"c1","scope":"always"}"#;
        let cmd: ProtocolCommand = serde_json::from_str(json).unwrap();
        match cmd {
            ProtocolCommand::ToolApprove { scope, .. } => {
                assert!(matches!(scope, ApprovalScope::Always));
            }
            _ => panic!("expected ToolApprove"),
        }
    }

    #[test]
    fn test_tool_deny_command() {
        let json = r#"{"type":"tool_deny","call_id":"c1","reason":"not allowed"}"#;
        let cmd: ProtocolCommand = serde_json::from_str(json).unwrap();
        match cmd {
            ProtocolCommand::ToolDeny { call_id, reason } => {
                assert_eq!(call_id, "c1");
                assert_eq!(reason, "not allowed");
            }
            _ => panic!("expected ToolDeny"),
        }
    }

    #[test]
    fn test_set_mode_command() {
        let json = r#"{"type":"set_mode","mode":"yolo"}"#;
        let cmd: ProtocolCommand = serde_json::from_str(json).unwrap();
        assert!(matches!(
            cmd,
            ProtocolCommand::SetMode {
                mode: SessionMode::Yolo
            }
        ));
    }
}
