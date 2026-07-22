use aion_types::{message::ContentBlock, skill_types::ContextModifier};

pub(crate) const DEFAULT_MAX_TOOL_CALL_MALFORMED: usize = 3;
pub(crate) const DEFAULT_MAX_TOOL_CALL_FAILURE: usize = 3;
pub(crate) const DEFAULT_MAX_ALL_ERROR_TOOL_ROUNDS: usize = 8;
pub(crate) const DEFAULT_MAX_TOOL_CALL_CYCLE_REPETITIONS: usize = 3;
const MAX_TOOL_CALL_CYCLE_PERIOD: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolCallMalformedReason {
    EmptyFunctionName,
    EmptyToolCallId,
}

impl ToolCallMalformedReason {
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
pub(crate) struct ToolCallMalformedFingerprint {
    calls: Vec<ToolCallMalformedFingerprintPart>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolCallFailureFingerprint {
    calls: Vec<ToolCallFailureFingerprintPart>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolCallMalformedFingerprintPart {
    reason: ToolCallMalformedReason,
    id: String,
    name: String,
    input: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolCallFailureFingerprintPart {
    name: String,
    input: String,
}

#[derive(Debug)]
pub(crate) struct ToolCallMalformedTracker {
    last: Option<ToolCallMalformedFingerprint>,
    count: usize,
    limit: usize,
}

impl ToolCallMalformedTracker {
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

    pub(crate) fn observe(&mut self, current: Option<ToolCallMalformedFingerprint>) -> usize {
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

pub(crate) fn tool_call_malformed_reason(id: &str, name: &str) -> Option<ToolCallMalformedReason> {
    if name.trim().is_empty() {
        Some(ToolCallMalformedReason::EmptyFunctionName)
    } else if id.trim().is_empty() {
        Some(ToolCallMalformedReason::EmptyToolCallId)
    } else {
        None
    }
}

pub(crate) fn tool_call_malformed_fingerprint(
    tool_calls: &[ContentBlock],
    tool_call_malformed_reasons: &[Option<ToolCallMalformedReason>],
) -> Option<ToolCallMalformedFingerprint> {
    if tool_calls.is_empty() || tool_call_malformed_reasons.iter().any(|reason| reason.is_none()) {
        return None;
    }

    let calls = tool_calls
        .iter()
        .zip(tool_call_malformed_reasons)
        .filter_map(|(block, reason)| {
            let ContentBlock::ToolUse { id, name, input, .. } = block else {
                return None;
            };
            Some(ToolCallMalformedFingerprintPart {
                reason: (*reason)?,
                id: id.trim().to_string(),
                name: name.trim().to_string(),
                input: serde_json::to_string(input).unwrap_or_default(),
            })
        })
        .collect();

    Some(ToolCallMalformedFingerprint { calls })
}

pub(crate) fn tool_call_failure_fingerprint(tool_calls: &[ContentBlock]) -> Option<ToolCallFailureFingerprint> {
    if tool_calls.is_empty() {
        return None;
    }

    let calls: Option<Vec<_>> = tool_calls
        .iter()
        .map(|block| {
            let ContentBlock::ToolUse { name, input, .. } = block else {
                return None;
            };
            Some(ToolCallFailureFingerprintPart {
                name: name.trim().to_string(),
                input: serde_json::to_string(input).unwrap_or_default(),
            })
        })
        .collect();

    Some(ToolCallFailureFingerprint { calls: calls? })
}

/// Interleave synthetic malformed-call results with executed-tool results back
/// into the original `tool_calls` order.
///
/// `tool_call_malformed_reasons[i]` is `Some` when call `i` was malformed (and gets a
/// synthetic error result). `policy_denied_tool_names[i]` is `Some` when the
/// runtime policy rejected an otherwise valid call. Remaining calls consume
/// the next result/modifier from the `executable_*` iterators. Kept as a free
/// function so the interleaving invariant can be unit-tested in isolation.
pub(crate) fn merge_tool_results(
    tool_calls: &[ContentBlock],
    tool_call_malformed_reasons: &[Option<ToolCallMalformedReason>],
    policy_denied_tool_names: &[Option<String>],
    executable_results: Vec<ContentBlock>,
    executable_modifiers: Vec<Option<ContextModifier>>,
) -> (Vec<ContentBlock>, Vec<Option<ContextModifier>>) {
    let mut executable_results = executable_results.into_iter();
    let mut executable_modifiers = executable_modifiers.into_iter();
    let mut tool_results = Vec::with_capacity(tool_calls.len());
    let mut tool_modifiers = Vec::with_capacity(tool_calls.len());

    for ((call, reason), denied_name) in tool_calls
        .iter()
        .zip(tool_call_malformed_reasons)
        .zip(policy_denied_tool_names)
    {
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
        } else if let Some(denied_name) = denied_name {
            let ContentBlock::ToolUse { id, .. } = call else {
                continue;
            };
            tracing::warn!(
                target: "aion_agent",
                event = "agent.tool_policy.denied",
                tool_call_id = %id,
                tool = %denied_name,
                "rejected tool call by runtime policy"
            );

            tool_results.push(ContentBlock::ToolResult {
                tool_use_id: id.clone(),
                content: format!(
                    "Tool '{denied_name}' is not available in this runtime. Use an available tool or answer in text."
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

pub(crate) struct ToolCallFailureTracker {
    last: Option<ToolCallFailureFingerprint>,
    count: usize,
    limit: usize,
}

impl ToolCallFailureTracker {
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

    pub(crate) fn observe(&mut self, current: Option<ToolCallFailureFingerprint>) -> usize {
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

    #[cfg(test)]
    pub(crate) fn count(&self) -> usize {
        self.count
    }
}

#[derive(Debug)]
pub(crate) struct ToolCallAllErrorRoundTracker {
    count: usize,
    limit: usize,
}

impl ToolCallAllErrorRoundTracker {
    pub(crate) fn new(limit: usize) -> Self {
        Self { count: 0, limit }
    }

    pub(crate) fn observe(&mut self, all_error: bool) -> usize {
        if all_error {
            self.count += 1;
        } else {
            self.count = 0;
        }
        self.count
    }

    pub(crate) fn is_limit_exceeded(&self) -> bool {
        self.limit > 0 && self.count >= self.limit
    }

    pub(crate) fn limit(&self) -> usize {
        self.limit
    }

    #[cfg(test)]
    pub(crate) fn count(&self) -> usize {
        self.count
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ToolCallCycle {
    pub(crate) period: usize,
    pub(crate) repetitions: usize,
}

#[derive(Debug)]
pub(crate) struct ToolCallCycleTracker {
    enabled: bool,
    history: Vec<ToolCallFailureFingerprint>,
}

impl ToolCallCycleTracker {
    pub(crate) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            history: Vec::new(),
        }
    }

    pub(crate) fn observe(&mut self, current: Option<ToolCallFailureFingerprint>) -> Option<ToolCallCycle> {
        if !self.enabled {
            self.history.clear();
            return None;
        }

        let Some(current) = current else {
            self.history.clear();
            return None;
        };

        self.history.push(current);
        let max_history = MAX_TOOL_CALL_CYCLE_PERIOD * DEFAULT_MAX_TOOL_CALL_CYCLE_REPETITIONS;
        if self.history.len() > max_history {
            self.history.remove(0);
        }

        self.detect_cycle()
    }

    fn detect_cycle(&self) -> Option<ToolCallCycle> {
        let history_len = self.history.len();
        let max_period = MAX_TOOL_CALL_CYCLE_PERIOD.min(history_len / 2);
        let mut best: Option<ToolCallCycle> = None;

        for period in 2..=max_period {
            let pattern_start = history_len - period;
            let pattern = &self.history[pattern_start..];
            let mut repetitions = 1;
            let mut previous_end = pattern_start;

            while previous_end >= period && self.history[previous_end - period..previous_end] == *pattern {
                repetitions += 1;
                previous_end -= period;
            }

            if repetitions < 2 {
                continue;
            }

            let cycle = ToolCallCycle { period, repetitions };
            if best.is_none_or(|current| {
                cycle.repetitions > current.repetitions
                    || (cycle.repetitions == current.repetitions && cycle.period < current.period)
            }) {
                best = Some(cycle);
            }
        }

        best
    }
}

#[cfg(test)]
#[path = "tool_call_test.rs"]
mod tool_call_test;
