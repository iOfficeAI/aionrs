use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_sink_does_not_panic() {
        let sink = NullSink;
        sink.emit_text_delta("hello", "msg1");
        sink.emit_thinking("thought", "msg1");
        sink.emit_tool_call("call_read_1", "Read", "{}");
        sink.emit_tool_result("call_read_1", "Read", false, "ok");
        sink.emit_stream_start("msg1");
        sink.emit_stream_end("msg1", 1, 100, 50, 0, 0);
        sink.emit_error("err");
        sink.emit_info("info");
    }
}
