use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use aion_types::message::{ContentBlock, Message, Role};
use aion_types::tool::ToolDef;

use crate::compact::estimate::{estimate_tokens_from_tool_image, estimate_tokens_from_tool_result};

const CONTEXT_STATE_SCHEMA_VERSION: u32 = 1;
const CHARS_PER_TOKEN: usize = 4;

/// Origin of the persisted context-usage value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextUsageSource {
    /// Exact input plus output usage reported by the latest provider turn.
    ProviderExact,
    /// Provider usage adjusted or reconstructed with local estimates.
    #[default]
    LocalProjected,
}

/// Session-scoped context accounting persisted alongside conversation history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextState {
    /// Version of this persisted object, independent from the enclosing session format.
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// Best-known token occupancy for the next provider request.
    #[serde(default)]
    pub context_usage: u64,
    /// Whether `context_usage` is provider-reported or locally projected.
    #[serde(default)]
    pub source: ContextUsageSource,
    /// Number of successful full compactions in this session.
    #[serde(default)]
    pub compact_count: u64,
    /// Number of microcompact passes that actually cleared content.
    #[serde(default)]
    pub microcompact_count: u64,
    /// Last time any context accounting field changed.
    #[serde(default = "default_updated_at")]
    pub updated_at: DateTime<Utc>,
}

impl ContextState {
    pub(crate) fn replace_with_provider_usage(&mut self, context_usage: u64) {
        self.context_usage = context_usage;
        self.source = ContextUsageSource::ProviderExact;
        self.touch();
    }

    pub(crate) fn replace_with_local_estimate(&mut self, context_usage: u64) {
        self.context_usage = context_usage;
        self.source = ContextUsageSource::LocalProjected;
        self.touch();
    }

    pub(crate) fn add_local_estimate(&mut self, tokens: u64) {
        self.context_usage = self.context_usage.saturating_add(tokens);
        self.source = ContextUsageSource::LocalProjected;
        self.touch();
    }

    pub(crate) fn record_compact(&mut self) {
        self.compact_count = self.compact_count.saturating_add(1);
        self.touch();
    }

    pub(crate) fn record_microcompact(&mut self, estimated_tokens_freed: u64) {
        self.context_usage = self.context_usage.saturating_sub(estimated_tokens_freed);
        self.source = ContextUsageSource::LocalProjected;
        self.microcompact_count = self.microcompact_count.saturating_add(1);
        self.touch();
    }

    fn touch(&mut self) {
        self.updated_at = Utc::now();
    }
}

impl Default for ContextState {
    fn default() -> Self {
        Self {
            schema_version: CONTEXT_STATE_SCHEMA_VERSION,
            context_usage: 0,
            source: ContextUsageSource::LocalProjected,
            compact_count: 0,
            microcompact_count: 0,
            updated_at: Utc::now(),
        }
    }
}

/// Runtime context status exposed to SDK hosts without persisting model limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContextStatus {
    /// Version of the context-accounting contract.
    pub schema_version: u32,
    /// Active provider model identifier.
    pub model: String,
    /// Effective runtime context window resolved by aionrs configuration.
    pub context_window: u64,
    /// Best-known token occupancy for the next provider request.
    pub context_usage: u64,
    /// Whether `context_usage` is provider-reported or locally projected.
    pub source: ContextUsageSource,
    /// Number of successful full compactions in this session.
    pub compact_count: u64,
    /// Number of microcompact passes that actually cleared content.
    pub microcompact_count: u64,
    /// Last time any context accounting field changed.
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct PromptUsage {
    pub(crate) system_prompt_tokens: u64,
    pub(crate) memory_tokens: u64,
    pub(crate) skills_tokens: u64,
    pub(crate) memory_files: Vec<String>,
    pub(crate) skills: Vec<String>,
}

impl PromptUsage {
    pub(crate) fn from_system_prompt(system_prompt: &str) -> Self {
        Self {
            system_prompt_tokens: estimate_text_tokens(system_prompt),
            ..Self::default()
        }
    }

    pub(crate) fn from_sections(
        system_prompt: &str,
        memory_prompt: Option<&str>,
        skills_prompt: Option<&str>,
        memory_files: Vec<String>,
        skills: Vec<String>,
    ) -> Self {
        let total_tokens = estimate_text_tokens(system_prompt);
        let memory_tokens = memory_prompt.map(estimate_text_tokens).unwrap_or(0);
        let skills_tokens = skills_prompt.map(estimate_text_tokens).unwrap_or(0);

        Self {
            system_prompt_tokens: total_tokens.saturating_sub(memory_tokens.saturating_add(skills_tokens)),
            memory_tokens,
            skills_tokens,
            memory_files,
            skills,
        }
    }

    pub(crate) fn total_tokens(&self) -> u64 {
        self.system_prompt_tokens
            .saturating_add(self.memory_tokens)
            .saturating_add(self.skills_tokens)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NamedUsage {
    pub(crate) name: String,
    pub(crate) tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MessageUsage {
    pub(crate) index: usize,
    pub(crate) role: Role,
    pub(crate) tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContextBreakdown {
    pub(crate) system_prompt: u64,
    pub(crate) memory: u64,
    pub(crate) skills: u64,
    pub(crate) tools: u64,
    pub(crate) messages: u64,
    pub(crate) unattributed: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct ContextSnapshot {
    pub(crate) model: String,
    pub(crate) context_usage: u64,
    pub(crate) context_window: u64,
    pub(crate) source: ContextUsageSource,
    pub(crate) compact_count: u64,
    pub(crate) microcompact_count: u64,
    pub(crate) updated_at: DateTime<Utc>,
    pub(crate) breakdown: ContextBreakdown,
    pub(crate) memory_files: Vec<NamedUsage>,
    pub(crate) skills: Vec<String>,
    pub(crate) tools: Vec<NamedUsage>,
    pub(crate) messages: Vec<MessageUsage>,
}

impl ContextSnapshot {
    pub(crate) fn build(
        model: &str,
        context_window: usize,
        state: &ContextState,
        prompt: &PromptUsage,
        dynamic_system_tokens: u64,
        tools: &[ToolDef],
        messages: &[Message],
    ) -> Self {
        let tool_usage = tools
            .iter()
            .map(|tool| NamedUsage {
                name: tool.name.clone(),
                tokens: estimate_tool_tokens(tool),
            })
            .collect::<Vec<_>>();
        let message_usage = messages
            .iter()
            .enumerate()
            .map(|(index, message)| MessageUsage {
                index,
                role: message.role,
                tokens: estimate_message_tokens(message),
            })
            .collect::<Vec<_>>();

        let raw = [
            prompt.system_prompt_tokens.saturating_add(dynamic_system_tokens),
            prompt.memory_tokens,
            prompt.skills_tokens,
            tool_usage.iter().map(|item| item.tokens).sum(),
            message_usage.iter().map(|item| item.tokens).sum(),
        ];
        let raw_total = raw.iter().copied().fold(0_u64, u64::saturating_add);
        let context_usage = if state.context_usage == 0 {
            raw_total
        } else {
            state.context_usage
        };
        let (normalized, unattributed) = normalize_breakdown(raw, raw_total, context_usage);

        let memory_files = prompt
            .memory_files
            .iter()
            .cloned()
            .map(|name| NamedUsage {
                name,
                tokens: prompt.memory_tokens,
            })
            .collect();

        Self {
            model: model.to_string(),
            context_usage,
            context_window: context_window as u64,
            source: state.source,
            compact_count: state.compact_count,
            microcompact_count: state.microcompact_count,
            updated_at: state.updated_at,
            breakdown: ContextBreakdown {
                system_prompt: normalized[0],
                memory: normalized[1],
                skills: normalized[2],
                tools: normalized[3],
                messages: normalized[4],
                unattributed,
            },
            memory_files,
            skills: prompt.skills.clone(),
            tools: tool_usage,
            messages: message_usage,
        }
    }
}

pub(crate) fn estimate_text_tokens(text: &str) -> u64 {
    if text.is_empty() {
        return 0;
    }
    text.len().div_ceil(CHARS_PER_TOKEN) as u64
}

pub(crate) fn estimate_content_tokens(content: &[ContentBlock]) -> u64 {
    content.iter().fold(0_u64, |total, block| {
        let tokens = match block {
            ContentBlock::Text { text } => estimate_text_tokens(text),
            ContentBlock::Image { .. } => estimate_tokens_from_tool_image(block),
            ContentBlock::ToolUse { id, name, input, extra } => {
                let extra_len = extra.as_ref().map_or(0, |value| value.to_string().len());
                estimate_chars(id.len() + name.len() + input.to_string().len() + extra_len)
            }
            ContentBlock::ToolResult { .. } => estimate_tokens_from_tool_result(block),
            ContentBlock::Thinking { thinking, signature } => {
                estimate_chars(thinking.len() + signature.as_deref().map_or(0, str::len))
            }
            ContentBlock::ProviderItem { provider, item } => estimate_chars(provider.len() + item.to_string().len()),
        };
        total.saturating_add(tokens)
    })
}

pub(crate) fn estimate_messages_tokens(messages: &[Message]) -> u64 {
    messages
        .iter()
        .map(estimate_message_tokens)
        .fold(0_u64, u64::saturating_add)
}

pub(crate) fn estimate_tool_definitions_tokens(tools: &[ToolDef]) -> u64 {
    tools.iter().map(estimate_tool_tokens).fold(0_u64, u64::saturating_add)
}

fn estimate_message_tokens(message: &Message) -> u64 {
    estimate_content_tokens(&message.content)
}

fn estimate_tool_tokens(tool: &ToolDef) -> u64 {
    estimate_chars(tool.name.len() + tool.description.len() + tool.input_schema.to_string().len())
}

fn estimate_chars(chars: usize) -> u64 {
    if chars == 0 {
        0
    } else {
        chars.div_ceil(CHARS_PER_TOKEN) as u64
    }
}

fn normalize_breakdown(raw: [u64; 5], raw_total: u64, context_usage: u64) -> ([u64; 5], u64) {
    if raw_total == 0 {
        return ([0; 5], context_usage);
    }
    if raw_total <= context_usage {
        return (raw, context_usage - raw_total);
    }

    let mut normalized = [0_u64; 5];
    for (index, value) in raw.into_iter().enumerate() {
        normalized[index] = ((value as u128 * context_usage as u128) / raw_total as u128) as u64;
    }
    let normalized_total = normalized.iter().copied().fold(0_u64, u64::saturating_add);
    (normalized, context_usage.saturating_sub(normalized_total))
}

fn default_schema_version() -> u32 {
    CONTEXT_STATE_SCHEMA_VERSION
}

fn default_updated_at() -> DateTime<Utc> {
    Utc::now()
}

#[cfg(test)]
#[path = "context_usage_test.rs"]
mod context_usage_test;
