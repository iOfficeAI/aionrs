use aion_providers::error::ProviderFailure;

#[derive(Debug, thiserror::Error)]
pub enum AgentFailure {
    #[error(
        "provider repeatedly returned malformed tool calls ({count}/{limit}); stopped to avoid wasting tokens"
    )]
    RepeatedMalformedToolCall { count: usize, limit: usize },
    #[error("Provider error: {0}")]
    Provider(#[from] ProviderFailure),
    #[error("User aborted the session")]
    UserAborted,
    #[error("Context window nearly full ({input_tokens} tokens used, limit {limit})")]
    ContextTooLong { input_tokens: u64, limit: usize },
    #[error("Command error: {0}")]
    Command(CommandFailure),
    #[error("Internal error: {0}")]
    Internal(InternalFailure),
}

pub type AgentError = AgentFailure;

#[derive(Debug, thiserror::Error)]
pub enum CommandFailure {
    #[error("slash command failed: {command}")]
    Failed { command: String },
}

#[derive(Debug, thiserror::Error)]
pub enum InternalFailure {
    #[error("{message}")]
    Unexpected { message: String },
}
