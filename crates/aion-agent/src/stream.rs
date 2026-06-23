use aion_types::message::{ContentBlock, StopReason, TokenUsage};

/// Everything a single provider stream produces in one model turn.
///
/// Collected by `AgentEngine::consume_stream` so the main loop deals with a
/// single named value instead of six mutable locals.
pub(crate) struct StreamOutcome {
    pub(crate) assistant_text: String,
    pub(crate) thinking_text: String,
    pub(crate) thinking_signature: Option<String>,
    pub(crate) tool_calls: Vec<ContentBlock>,
    pub(crate) stop_reason: StopReason,
    pub(crate) usage: TokenUsage,
}
