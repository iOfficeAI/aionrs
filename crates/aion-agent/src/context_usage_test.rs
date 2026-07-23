use super::*;

#[cfg(test)]
mod tests {
    use aion_types::message::{ContentBlock, Message, Role};
    use aion_types::tool::ToolDef;
    use serde_json::json;

    use super::*;

    #[test]
    fn context_state_serializes_stable_field_names() {
        let mut state = ContextState::default();
        state.replace_with_provider_usage(89_600);
        state.record_compact();
        state.record_microcompact();

        let value = serde_json::to_value(state).unwrap();

        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["context_usage"], 89_600);
        assert_eq!(value["source"], "local_projected");
        assert_eq!(value["compact_count"], 1);
        assert_eq!(value["microcompact_count"], 1);
        assert!(value.get("observed_context_window").is_none());
        assert!(value.get("request_shape_fingerprint").is_none());
    }

    #[test]
    fn provider_usage_replaces_local_projection() {
        let mut state = ContextState::default();
        state.add_local_estimate(500);
        state.replace_with_provider_usage(1_200);

        assert_eq!(state.context_usage, 1_200);
        assert_eq!(state.source, ContextUsageSource::ProviderExact);
    }

    #[test]
    fn breakdown_sums_to_authoritative_context_usage() {
        let state = ContextState {
            context_usage: 1_000,
            source: ContextUsageSource::ProviderExact,
            ..ContextState::default()
        };
        let prompt = PromptUsage {
            system_prompt_tokens: 100,
            memory_tokens: 50,
            skills_tokens: 25,
            memory_files: vec!["MEMORY.md".into()],
            skills: vec!["review".into()],
        };
        let tools = vec![ToolDef {
            name: "Read".into(),
            description: "Read a file".into(),
            input_schema: json!({"type": "object"}),
            deferred: false,
        }];
        let messages = vec![Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "hello".repeat(20),
            }],
        )];

        let snapshot = ContextSnapshot::build("test-model", 2_000, &state, &prompt, 0, &tools, &messages);
        let breakdown = snapshot.breakdown;
        let sum = breakdown
            .system_prompt
            .saturating_add(breakdown.memory)
            .saturating_add(breakdown.skills)
            .saturating_add(breakdown.tools)
            .saturating_add(breakdown.messages)
            .saturating_add(breakdown.unattributed);

        assert_eq!(sum, 1_000);
    }

    #[test]
    fn oversized_local_categories_are_scaled_to_provider_total() {
        let raw = [500, 500, 500, 500, 500];
        let (normalized, unattributed) = normalize_breakdown(raw, 2_500, 1_000);

        assert_eq!(normalized, [200; 5]);
        assert_eq!(unattributed, 0);
    }
}
