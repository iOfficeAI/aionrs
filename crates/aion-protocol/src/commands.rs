use serde::Deserialize;

/// Commands sent from the client to the agent (Client -> Agent)
#[derive(Debug, Deserialize, PartialEq, Eq)]
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
        #[serde(default)]
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
    SetConfig {
        #[serde(default)]
        model: Option<String>,
    },
}

#[derive(Debug, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalScope {
    #[default]
    Once,
    Always,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    Default,
    AutoEdit,
    Yolo,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_config_debug_format() {
        let cmd = ProtocolCommand::SetConfig {
            model: Some("test-model".into()),
        };
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("SetConfig"));
        assert!(dbg.contains("test-model"));
    }

    #[test]
    fn set_config_equality() {
        let a = ProtocolCommand::SetConfig {
            model: Some("m".into()),
        };
        let b = ProtocolCommand::SetConfig {
            model: Some("m".into()),
        };
        assert_eq!(a, b);

        let c = ProtocolCommand::SetConfig { model: None };
        assert_ne!(a, c);
    }
}
