use crate::error::AgentError;
use crate::stream::StreamOutcome;
use crate::tool_call::{
    DEFAULT_MAX_ALL_ERROR_TOOL_ROUNDS, DEFAULT_MAX_TOOL_CALL_CYCLE_REPETITIONS, ToolCallAllErrorRoundTracker,
    ToolCallCycle, ToolCallCycleTracker, ToolCallFailureFingerprint, ToolCallFailureTracker,
    ToolCallMalformedFingerprint, ToolCallMalformedTracker,
};
use aion_types::message::StopReason;

const EXACT_FAILURE_WARNING_COUNT: usize = 2;
const ALL_ERROR_ROUND_WARNING_COUNT: usize = 3;
const TOOL_CALL_CYCLE_WARNING_REPETITIONS: usize = 2;

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
    ToolFailure,
    MaxTokens,
    EmptyFinal,
}

impl FinalizationReason {
    pub(crate) fn fallback_prompt(self) -> &'static str {
        match self {
            FinalizationReason::TurnBudget => {
                "Stopped after reaching the turn budget before the model produced a final answer."
            }
            FinalizationReason::ToolFailure => {
                "Tool execution repeatedly failed without making progress. Review the latest tool errors and try again after resolving the blocker."
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
            Self::Finalization(FinalizationReason::ToolFailure) => Some(
                "Tool execution repeatedly failed without making progress. Do not call any more tools. Summarize what was completed, explain the concrete blocker using the latest tool results, and state what the user should change or provide next. Do not mention internal retry counters.",
            ),
            Self::Finalization(FinalizationReason::MaxTokens) => Some(
                "The previous response was cut off by the token limit. Finish the answer now. Do not call any tools.",
            ),
            Self::Finalization(FinalizationReason::EmptyFinal) => Some(
                "The previous assistant response finished without visible answer text. Provide a concise visible answer now. Do not send reasoning only. Do not call any tools.",
            ),
        }
    }

    pub(crate) fn diagnostic_phase(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Finalization(FinalizationReason::TurnBudget) => "turn_budget_finalization",
            Self::Finalization(FinalizationReason::ToolFailure) => "tool_failure_finalization",
            Self::Finalization(FinalizationReason::MaxTokens) => "max_tokens_finalization",
            Self::Finalization(FinalizationReason::EmptyFinal) => "empty_final_retry",
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
    all_error_tool_rounds: ToolCallAllErrorRoundTracker,
    tool_call_cycles: ToolCallCycleTracker,
    tool_call_cycle_warning_emitted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolLoopWarning {
    ExactFailure {
        count: usize,
        limit: usize,
    },
    AllErrorRounds {
        count: usize,
        limit: usize,
    },
    Cycle {
        period: usize,
        repetitions: usize,
        limit: usize,
    },
}

impl ToolLoopWarning {
    pub(crate) fn guidance(self) -> String {
        match self {
            Self::ExactFailure { count, limit } => format!(
                "[Tool recovery required: the same tool call has failed {count}/{limit} times. Do not repeat it unchanged. Inspect the latest error, change arguments or strategy, use another tool, or explain the blocker in the final answer.]"
            ),
            Self::AllErrorRounds { count, limit } => format!(
                "[Tool recovery required: every tool call has failed for {count}/{limit} consecutive rounds. Stop retrying reflexively. Diagnose the latest errors, choose a materially different approach, or explain the blocker in the final answer.]"
            ),
            Self::Cycle {
                period,
                repetitions,
                limit,
            } => format!(
                "[Tool recovery required: a {period}-round tool-call cycle has repeated {repetitions}/{limit} times without progress. Break the cycle by changing strategy or explain the blocker in the final answer.]"
            ),
        }
    }
}

pub(crate) enum TurnGuardAction {
    Continue,
    Warn(ToolLoopWarning),
    Finalize(FinalizationReason),
    Stop(AgentError),
}

impl TurnGuards {
    pub(crate) fn new(
        max_turns_per_run: Option<usize>,
        max_tool_call_malformed_turns: usize,
        max_tool_call_failure_turns: usize,
    ) -> Self {
        let tool_failure_guards_enabled = max_tool_call_failure_turns > 0;
        Self {
            turns: TurnTracker::new(max_turns_per_run),
            tool_call_malformed: ToolCallMalformedTracker::new(max_tool_call_malformed_turns),
            tool_call_failures: ToolCallFailureTracker::new(max_tool_call_failure_turns),
            all_error_tool_rounds: ToolCallAllErrorRoundTracker::new(if tool_failure_guards_enabled {
                DEFAULT_MAX_ALL_ERROR_TOOL_ROUNDS
            } else {
                0
            }),
            tool_call_cycles: ToolCallCycleTracker::new(tool_failure_guards_enabled),
            tool_call_cycle_warning_emitted: false,
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
        tool_call_failure_fingerprint: Option<ToolCallFailureFingerprint>,
        all_tool_results_error: bool,
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

        let tool_call_failure_count = self.tool_call_failures.observe(tool_call_failure_fingerprint.clone());
        let all_error_round_count = self.all_error_tool_rounds.observe(all_tool_results_error);
        let cycle = self.tool_call_cycles.observe(tool_call_failure_fingerprint);
        if cycle.is_none() {
            self.tool_call_cycle_warning_emitted = false;
        }

        if self.tool_call_failures.is_limit_exceeded() {
            tracing::warn!(
                target: "aion_agent",
                count = tool_call_failure_count,
                limit = self.tool_call_failures.limit(),
                loop_kind = "exact_failure",
                "finalizing after repeated tool-call failures"
            );
            return TurnGuardAction::Finalize(FinalizationReason::ToolFailure);
        }

        if let Some(ToolCallCycle { period, repetitions }) = cycle
            && repetitions >= DEFAULT_MAX_TOOL_CALL_CYCLE_REPETITIONS
        {
            tracing::warn!(
                target: "aion_agent",
                period,
                repetitions,
                limit = DEFAULT_MAX_TOOL_CALL_CYCLE_REPETITIONS,
                loop_kind = "cycle",
                "finalizing after repeated tool-call cycle"
            );
            return TurnGuardAction::Finalize(FinalizationReason::ToolFailure);
        }

        if self.all_error_tool_rounds.is_limit_exceeded() {
            tracing::warn!(
                target: "aion_agent",
                count = all_error_round_count,
                limit = self.all_error_tool_rounds.limit(),
                loop_kind = "all_error_rounds",
                "finalizing after consecutive all-error tool rounds"
            );
            return TurnGuardAction::Finalize(FinalizationReason::ToolFailure);
        }

        if self.turn_budget_reached().is_some() {
            return TurnGuardAction::Finalize(FinalizationReason::TurnBudget);
        }

        if self.tool_call_failures.limit() > EXACT_FAILURE_WARNING_COUNT
            && tool_call_failure_count == EXACT_FAILURE_WARNING_COUNT
        {
            return TurnGuardAction::Warn(ToolLoopWarning::ExactFailure {
                count: tool_call_failure_count,
                limit: self.tool_call_failures.limit(),
            });
        }

        if let Some(ToolCallCycle { period, repetitions }) = cycle
            && repetitions == TOOL_CALL_CYCLE_WARNING_REPETITIONS
            && !self.tool_call_cycle_warning_emitted
        {
            self.tool_call_cycle_warning_emitted = true;
            return TurnGuardAction::Warn(ToolLoopWarning::Cycle {
                period,
                repetitions,
                limit: DEFAULT_MAX_TOOL_CALL_CYCLE_REPETITIONS,
            });
        }

        if self.all_error_tool_rounds.limit() > ALL_ERROR_ROUND_WARNING_COUNT
            && all_error_round_count == ALL_ERROR_ROUND_WARNING_COUNT
        {
            return TurnGuardAction::Warn(ToolLoopWarning::AllErrorRounds {
                count: all_error_round_count,
                limit: self.all_error_tool_rounds.limit(),
            });
        }

        TurnGuardAction::Continue
    }

    #[cfg(test)]
    pub(crate) fn tool_call_failure_count(&self) -> usize {
        self.tool_call_failures.count()
    }

    #[cfg(test)]
    pub(crate) fn all_error_tool_round_count(&self) -> usize {
        self.all_error_tool_rounds.count()
    }
}
