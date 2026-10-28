use serde::{Deserialize, Serialize};

/// Configuration for the multi-level context compaction system.
///
/// All token-related fields are in tokens (not bytes or characters).
/// The defaults are tuned for Claude models with a 200k context window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactConfig {
    /// Context window size in tokens (e.g. 200_000 for Claude).
    #[serde(default = "default_context_window")]
    pub context_window: usize,

    /// Tokens reserved for output generation.
    /// Subtracted from `context_window` to get the effective input budget.
    #[serde(default = "default_output_reserve")]
    pub output_reserve: usize,

    /// Buffer below the effective window that triggers autocompact.
    /// `threshold = context_window - output_reserve - autocompact_buffer`
    #[serde(default = "default_autocompact_buffer")]
    pub autocompact_buffer: usize,

    /// Tokens from context_window limit to trigger emergency block.
    /// `emergency_limit = context_window - emergency_buffer`
    #[serde(default = "default_emergency_buffer")]
    pub emergency_buffer: usize,

    /// Tokens of the most recent conversation kept verbatim after a full
    /// compaction. The summary covers everything older than this tail.
    ///
    /// `0` restores the legacy behaviour of keeping only the summary.
    #[serde(default = "default_keep_recent_tokens")]
    pub keep_recent_tokens: usize,

    /// Max consecutive autocompact failures before the circuit breaker trips.
    #[serde(default = "default_max_failures")]
    pub max_failures: u32,

    /// Maximum UTF-8 byte length of one model-facing tool result.
    ///
    /// Oversized results are truncated once, preserving their beginning and
    /// end. This keeps stored history stable for prompt caching.
    #[serde(default = "default_tool_output_max_bytes")]
    pub tool_output_max_bytes: usize,

    /// Whether to enable the legacy history-rewriting microcompact pass.
    ///
    /// Disabled by default because rewriting old tool results invalidates the
    /// stable prompt prefix. Prefer the per-result output limit instead.
    #[serde(default)]
    pub microcompact_enabled: bool,

    /// Legacy microcompact: keep the N most recent compactable tool results.
    #[serde(default = "default_micro_keep_recent")]
    pub micro_keep_recent: usize,

    /// Legacy microcompact: gap threshold in seconds for time-based trigger.
    /// When the last assistant message is older than this, microcompact fires.
    #[serde(default = "default_micro_gap_seconds")]
    pub micro_gap_seconds: u64,

    /// Tool names whose results are eligible for microcompact content clearing.
    #[serde(default = "default_compactable_tools")]
    pub compactable_tools: Vec<String>,

    /// Optional percentage override for the autocompact trigger threshold.
    /// When set, threshold = context_window * pct / 100, ignoring
    /// output_reserve and autocompact_buffer.
    #[serde(default)]
    pub autocompact_threshold_pct: Option<u8>,

    /// Whether the compaction system is enabled.
    /// When false, legacy microcompact and autocompact are skipped
    /// (emergency truncation still applies).
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Enable prompt cache diagnostics output to user.
    /// When true, cache hit/miss info is shown via OutputSink.
    /// Default: false.
    #[serde(default)]
    pub cache_diagnostics: bool,

    #[serde(default)]
    pub compaction: aion_compact::CompactLevel,

    #[serde(default)]
    pub toon: bool,
}

impl Default for CompactConfig {
    fn default() -> Self {
        Self {
            context_window: default_context_window(),
            output_reserve: default_output_reserve(),
            autocompact_buffer: default_autocompact_buffer(),
            emergency_buffer: default_emergency_buffer(),
            keep_recent_tokens: default_keep_recent_tokens(),
            max_failures: default_max_failures(),
            tool_output_max_bytes: default_tool_output_max_bytes(),
            microcompact_enabled: false,
            micro_keep_recent: default_micro_keep_recent(),
            micro_gap_seconds: default_micro_gap_seconds(),
            compactable_tools: default_compactable_tools(),
            autocompact_threshold_pct: None,
            enabled: default_true(),
            cache_diagnostics: false,
            compaction: aion_compact::CompactLevel::default(),
            toon: false,
        }
    }
}

impl CompactConfig {
    /// Shrink reserves that would not fit the active context window.
    ///
    /// The defaults target a 200k window. When a smaller model window applies
    /// (e.g. a 32k local model), the fixed reserves can exceed the window and
    /// saturate the autocompact threshold to zero. Clamp each reserve to a
    /// fraction of the window so a positive threshold remains. This is a no-op
    /// for the default 200k configuration.
    pub fn clamp_reserves(&mut self) {
        let window = self.context_window;
        self.output_reserve = self.output_reserve.min(window / 4);
        self.autocompact_buffer = self.autocompact_buffer.min(window / 10);
        self.emergency_buffer = self.emergency_buffer.min(window / 40);
    }
}

// --- Default value functions ---

fn default_context_window() -> usize {
    200_000
}
fn default_output_reserve() -> usize {
    20_000
}
fn default_autocompact_buffer() -> usize {
    13_000
}
fn default_emergency_buffer() -> usize {
    3_000
}
fn default_keep_recent_tokens() -> usize {
    20_000
}
fn default_max_failures() -> u32 {
    3
}
fn default_tool_output_max_bytes() -> usize {
    10_000
}
fn default_micro_keep_recent() -> usize {
    5
}
fn default_micro_gap_seconds() -> u64 {
    3600
}
fn default_compactable_tools() -> Vec<String> {
    vec![
        "Read".into(),
        "ExecCommand".into(),
        "Grep".into(),
        "Glob".into(),
        "Write".into(),
        "Edit".into(),
    ]
}
fn default_true() -> bool {
    true
}

#[cfg(test)]
#[path = "compact_test.rs"]
mod compact_test;
