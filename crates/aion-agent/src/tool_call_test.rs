use super::*;

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn reason_detects_blank_name_before_blank_id() {
        assert_eq!(
            tool_call_malformed_reason("", ""),
            Some(ToolCallMalformedReason::EmptyFunctionName)
        );
        assert_eq!(
            tool_call_malformed_reason("call_1", "   "),
            Some(ToolCallMalformedReason::EmptyFunctionName)
        );
    }

    #[test]
    fn reason_detects_blank_id() {
        assert_eq!(
            tool_call_malformed_reason(" ", "Read"),
            Some(ToolCallMalformedReason::EmptyToolCallId)
        );
    }

    #[test]
    fn tracker_counts_only_same_fingerprint() {
        let call = ContentBlock::ToolUse {
            id: "bad".into(),
            name: "".into(),
            input: json!({}),
            extra: None,
        };
        let fingerprint = tool_call_malformed_fingerprint(&[call], &[Some(ToolCallMalformedReason::EmptyFunctionName)]);
        let mut tracker = ToolCallMalformedTracker::new(3);

        assert_eq!(tracker.observe(fingerprint.clone()), 1);
        assert_eq!(tracker.observe(fingerprint), 2);
        assert_eq!(tracker.observe(None), 0);
    }

    #[test]
    fn tool_call_malformed_tracker_limit_zero_disables_breaker() {
        let call = ContentBlock::ToolUse {
            id: "bad".into(),
            name: "".into(),
            input: json!({}),
            extra: None,
        };
        let fingerprint = tool_call_malformed_fingerprint(&[call], &[Some(ToolCallMalformedReason::EmptyFunctionName)]);
        let mut tracker = ToolCallMalformedTracker::new(0);

        assert_eq!(tracker.observe(fingerprint.clone()), 1);
        assert!(!tracker.is_limit_exceeded());
        assert_eq!(tracker.observe(fingerprint), 2);
        assert!(!tracker.is_limit_exceeded());
    }

    #[test]
    fn tool_call_failure_tracker_counts_only_same_fingerprint() {
        let command_a = ContentBlock::ToolUse {
            id: "call-a".into(),
            name: "ExecCommand".into(),
            input: json!({ "cmd": "python update_config.py" }),
            extra: None,
        };
        let command_a_reissued = ContentBlock::ToolUse {
            id: "call-a-reissued".into(),
            name: "ExecCommand".into(),
            input: json!({ "cmd": "python update_config.py" }),
            extra: None,
        };
        let command_b = ContentBlock::ToolUse {
            id: "call-b".into(),
            name: "ExecCommand".into(),
            input: json!({ "cmd": "aioncore assistants update" }),
            extra: None,
        };
        let command_a = tool_call_failure_fingerprint(&[command_a]);
        let command_a_reissued = tool_call_failure_fingerprint(&[command_a_reissued]);
        let command_b = tool_call_failure_fingerprint(&[command_b]);
        let mut tracker = ToolCallFailureTracker::new(3);

        assert_eq!(tracker.observe(command_a.clone()), 1);
        assert_eq!(tracker.observe(command_a_reissued), 2);
        assert_eq!(tracker.observe(command_b), 1);
        assert_eq!(tracker.count(), 1);
        assert!(!tracker.is_limit_exceeded());
        assert_eq!(tracker.observe(None), 0);
        assert_eq!(tracker.observe(command_a.clone()), 1);
        assert_eq!(tracker.observe(command_a.clone()), 2);
        assert_eq!(tracker.observe(command_a), 3);
        assert!(tracker.is_limit_exceeded());
        assert_eq!(tracker.limit(), 3);
    }

    #[test]
    fn tool_call_failure_tracker_limit_zero_disables_breaker() {
        let call = ContentBlock::ToolUse {
            id: "call-a".into(),
            name: "ExecCommand".into(),
            input: json!({ "cmd": "python update_config.py" }),
            extra: None,
        };
        let fingerprint = tool_call_failure_fingerprint(&[call]);
        let mut tracker = ToolCallFailureTracker::new(0);

        assert_eq!(tracker.observe(fingerprint.clone()), 1);
        assert!(!tracker.is_limit_exceeded());
        assert_eq!(tracker.observe(fingerprint), 2);
        assert!(!tracker.is_limit_exceeded());
    }
}
