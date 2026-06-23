use aion_types::{message::ContentBlock, skill_types::ContextModifier};

pub(crate) const DEFAULT_MAX_MALFORMED_TOOL_CALL_TURNS: usize = 3;
pub(crate) const DEFAULT_MAX_CONSECUTIVE_TOOL_FAILURE_ROUNDS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MalformedToolCallReason {
    EmptyFunctionName,
    EmptyToolCallId,
}

impl MalformedToolCallReason {
    fn description(self) -> &'static str {
        match self {
            Self::EmptyFunctionName => "empty function name",
            Self::EmptyToolCallId => "empty tool call id",
        }
    }

    fn reissue_field(self) -> &'static str {
        match self {
            Self::EmptyFunctionName => "function name",
            Self::EmptyToolCallId => "tool call id",
        }
    }

    pub(crate) fn log_reason(self) -> &'static str {
        match self {
            Self::EmptyFunctionName => "empty_function_name",
            Self::EmptyToolCallId => "empty_tool_call_id",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MalformedToolCallFingerprint {
    calls: Vec<MalformedToolCallFingerprintPart>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MalformedToolCallFingerprintPart {
    reason: MalformedToolCallReason,
    id: String,
    name: String,
    input: String,
}

#[derive(Debug)]
pub(crate) struct MalformedToolCallTracker {
    last: Option<MalformedToolCallFingerprint>,
    count: usize,
    limit: usize,
}

impl MalformedToolCallTracker {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            last: None,
            count: 0,
            limit,
        }
    }
    pub(crate) fn limit(&self) -> usize {
        self.limit
    }

    pub(crate) fn is_limit_exceeded(&self) -> bool {
        self.limit > 0 && self.count >= self.limit
    }

    pub(crate) fn observe(&mut self, current: Option<MalformedToolCallFingerprint>) -> usize {
        let Some(current) = current else {
            self.last = None;
            self.count = 0;
            return 0;
        };

        if self.last.as_ref() == Some(&current) {
            self.count += 1;
        } else {
            self.last = Some(current);
            self.count = 1;
        }

        self.count
    }
}

pub(crate) fn malformed_tool_call_reason(id: &str, name: &str) -> Option<MalformedToolCallReason> {
    if name.trim().is_empty() {
        Some(MalformedToolCallReason::EmptyFunctionName)
    } else if id.trim().is_empty() {
        Some(MalformedToolCallReason::EmptyToolCallId)
    } else {
        None
    }
}

pub(crate) fn malformed_only_fingerprint(
    tool_calls: &[ContentBlock],
    malformed_reasons: &[Option<MalformedToolCallReason>],
) -> Option<MalformedToolCallFingerprint> {
    if tool_calls.is_empty() || malformed_reasons.iter().any(|reason| reason.is_none()) {
        return None;
    }

    let calls = tool_calls
        .iter()
        .zip(malformed_reasons)
        .filter_map(|(block, reason)| {
            let ContentBlock::ToolUse {
                id, name, input, ..
            } = block
            else {
                return None;
            };
            Some(MalformedToolCallFingerprintPart {
                reason: (*reason)?,
                id: id.trim().to_string(),
                name: name.trim().to_string(),
                input: serde_json::to_string(input).unwrap_or_default(),
            })
        })
        .collect();

    Some(MalformedToolCallFingerprint { calls })
}

/// Interleave synthetic malformed-call results with executed-tool results back
/// into the original `tool_calls` order.
///
/// `malformed_reasons[i]` is `Some` when call `i` was malformed (and gets a
/// synthetic error result), otherwise the next executed result/modifier is
/// pulled from the `executable_*` iterators. Kept as a free function so the
/// interleaving invariant can be unit-tested in isolation.
pub(crate) fn merge_tool_results(
    tool_calls: &[ContentBlock],
    malformed_reasons: &[Option<MalformedToolCallReason>],
    executable_results: Vec<ContentBlock>,
    executable_modifiers: Vec<Option<ContextModifier>>,
) -> (Vec<ContentBlock>, Vec<Option<ContextModifier>>) {
    let mut executable_results = executable_results.into_iter();
    let mut executable_modifiers = executable_modifiers.into_iter();
    let mut tool_results = Vec::with_capacity(tool_calls.len());
    let mut tool_modifiers = Vec::with_capacity(tool_calls.len());

    for (call, reason) in tool_calls.iter().zip(malformed_reasons) {
        if let Some(reason) = reason {
            let ContentBlock::ToolUse { id, name, .. } = call else {
                continue;
            };
            tracing::warn!(
                target: "aion_agent",
                tool_call_id = %id,
                tool = %name,
                reason = reason.log_reason(),
                "generated synthetic error result for malformed tool call"
            );

            tool_results.push(ContentBlock::ToolResult {
                tool_use_id: id.clone(),
                content: format!(
                    "Malformed tool call: {}. Re-issue the tool call with a non-empty {} if still needed, or answer in text.",
                    reason.description(),
                    reason.reissue_field()
                ),
                is_error: true,
            });
            tool_modifiers.push(None);
        } else {
            tool_results.push(
                executable_results
                    .next()
                    .expect("tool execution result missing for executable tool call"),
            );
            tool_modifiers.push(
                executable_modifiers
                    .next()
                    .expect("tool execution modifier missing for executable tool call"),
            );
        }
    }

    (tool_results, tool_modifiers)
}

pub(crate) struct ToolFailureTracker {
    count: usize,
    limit: usize,
}

impl ToolFailureTracker {
    pub(crate) fn new(limit: usize) -> Self {
        Self { count: 0, limit }
    }

    pub(crate) fn limit(&self) -> usize {
        self.limit
    }

    pub(crate) fn is_limit_exceeded(&self) -> bool {
        self.limit > 0 && self.count >= self.limit
    }

    pub(crate) fn observe(&mut self, failed_round: bool) -> usize {
        if failed_round {
            self.count += 1;
        } else {
            self.count = 0;
        }

        self.count
    }

    #[cfg(test)]
    pub(crate) fn count(&self) -> usize {
        self.count
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn reason_detects_blank_name_before_blank_id() {
        assert_eq!(
            malformed_tool_call_reason("", ""),
            Some(MalformedToolCallReason::EmptyFunctionName)
        );
        assert_eq!(
            malformed_tool_call_reason("call_1", "   "),
            Some(MalformedToolCallReason::EmptyFunctionName)
        );
    }

    #[test]
    fn reason_detects_blank_id() {
        assert_eq!(
            malformed_tool_call_reason(" ", "Read"),
            Some(MalformedToolCallReason::EmptyToolCallId)
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
        let fingerprint = malformed_only_fingerprint(
            &[call],
            &[Some(MalformedToolCallReason::EmptyFunctionName)],
        );
        let mut tracker = MalformedToolCallTracker::new(3);

        assert_eq!(tracker.observe(fingerprint.clone()), 1);
        assert_eq!(tracker.observe(fingerprint), 2);
        assert_eq!(tracker.observe(None), 0);
    }

    #[test]
    fn malformed_tracker_limit_zero_disables_breaker() {
        let call = ContentBlock::ToolUse {
            id: "bad".into(),
            name: "".into(),
            input: json!({}),
            extra: None,
        };
        let fingerprint = malformed_only_fingerprint(
            &[call],
            &[Some(MalformedToolCallReason::EmptyFunctionName)],
        );
        let mut tracker = MalformedToolCallTracker::new(0);

        assert_eq!(tracker.observe(fingerprint.clone()), 1);
        assert!(!tracker.is_limit_exceeded());
        assert_eq!(tracker.observe(fingerprint), 2);
        assert!(!tracker.is_limit_exceeded());
    }

    #[test]
    fn tool_failure_tracker_counts_consecutive_failed_rounds() {
        let mut tracker = ToolFailureTracker::new(3);

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
    fn tool_failure_tracker_limit_zero_disables_breaker() {
        let mut tracker = ToolFailureTracker::new(0);

        assert_eq!(tracker.observe(true), 1);
        assert!(!tracker.is_limit_exceeded());
        assert_eq!(tracker.observe(true), 2);
        assert!(!tracker.is_limit_exceeded());
    }
}
