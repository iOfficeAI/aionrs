/// Abstraction over output channels (terminal vs JSON stream protocol)
pub trait OutputSink: Send + Sync {
    /// Stream text delta from LLM
    fn emit_text_delta(&self, text: &str, msg_id: &str);

    /// Stream thinking content from LLM
    fn emit_thinking(&self, text: &str, msg_id: &str);

    /// Announce a tool call.
    fn emit_tool_call(&self, tool_use_id: &str, name: &str, input: &str);

    /// Display tool result.
    fn emit_tool_result(&self, tool_use_id: &str, name: &str, is_error: bool, content: &str);

    /// Signal start of a new message stream
    fn emit_stream_start(&self, msg_id: &str);

    /// Signal end of a message stream with usage stats
    fn emit_stream_end(
        &self,
        msg_id: &str,
        turns: usize,
        input_tokens: u64,
        output_tokens: u64,
        cache_creation_tokens: u64,
        cache_read_tokens: u64,
    );

    /// Display error
    fn emit_error(&self, msg: &str);

    /// Display informational message
    fn emit_info(&self, msg: &str);

    /// Report the best-known context occupancy for the next provider request.
    ///
    /// `size` is the effective context window when known. Sinks that do not
    /// surface context accounting can keep the default no-op.
    fn emit_context_usage(&self, _used: u64, _size: Option<u64>, _source: &str) {}

    /// Announce that a context compaction pass started.
    fn emit_compact_started(&self, _trigger: &str, _pre_compact_tokens: Option<u64>) {}

    /// Announce that a context compaction pass finished successfully.
    fn emit_compact_finished(
        &self,
        _trigger: &str,
        _pre_compact_tokens: u64,
        _post_compact_tokens: u64,
        _messages_summarized: usize,
    ) {
    }
}
