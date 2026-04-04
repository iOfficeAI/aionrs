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
