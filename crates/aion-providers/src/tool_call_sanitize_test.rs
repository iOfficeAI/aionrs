use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // F1-8
    #[test]
    fn test_format_dropped_tool_call_template() {
        assert_eq!(
            format_dropped_tool_call(DroppedToolCallReason::EmptyName, &json!({})),
            "[tool call skipped: malformed (empty function name). arguments={}. This call was not executed; re-issue with a valid name if still needed.]"
        );
        assert_eq!(
            format_dropped_tool_call(DroppedToolCallReason::EmptyName, &json!({"a":1})),
            "[tool call skipped: malformed (empty function name). arguments={\"a\":1}. This call was not executed; re-issue with a valid name if still needed.]"
        );
    }

    // F2-8
    #[test]
    fn test_format_dropped_tool_call_empty_id_template() {
        assert_eq!(
            format_dropped_tool_call(DroppedToolCallReason::EmptyId, &json!({"command":"ls"})),
            "[tool call skipped: malformed (empty tool call id). arguments={\"command\":\"ls\"}. This call was not executed; re-issue with a valid id if still needed.]"
        );
    }

    // F1-6
    #[test]
    fn test_format_truncates_at_char_boundary() {
        // 150 multi-byte chars; must truncate to 100 chars with `…`, no panic.
        let big = "中".repeat(150);
        let out = format_dropped_tool_call(DroppedToolCallReason::EmptyId, &json!({"k": big}));
        assert!(out.contains('…'));
        assert!(out.starts_with("[tool call skipped:"));
        // Pin the exact 100-char truncation boundary: the args segment between
        // `arguments=` and the `…` ellipsis must be exactly 100 chars.
        let after = out.split("arguments=").nth(1).unwrap();
        let args = after.split('…').next().unwrap();
        assert_eq!(args.chars().count(), 100);
    }
}
