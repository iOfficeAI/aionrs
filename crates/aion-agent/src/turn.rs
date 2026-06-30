use crate::error::AgentError;
use crate::stream::StreamOutcome;
use crate::tool_call::{ToolCallFailureTracker, ToolCallMalformedFingerprint, ToolCallMalformedTracker};
use aion_types::message::StopReason;

pub(crate) enum TurnOutcome {
    ToolRound(StreamOutcome),
    Final(StreamOutcome),
    Truncated(StreamOutcome),
    EmptyFinal(StreamOutcome),
}

impl TurnOutcome {
    pub(crate) fn from_stream(outcome: StreamOutcome) -> Self {
        if !outcome.tool_calls.is_empty() {
            return Self::ToolRound(outcome);
        }

        match outcome.stop_reason {
            StopReason::EndTurn if !outcome.assistant_text.trim().is_empty() => Self::Final(outcome),
            StopReason::MaxTokens => Self::Truncated(outcome),
            _ => Self::EmptyFinal(outcome),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FinalizationReason {
    TurnBudget,
    MaxTokens,
    EmptyFinal,
}

impl FinalizationReason {
    pub(crate) fn fallback_prompt(self) -> &'static str {
        match self {
            FinalizationReason::TurnBudget => {
                "Stopped after reaching the turn budget before the model produced a final answer."
            }
            FinalizationReason::MaxTokens => {
                "The response was cut off by the token limit and could not be completed automatically."
            }
            FinalizationReason::EmptyFinal => "The model finished without visible answer text after one retry.",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TurnKind {
    Normal,
    Finalization(FinalizationReason),
}

impl TurnKind {
    pub(crate) fn disable_tools(self) -> bool {
        matches!(self, Self::Finalization(_))
    }

    pub(crate) fn control_prompt(self) -> Option<&'static str> {
        match self {
            Self::Normal => None,
            Self::Finalization(FinalizationReason::TurnBudget) => {
                Some("Do not call any more tools. Use the tool results already provided and give the final answer now.")
            }
            Self::Finalization(FinalizationReason::MaxTokens) => Some(
                "The previous response was cut off by the token limit. Finish the answer now. Do not call any tools.",
            ),
            Self::Finalization(FinalizationReason::EmptyFinal) => Some(
                "The previous assistant response finished without visible answer text. Provide a concise visible answer now. Do not send reasoning only. Do not call any tools.",
            ),
        }
    }
}

#[derive(Debug)]
pub(crate) struct TurnTracker {
    count: usize,
    limit: Option<usize>,
}

impl TurnTracker {
    pub(crate) fn new(limit: Option<usize>) -> Self {
        Self { count: 0, limit }
    }

    pub(crate) fn count(&self) -> usize {
        self.count
    }

    pub(crate) fn observe(&mut self) -> usize {
        self.count += 1;
        self.count
    }

    pub(crate) fn limit_reached(&self) -> Option<usize> {
        self.limit.filter(|&limit| self.count >= limit)
    }
}

/// Per-`run` loop-termination bookkeeping: the turn counter plus the
/// tool-call-malformed and tool-call-failure breakers. Keeps the counters and
/// their thresholds out of the loop body so the main loop has a single stop
/// decision: [`TurnGuards::after_tool_round`].
pub(crate) struct TurnGuards {
    /// Number of counted normal model turns so far.
    turns: TurnTracker,
    tool_call_malformed: ToolCallMalformedTracker,
    tool_call_failures: ToolCallFailureTracker,
}

pub(crate) enum TurnGuardAction {
    Continue,
    Finalize,
    Stop(AgentError),
}

impl TurnGuards {
    pub(crate) fn new(
        max_turns_per_run: Option<usize>,
        max_tool_call_malformed_turns: usize,
        max_tool_call_failure_turns: usize,
    ) -> Self {
        Self {
            turns: TurnTracker::new(max_turns_per_run),
            tool_call_malformed: ToolCallMalformedTracker::new(max_tool_call_malformed_turns),
            tool_call_failures: ToolCallFailureTracker::new(max_tool_call_failure_turns),
        }
    }

    pub(crate) fn counted_turns(&self) -> usize {
        self.turns.count()
    }

    /// Returns the configured limit when the turn budget is exhausted, else `None`.
    pub(crate) fn turn_budget_reached(&self) -> Option<usize> {
        self.turns.limit_reached()
    }

    pub(crate) fn record_counted_turn(&mut self) {
        self.turns.observe();
    }

    /// Fold one tool round into the breakers and return the loop action. Must
    /// be called once per tool round, after the results are recorded.
    pub(crate) fn after_tool_round(
        &mut self,
        tool_call_malformed_fingerprint: Option<ToolCallMalformedFingerprint>,
        tool_call_failure_round: bool,
    ) -> TurnGuardAction {
        let malformed_count = self.tool_call_malformed.observe(tool_call_malformed_fingerprint);
        if self.tool_call_malformed.is_limit_exceeded() {
            tracing::warn!(
                target: "aion_agent",
                count = malformed_count,
                limit = self.tool_call_malformed.limit(),
                "stopping tool-call malformed loop"
            );
            return TurnGuardAction::Stop(AgentError::ToolCallMalformed {
                count: malformed_count,
                limit: self.tool_call_malformed.limit(),
            });
        }

        let tool_call_failure_count = self.tool_call_failures.observe(tool_call_failure_round);
        if self.tool_call_failures.is_limit_exceeded() {
            tracing::warn!(
                target: "aion_agent",
                count = tool_call_failure_count,
                limit = self.tool_call_failures.limit(),
                "stopping tool-call failure loop"
            );
            return TurnGuardAction::Stop(AgentError::ToolCallFailures {
                count: tool_call_failure_count,
                limit: self.tool_call_failures.limit(),
            });
        }

        if self.turn_budget_reached().is_some() {
            TurnGuardAction::Finalize
        } else {
            TurnGuardAction::Continue
        }
    }

    #[cfg(test)]
    pub(crate) fn tool_call_failure_count(&self) -> usize {
        self.tool_call_failures.count()
    }
}
