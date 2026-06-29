#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DroppedToolCallReason {
    EmptyName,
    EmptyId,
}

impl DroppedToolCallReason {
    fn description(self) -> &'static str {
        match self {
            DroppedToolCallReason::EmptyName => "empty function name",
            DroppedToolCallReason::EmptyId => "empty tool call id",
        }
    }

    fn reissue_field(self) -> &'static str {
        match self {
            DroppedToolCallReason::EmptyName => "name",
            DroppedToolCallReason::EmptyId => "id",
        }
    }

    pub(crate) fn log_reason(self) -> &'static str {
        match self {
            DroppedToolCallReason::EmptyName => "empty_name",
            DroppedToolCallReason::EmptyId => "empty_id",
        }
    }

    pub(crate) fn short_placeholder(self) -> &'static str {
        match self {
            DroppedToolCallReason::EmptyName => "[tool call skipped: malformed (empty function name).]",
            DroppedToolCallReason::EmptyId => "[tool call skipped: malformed (empty tool call id).]",
        }
    }
}

/// Format a malformed tool_call as a human/model-readable line to embed in the
/// assistant content during projection. Shared by OpenAI and Anthropic
/// projection paths so the wording stays identical across providers.
/// `arguments` is the tool input, truncated to 100 chars on a char boundary.
pub(crate) fn format_dropped_tool_call(reason: DroppedToolCallReason, input: &serde_json::Value) -> String {
    let raw = serde_json::to_string(input).unwrap_or_default();
    let args = truncate_chars(&raw, 100);
    format!(
        "[tool call skipped: malformed ({}). arguments={}. This call was not executed; re-issue with a valid {} if still needed.]",
        reason.description(),
        args,
        reason.reissue_field()
    )
}

/// Truncate to at most `max` chars on a char boundary, appending `…` if cut.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let end = s.char_indices().nth(max).map(|(i, _)| i).unwrap_or(s.len());
    format!("{}…", &s[..end])
}

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
