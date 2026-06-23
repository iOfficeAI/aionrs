use crate::error::AgentError;
use crate::stream::StreamOutcome;
use crate::tool_call::{
    MalformedToolCallFingerprint, MalformedToolCallTracker, ToolFailureTracker,
};
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
            StopReason::EndTurn if !outcome.assistant_text.trim().is_empty() => {
                Self::Final(outcome)
            }
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
            Self::Finalization(FinalizationReason::TurnBudget) => Some(
                "Do not call any more tools. Use the tool results already provided and give the final answer now.",
            ),
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

/// Per-`run` loop-termination bookkeeping: the turn counter and the two
/// consecutive-failure breakers (malformed calls, tool errors). Keeps the
/// counters and their thresholds out of the loop body so the main loop has a
/// single stop decision: [`TurnGuards::after_tool_round`].
pub(crate) struct TurnGuards {
    /// Number of counted normal model turns so far.
    turns: TurnTracker,
    malformed_tool_calls: MalformedToolCallTracker,
    tool_failures: ToolFailureTracker,
}

pub(crate) enum TurnGuardAction {
    Continue,
    Finalize,
    Stop(AgentError),
}

impl TurnGuards {
    pub(crate) fn new(
        max_turns_per_run: Option<usize>,
        max_malformed_tool_calls: usize,
        max_tool_failure_rounds: usize,
    ) -> Self {
        Self {
            turns: TurnTracker::new(max_turns_per_run),
            malformed_tool_calls: MalformedToolCallTracker::new(max_malformed_tool_calls),
            tool_failures: ToolFailureTracker::new(max_tool_failure_rounds),
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
        malformed_only_fingerprint: Option<MalformedToolCallFingerprint>,
        executable_tool_error_round: bool,
    ) -> TurnGuardAction {
        let malformed_count = self
            .malformed_tool_calls
            .observe(malformed_only_fingerprint);
        if self.malformed_tool_calls.is_limit_exceeded() {
            tracing::warn!(
                target: "aion_agent",
                count = malformed_count,
                limit = self.malformed_tool_calls.limit(),
                "stopping malformed tool call loop"
            );
            return TurnGuardAction::Stop(AgentError::MalformedToolCall {
                count: malformed_count,
                limit: self.malformed_tool_calls.limit(),
            });
        }

        let tool_failure_count = self.tool_failures.observe(executable_tool_error_round);
        if self.tool_failures.is_limit_exceeded() {
            tracing::warn!(
                target: "aion_agent",
                count = tool_failure_count,
                limit = self.tool_failures.limit(),
                "stopping tool failure loop"
            );
            return TurnGuardAction::Stop(AgentError::ToolFailures {
                count: tool_failure_count,
                limit: self.tool_failures.limit(),
            });
        }

        if self.turn_budget_reached().is_some() {
            TurnGuardAction::Finalize
        } else {
            TurnGuardAction::Continue
        }
    }

    #[cfg(test)]
    pub(crate) fn tool_failure_count(&self) -> usize {
        self.tool_failures.count()
    }
}
