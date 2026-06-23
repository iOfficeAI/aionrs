use aion_providers::error::ProviderError;

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("API error: {0}")]
    ApiError(String),
    #[error(
        "provider repeatedly returned tool-call malformed outputs ({count}/{limit}); stopped to avoid wasting tokens"
    )]
    ToolCallMalformed { count: usize, limit: usize },
    #[error(
        "stopped after {count}/{limit} consecutive tool-call failures; the task did not converge. Try adjusting the request or retrying."
    )]
    ToolCallFailures { count: usize, limit: usize },
    #[error("Provider error: {0}")]
    Provider(#[from] ProviderError),
    #[error("User aborted the session")]
    UserAborted,
    #[error("Context window nearly full ({input_tokens} tokens used, limit {limit})")]
    ContextTooLong { input_tokens: u64, limit: usize },
}
