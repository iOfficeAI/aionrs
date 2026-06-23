use aion_types::message::ContentBlock;

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

#[derive(Debug, Default)]
pub(crate) struct MalformedToolCallTracker {
    last: Option<MalformedToolCallFingerprint>,
    count: usize,
}

impl MalformedToolCallTracker {
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

#[derive(Debug, Default)]
pub(crate) struct ToolFailureTracker {
    count: usize,
}

impl ToolFailureTracker {
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

pub(crate) fn reason(id: &str, name: &str) -> Option<MalformedToolCallReason> {
    if name.trim().is_empty() {
        Some(MalformedToolCallReason::EmptyFunctionName)
    } else if id.trim().is_empty() {
        Some(MalformedToolCallReason::EmptyToolCallId)
    } else {
        None
    }
}

pub(crate) fn synthetic_result(id: String, reason: MalformedToolCallReason) -> ContentBlock {
    ContentBlock::ToolResult {
        tool_use_id: id,
        content: format!(
            "Malformed tool call: {}. Re-issue the tool call with a non-empty {} if still needed, or answer in text.",
            reason.description(),
            reason.reissue_field()
        ),
        is_error: true,
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn reason_detects_blank_name_before_blank_id() {
        assert_eq!(
            reason("", ""),
            Some(MalformedToolCallReason::EmptyFunctionName)
        );
        assert_eq!(
            reason("call_1", "   "),
            Some(MalformedToolCallReason::EmptyFunctionName)
        );
    }

    #[test]
    fn reason_detects_blank_id() {
        assert_eq!(
            reason(" ", "Read"),
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
        let mut tracker = MalformedToolCallTracker::default();

        assert_eq!(tracker.observe(fingerprint.clone()), 1);
        assert_eq!(tracker.observe(fingerprint), 2);
        assert_eq!(tracker.observe(None), 0);
    }

    #[test]
    fn tool_failure_tracker_counts_consecutive_failed_rounds() {
        let mut tracker = ToolFailureTracker::default();

        assert_eq!(tracker.observe(true), 1);
        assert_eq!(tracker.observe(true), 2);
        assert_eq!(tracker.observe(false), 0);
        assert_eq!(tracker.observe(true), 1);
        assert_eq!(tracker.count(), 1);
    }
}
