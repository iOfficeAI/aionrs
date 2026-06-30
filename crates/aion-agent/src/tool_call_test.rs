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
    fn tool_call_failure_tracker_counts_consecutive_failed_rounds() {
        let mut tracker = ToolCallFailureTracker::new(3);

        assert_eq!(tracker.observe(true), 1);
        assert_eq!(tracker.observe(true), 2);
        assert_eq!(tracker.observe(false), 0);
        assert_eq!(tracker.observe(true), 1);
        assert_eq!(tracker.count(), 1);
        assert!(!tracker.is_limit_exceeded());
        assert_eq!(tracker.observe(true), 2);
        assert_eq!(tracker.observe(true), 3);
        assert!(tracker.is_limit_exceeded());
        assert_eq!(tracker.limit(), 3);
    }

    #[test]
    fn tool_call_failure_tracker_limit_zero_disables_breaker() {
        let mut tracker = ToolCallFailureTracker::new(0);

        assert_eq!(tracker.observe(true), 1);
        assert!(!tracker.is_limit_exceeded());
        assert_eq!(tracker.observe(true), 2);
        assert!(!tracker.is_limit_exceeded());
    }
}
