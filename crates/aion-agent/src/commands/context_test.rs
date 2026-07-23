use super::*;

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::context_usage::{ContextBreakdown, ContextSnapshot, ContextUsageSource};

    fn snapshot() -> ContextSnapshot {
        ContextSnapshot {
            model: "claude-opus-4-8".into(),
            context_usage: 89_600,
            context_window: 200_000,
            source: ContextUsageSource::ProviderExact,
            compact_count: 2,
            microcompact_count: 5,
            updated_at: Utc::now(),
            breakdown: ContextBreakdown {
                system_prompt: 1_600,
                memory: 16_100,
                skills: 2_000,
                tools: 5_000,
                messages: 64_900,
                unattributed: 0,
            },
            memory_files: Vec::new(),
            skills: vec!["review".into()],
            tools: Vec::new(),
            messages: Vec::new(),
        }
    }

    #[test]
    fn compact_view_contains_authoritative_usage_and_counts() {
        let output = format_snapshot(&snapshot(), false);

        assert!(output.contains("89.6k/200k tokens (44.8%)"));
        assert!(output.contains("Source: provider exact"));
        assert!(output.contains("2 compact, 5 microcompact"));
        assert!(output.contains("/context all to expand"));
    }

    #[test]
    fn expanded_view_lists_categories_and_details() {
        let output = format_snapshot(&snapshot(), true);

        assert!(output.contains("Estimated usage by category"));
        assert!(output.contains("Skills · 1"));
        assert!(output.contains("- review"));
        assert!(!output.contains("/context all to expand"));
    }
}
