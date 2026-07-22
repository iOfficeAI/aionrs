use super::{
    AgentEngine, AgentError, CacheBreakDetector, CompactLevel, CompactState, ProviderCompat, build_assistant_content,
    merge_tool_results, tool_call_malformed_fingerprint,
};

// ---------------------------------------------------------------------------
// set_config tests — apply_config_update()
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests_set_config {
    use std::sync::{Arc, Mutex};

    use aion_config::compat::ReasoningCompat;
    use aion_providers::error::ProviderError;
    use aion_providers::provider::LlmProvider;
    use aion_tools::registry::ToolRegistry;
    use aion_types::llm::{LlmEvent, LlmRequest};
    use aion_types::message::ImageInputCapability;

    use super::{CompactLevel, ProviderCompat};
    use crate::confirm::ToolConfirmer;
    use crate::output::OutputSink;

    struct NullOutput;
    impl OutputSink for NullOutput {
        fn emit_text_delta(&self, _: &str, _: &str) {}
        fn emit_thinking(&self, _: &str, _: &str) {}
        fn emit_tool_call(&self, _: &str, _: &str, _: &str) {}
        fn emit_tool_result(&self, _: &str, _: &str, _: bool, _: &str) {}
        fn emit_stream_start(&self, _: &str) {}
        fn emit_stream_end(&self, _: &str, _: usize, _: u64, _: u64, _: u64, _: u64) {}
        fn emit_error(&self, _: &str) {}
        fn emit_info(&self, _: &str) {}
    }

    struct NullProvider;
    #[async_trait::async_trait]
    impl LlmProvider for NullProvider {
        async fn stream(&self, _: &LlmRequest) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            Ok(rx)
        }
    }

    fn make_engine(model: &str) -> super::AgentEngine {
        super::AgentEngine {
            provider: Arc::new(NullProvider),
            model: model.to_string(),
            max_tokens: Some(4096),
            thinking: None,
            compat: ProviderCompat::anthropic_defaults(),
            system_prompt: String::new(),
            reasoning_effort: None,
            messages: vec![],
            total_usage: Default::default(),
            msg_id: String::new(),
            max_turns_per_run: Some(10),
            max_tool_call_malformed_turns: 3,
            max_tool_call_failure_turns: 3,
            tools: ToolRegistry::new(),
            tool_policy: Default::default(),
            confirmer: Arc::new(Mutex::new(ToolConfirmer::new(true, vec![]))),
            allow_list: vec![],
            hooks: None,
            session_manager: None,
            current_session: None,
            output: Arc::new(NullOutput),
            approval_manager: None,
            protocol_writer: None,
            compact_config: aion_config::compact::CompactConfig::default(),
            compact_state: super::CompactState::new(),
            compact_level: CompactLevel::default(),
            toon_enabled: false,
            plan_state: Default::default(),
            plan_active_flag: None,
            cache_detector: super::CacheBreakDetector::new(),
            commands: crate::commands::default_registry(),
        }
    }

    fn make_engine_with_compat(model: &str, compat: ProviderCompat) -> super::AgentEngine {
        let mut engine = make_engine(model);
        engine.compat = compat;
        engine
    }

    // --- Cycle 1 tests (updated signature) ---

    #[test]
    fn set_config_changes_model() {
        let mut engine = make_engine("old-model");
        let changes = engine.apply_config_update(Some("new-model".into()), None, None, None, None, None);
        assert_eq!(engine.model, "new-model");
        assert_eq!(changes.len(), 1);
        assert!(changes[0].contains("old-model"));
        assert!(changes[0].contains("new-model"));
    }

    #[test]
    fn set_config_none_model_no_change() {
        let mut engine = make_engine("current");
        let changes = engine.apply_config_update(None, None, None, None, None, None);
        assert_eq!(engine.model, "current");
        assert!(changes.is_empty());
    }

    #[test]
    fn set_config_same_model_still_reports_change() {
        let mut engine = make_engine("same");
        let changes = engine.apply_config_update(Some("same".into()), None, None, None, None, None);
        assert_eq!(changes.len(), 1);
    }

    #[test]
    fn set_config_empty_string_model_accepted() {
        let mut engine = make_engine("real-model");
        engine.apply_config_update(Some(String::new()), None, None, None, None, None);
        assert_eq!(engine.model, "");
    }

    #[test]
    fn set_config_model_does_not_affect_other_state() {
        let mut engine = make_engine("m");
        engine.reasoning_effort = Some("high".into());
        engine.apply_config_update(Some("new-m".into()), None, None, None, None, None);
        assert_eq!(engine.model, "new-m");
        assert_eq!(engine.reasoning_effort.as_deref(), Some("high"));
    }

    #[test]
    fn set_config_model_change_resets_stale_image_capability() {
        let compat = ProviderCompat {
            image_input: Some(ImageInputCapability::Supported),
            ..Default::default()
        };
        let mut engine = make_engine_with_compat("vision-model", compat);

        let changes = engine.apply_config_update(Some("text-model".into()), None, None, None, None, None);

        assert_eq!(engine.compat.image_input(), ImageInputCapability::Unknown);
        assert!(changes.iter().any(|change| change.contains("image input")));
    }

    #[test]
    fn set_config_applies_host_resolved_image_capability_with_model() {
        let mut engine = make_engine("old-model");

        let changes = engine.apply_config_update(
            Some("vision-model".into()),
            Some(ImageInputCapability::Supported),
            None,
            None,
            None,
            None,
        );

        assert_eq!(engine.model, "vision-model");
        assert_eq!(engine.compat.image_input(), ImageInputCapability::Supported);
        assert!(changes.iter().any(|change| change.contains("image input")));
    }

    // --- Cycle 2: Effort config tests ---

    #[test]
    fn set_config_changes_effort() {
        let mut engine = make_engine_with_compat("m", ProviderCompat::openai_defaults());
        assert!(engine.reasoning_effort.is_none());
        let changes = engine.apply_config_update(None, None, None, None, Some("high".into()), None);
        assert_eq!(engine.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(changes.len(), 1);
        assert!(changes[0].contains("high"));
    }

    #[test]
    fn set_config_clears_effort_with_empty_string() {
        let mut engine = make_engine("m");
        engine.reasoning_effort = Some("high".into());
        let changes = engine.apply_config_update(None, None, None, None, Some(String::new()), None);
        assert!(engine.reasoning_effort.is_none());
        assert_eq!(changes.len(), 1);
    }

    // --- Cycle 2: Thinking config tests ---

    #[test]
    fn set_config_enables_thinking() {
        let mut engine = make_engine("m");
        let changes = engine.apply_config_update(None, None, Some("enabled".into()), Some(16000), None, None);
        match &engine.thinking {
            Some(aion_types::llm::ThinkingConfig::Enabled { budget_tokens }) => {
                assert_eq!(*budget_tokens, 16000);
            }
            other => panic!("expected Enabled, got: {other:?}"),
        }
        assert_eq!(changes.len(), 1);
    }

    #[test]
    fn set_config_disables_thinking() {
        let mut engine = make_engine("m");
        engine.thinking = Some(aion_types::llm::ThinkingConfig::Enabled { budget_tokens: 8000 });
        let changes = engine.apply_config_update(None, None, Some("disabled".into()), None, None, None);
        match &engine.thinking {
            Some(aion_types::llm::ThinkingConfig::Disabled) => {}
            other => panic!("expected Disabled, got: {other:?}"),
        }
        assert_eq!(changes.len(), 1);
    }

    #[test]
    fn set_config_thinking_enabled_default_budget() {
        let mut engine = make_engine("m");
        let changes = engine.apply_config_update(None, None, Some("enabled".into()), None, None, None);
        match &engine.thinking {
            Some(aion_types::llm::ThinkingConfig::Enabled { budget_tokens }) => {
                assert!(*budget_tokens > 0);
            }
            other => panic!("expected Enabled with default budget, got: {other:?}"),
        }
        assert_eq!(changes.len(), 1);
    }

    #[test]
    fn set_config_invalid_thinking_ignored() {
        let mut engine = make_engine("m");
        engine.thinking = Some(aion_types::llm::ThinkingConfig::Enabled { budget_tokens: 8000 });
        let changes = engine.apply_config_update(None, None, Some("invalid_value".into()), None, None, None);
        match &engine.thinking {
            Some(aion_types::llm::ThinkingConfig::Enabled { budget_tokens }) => {
                assert_eq!(*budget_tokens, 8000);
            }
            other => panic!("expected Enabled unchanged, got: {other:?}"),
        }
        assert_eq!(changes.len(), 1);
        assert!(changes[0].contains("invalid") || changes[0].contains("ignored"));
    }

    // --- Cycle 2: Combined fields test ---

    #[test]
    fn set_config_all_fields_at_once() {
        let compat = ProviderCompat {
            reasoning: ReasoningCompat {
                supports_thinking: Some(true),
                supports_effort: Some(true),
                effort_levels: Some(vec!["low".into()]),
            },
            ..Default::default()
        };
        let mut engine = make_engine_with_compat("old-model", compat);
        let changes = engine.apply_config_update(
            Some("new-model".into()),
            None,
            Some("enabled".into()),
            Some(12000),
            Some("low".into()),
            None,
        );
        assert_eq!(engine.model, "new-model");
        assert_eq!(engine.reasoning_effort.as_deref(), Some("low"));
        match &engine.thinking {
            Some(aion_types::llm::ThinkingConfig::Enabled { budget_tokens }) => {
                assert_eq!(*budget_tokens, 12000);
            }
            other => panic!("expected Enabled, got: {other:?}"),
        }
        assert_eq!(changes.len(), 3);
    }

    // --- Cycle 2: White-box edge case tests ---

    #[test]
    fn set_config_thinking_budget_only_updates_existing_enabled() {
        let mut engine = make_engine("m");
        engine.thinking = Some(aion_types::llm::ThinkingConfig::Enabled { budget_tokens: 5000 });
        let changes = engine.apply_config_update(None, None, None, Some(20000), None, None);
        match &engine.thinking {
            Some(aion_types::llm::ThinkingConfig::Enabled { budget_tokens }) => {
                assert_eq!(*budget_tokens, 20000);
            }
            other => panic!("expected Enabled with 20000, got: {other:?}"),
        }
        assert_eq!(changes.len(), 1);
    }

    #[test]
    fn set_config_thinking_budget_ignored_when_disabled() {
        let mut engine = make_engine("m");
        engine.thinking = Some(aion_types::llm::ThinkingConfig::Disabled);
        let changes = engine.apply_config_update(None, None, None, Some(20000), None, None);
        match &engine.thinking {
            Some(aion_types::llm::ThinkingConfig::Disabled) => {}
            other => panic!("expected Disabled unchanged, got: {other:?}"),
        }
        assert!(changes.is_empty());
    }

    #[test]
    fn set_config_thinking_enabled_applies_even_when_capability_is_false() {
        let compat = ProviderCompat {
            reasoning: ReasoningCompat {
                supports_thinking: Some(false),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut engine = make_engine_with_compat("m", compat);

        let changes = engine.apply_config_update(None, None, Some("enabled".into()), Some(16000), None, None);

        match &engine.thinking {
            Some(aion_types::llm::ThinkingConfig::Enabled { budget_tokens }) => {
                assert_eq!(*budget_tokens, 16000);
            }
            other => panic!("expected Enabled with 16000, got: {other:?}"),
        }
        assert_eq!(changes, vec!["thinking: enabled (budget: 16000)"]);
    }

    #[test]
    fn set_config_effort_valid_values() {
        let compat = ProviderCompat {
            reasoning: ReasoningCompat {
                supports_effort: Some(true),
                effort_levels: Some(vec!["low".into(), "medium".into(), "high".into(), "max".into()]),
                ..Default::default()
            },
            ..Default::default()
        };
        for value in ["low", "medium", "high", "max"] {
            let mut engine = make_engine_with_compat("m", compat.clone());
            engine.apply_config_update(None, None, None, None, Some(value.to_string()), None);
            assert_eq!(
                engine.reasoning_effort.as_deref(),
                Some(value),
                "effort should be set to {value}"
            );
        }
    }

    // --- Capability validation tests ---

    #[test]
    fn set_config_thinking_applies_when_capability_is_false() {
        let mut engine = make_engine_with_compat("m", ProviderCompat::openai_defaults());
        let changes = engine.apply_config_update(None, None, Some("enabled".into()), None, None, None);
        assert_eq!(changes, vec!["thinking: enabled (budget: 10000)"]);
        assert!(matches!(
            engine.thinking,
            Some(aion_types::llm::ThinkingConfig::Enabled { budget_tokens: 10000 })
        ));
    }

    #[test]
    fn set_config_effort_rejected_when_unsupported() {
        let mut engine = make_engine("m"); // anthropic defaults: supports_effort = false
        let changes = engine.apply_config_update(None, None, None, None, Some("high".into()), None);
        assert!(changes.iter().any(|c| c.contains("not supported")));
        assert!(engine.reasoning_effort.is_none());
    }

    #[test]
    fn set_config_effort_rejected_invalid_level() {
        let mut engine = make_engine_with_compat("m", ProviderCompat::openai_defaults());
        let changes = engine.apply_config_update(None, None, None, None, Some("max".into()), None);
        assert!(changes.iter().any(|c| c.contains("invalid")));
        assert!(engine.reasoning_effort.is_none());
    }

    #[test]
    fn set_config_effort_clear_always_works() {
        let mut engine = make_engine("m"); // anthropic defaults: supports_effort = false
        engine.reasoning_effort = Some("high".into());
        let changes = engine.apply_config_update(None, None, None, None, Some(String::new()), None);
        assert!(engine.reasoning_effort.is_none());
        assert!(changes.iter().any(|c| c.contains("cleared")));
    }
}

// ---------------------------------------------------------------------------
// Phase 6 tests — apply_context_modifiers()
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests_phase6 {
    use std::sync::{Arc, Mutex};

    use aion_providers::error::ProviderError;
    use aion_providers::provider::LlmProvider;
    use aion_tools::registry::ToolRegistry;
    use aion_types::llm::{LlmEvent, LlmRequest};
    use aion_types::skill_types::{ContextModifier, EffortLevel};

    use super::{CompactLevel, ProviderCompat};
    use crate::confirm::ToolConfirmer;
    use crate::output::OutputSink;

    struct NullOutput;
    impl OutputSink for NullOutput {
        fn emit_text_delta(&self, _: &str, _: &str) {}
        fn emit_thinking(&self, _: &str, _: &str) {}
        fn emit_tool_call(&self, _: &str, _: &str, _: &str) {}
        fn emit_tool_result(&self, _: &str, _: &str, _: bool, _: &str) {}
        fn emit_stream_start(&self, _: &str) {}
        fn emit_stream_end(&self, _: &str, _: usize, _: u64, _: u64, _: u64, _: u64) {}
        fn emit_error(&self, _: &str) {}
        fn emit_info(&self, _: &str) {}
    }

    struct NullProvider;
    #[async_trait::async_trait]
    impl LlmProvider for NullProvider {
        async fn stream(&self, _: &LlmRequest) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            Ok(rx)
        }
    }

    fn make_engine(model: &str, allow_list: Vec<String>) -> super::AgentEngine {
        super::AgentEngine {
            provider: Arc::new(NullProvider),
            model: model.to_string(),
            max_tokens: Some(4096),
            thinking: None,
            compat: ProviderCompat::anthropic_defaults(),
            system_prompt: String::new(),
            reasoning_effort: None,
            messages: vec![],
            total_usage: Default::default(),
            msg_id: String::new(),
            max_turns_per_run: Some(10),
            max_tool_call_malformed_turns: 3,
            max_tool_call_failure_turns: 3,
            tools: ToolRegistry::new(),
            tool_policy: Default::default(),
            confirmer: Arc::new(Mutex::new(ToolConfirmer::new(true, allow_list.clone()))),
            allow_list,
            hooks: None,
            session_manager: None,
            current_session: None,
            output: Arc::new(NullOutput),
            approval_manager: None,
            protocol_writer: None,
            compact_config: aion_config::compact::CompactConfig::default(),
            compact_state: super::CompactState::new(),
            compact_level: CompactLevel::default(),
            toon_enabled: false,
            plan_state: Default::default(),
            plan_active_flag: None,
            cache_detector: super::CacheBreakDetector::new(),
            commands: crate::commands::default_registry(),
        }
    }

    #[test]
    fn tc_6_21_model_override_applied() {
        let mut engine = make_engine("original-model", vec![]);
        let modifiers = vec![Some(ContextModifier {
            model: Some("override-model".to_string()),
            ..Default::default()
        })];
        engine.apply_context_modifiers(&modifiers);
        assert_eq!(engine.model, "override-model");
    }

    #[test]
    fn tc_6_22_effort_override_applied() {
        let mut engine = make_engine("m", vec![]);
        let modifiers = vec![Some(ContextModifier {
            effort: Some(EffortLevel::High),
            ..Default::default()
        })];
        engine.apply_context_modifiers(&modifiers);
        assert_eq!(engine.reasoning_effort.as_deref(), Some("high"));
    }

    #[test]
    fn tc_6_22b_effort_all_variants() {
        for (level, expected) in [
            (EffortLevel::Low, "low"),
            (EffortLevel::Medium, "medium"),
            (EffortLevel::High, "high"),
            (EffortLevel::Max, "max"),
        ] {
            let mut engine = make_engine("m", vec![]);
            engine.apply_context_modifiers(&[Some(ContextModifier {
                effort: Some(level),
                ..Default::default()
            })]);
            assert_eq!(
                engine.reasoning_effort.as_deref(),
                Some(expected),
                "EffortLevel::{level:?} should map to {expected:?}"
            );
        }
    }

    #[test]
    fn tc_6_23_allowed_tools_no_duplicates() {
        let mut engine = make_engine("m", vec!["ExecCommand".to_string()]);
        let modifiers = vec![Some(ContextModifier {
            allowed_tools: vec!["ExecCommand".to_string(), "Read".to_string()],
            ..Default::default()
        })];
        engine.apply_context_modifiers(&modifiers);
        let bash_count = engine.allow_list.iter().filter(|t| t.as_str() == "ExecCommand").count();
        assert_eq!(bash_count, 1, "ExecCommand should appear exactly once");
        assert!(engine.allow_list.contains(&"Read".to_string()));
    }

    #[test]
    fn tc_6_24_none_modifiers_skipped() {
        let mut engine = make_engine("original", vec![]);
        engine.apply_context_modifiers(&[None, None]);
        assert_eq!(engine.model, "original");
        assert!(engine.reasoning_effort.is_none());
    }

    #[test]
    fn tc_6_25_empty_modifiers_no_change() {
        let mut engine = make_engine("current-model", vec![]);
        engine.apply_context_modifiers(&[]);
        assert_eq!(engine.model, "current-model");
        assert!(engine.allow_list.is_empty());
    }

    #[test]
    fn tc_6_26_none_model_does_not_overwrite() {
        let mut engine = make_engine("current-model", vec![]);
        engine.apply_context_modifiers(&[Some(ContextModifier {
            allowed_tools: vec!["ExecCommand".to_string()],
            ..Default::default()
        })]);
        assert_eq!(engine.model, "current-model");
        assert!(engine.allow_list.contains(&"ExecCommand".to_string()));
    }

    #[test]
    fn tc_6_27_multiple_modifiers_stacked() {
        let mut engine = make_engine("initial", vec![]);
        let modifiers = vec![
            Some(ContextModifier {
                model: Some("model-a".to_string()),
                allowed_tools: vec!["ExecCommand".to_string()],
                ..Default::default()
            }),
            Some(ContextModifier {
                model: Some("model-b".to_string()),
                allowed_tools: vec!["Read".to_string()],
                ..Default::default()
            }),
        ];
        engine.apply_context_modifiers(&modifiers);
        assert_eq!(engine.model, "model-b", "last model wins");
        assert!(engine.allow_list.contains(&"ExecCommand".to_string()));
        assert!(engine.allow_list.contains(&"Read".to_string()));
    }

    #[test]
    fn tc_6_28_modifier_applied_after_tool_execution_not_during() {
        let mut engine = make_engine("original", vec![]);
        let model_before = engine.model.clone();
        let modifiers = vec![Some(ContextModifier {
            model: Some("new-model".to_string()),
            ..Default::default()
        })];
        assert_eq!(engine.model, model_before);
        engine.apply_context_modifiers(&modifiers);
        assert_eq!(engine.model, "new-model");
        assert_eq!(model_before, "original");
    }
}

// ---------------------------------------------------------------------------
// Phase 2 tests — run_compaction()
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests_compact {
    use std::sync::{Arc, Mutex};

    use aion_config::compact::CompactConfig;
    use aion_providers::error::ProviderError;
    use aion_providers::provider::LlmProvider;
    use aion_tools::registry::ToolRegistry;
    use aion_types::llm::{LlmEvent, LlmRequest};
    use aion_types::message::{ContentBlock, ImageInputCapability, ImageUrl, Message, Role, TokenUsage};
    use serde_json::json;

    use super::{CompactLevel, ProviderCompat};
    use crate::compact::auto::should_autocompact;
    use crate::compact::state::CompactState;
    use crate::confirm::ToolConfirmer;
    use crate::output::OutputSink;

    struct NullOutput;
    impl OutputSink for NullOutput {
        fn emit_text_delta(&self, _: &str, _: &str) {}
        fn emit_thinking(&self, _: &str, _: &str) {}
        fn emit_tool_call(&self, _: &str, _: &str, _: &str) {}
        fn emit_tool_result(&self, _: &str, _: &str, _: bool, _: &str) {}
        fn emit_stream_start(&self, _: &str) {}
        fn emit_stream_end(&self, _: &str, _: usize, _: u64, _: u64, _: u64, _: u64) {}
        fn emit_error(&self, _: &str) {}
        fn emit_info(&self, _: &str) {}
    }

    #[derive(Default)]
    struct RecordingOutput {
        tool_results: Mutex<Vec<(String, String, bool, String)>>,
        errors: Mutex<Vec<String>>,
        infos: Mutex<Vec<String>>,
    }

    impl OutputSink for RecordingOutput {
        fn emit_text_delta(&self, _: &str, _: &str) {}
        fn emit_thinking(&self, _: &str, _: &str) {}
        fn emit_tool_call(&self, _: &str, _: &str, _: &str) {}
        fn emit_tool_result(&self, tool_use_id: &str, name: &str, is_error: bool, content: &str) {
            self.tool_results.lock().unwrap().push((
                tool_use_id.to_string(),
                name.to_string(),
                is_error,
                content.to_string(),
            ));
        }
        fn emit_stream_start(&self, _: &str) {}
        fn emit_stream_end(&self, _: &str, _: usize, _: u64, _: u64, _: u64, _: u64) {}

        fn emit_error(&self, msg: &str) {
            self.errors.lock().unwrap().push(msg.to_string());
        }

        fn emit_info(&self, msg: &str) {
            self.infos.lock().unwrap().push(msg.to_string());
        }
    }

    struct NullProvider;
    #[async_trait::async_trait]
    impl LlmProvider for NullProvider {
        async fn stream(&self, _: &LlmRequest) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            Ok(rx)
        }
    }

    #[derive(Default)]
    struct RecordingRejectingProvider {
        requests: Mutex<Vec<LlmRequest>>,
    }

    #[async_trait::async_trait]
    impl LlmProvider for RecordingRejectingProvider {
        async fn stream(&self, request: &LlmRequest) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            self.requests.lock().unwrap().push(request.clone());
            Err(ProviderError::Connection("intentional test failure".to_owned()))
        }
    }

    fn make_compact_engine(
        compact_config: CompactConfig,
        compact_state: CompactState,
        messages: Vec<Message>,
    ) -> super::AgentEngine {
        make_compact_engine_with_output(compact_config, compact_state, messages, Arc::new(NullOutput))
    }

    fn make_compact_engine_with_output(
        compact_config: CompactConfig,
        compact_state: CompactState,
        messages: Vec<Message>,
        output: Arc<dyn OutputSink>,
    ) -> super::AgentEngine {
        super::AgentEngine {
            provider: Arc::new(NullProvider),
            model: "test-model".to_string(),
            max_tokens: Some(4096),
            thinking: None,
            compat: ProviderCompat::anthropic_defaults(),
            system_prompt: String::new(),
            reasoning_effort: None,
            messages,
            total_usage: Default::default(),
            msg_id: String::new(),
            max_turns_per_run: Some(10),
            max_tool_call_malformed_turns: 3,
            max_tool_call_failure_turns: 3,
            tools: ToolRegistry::new(),
            tool_policy: Default::default(),
            confirmer: Arc::new(Mutex::new(ToolConfirmer::new(true, vec![]))),
            allow_list: vec![],
            hooks: None,
            session_manager: None,
            current_session: None,
            output,
            approval_manager: None,
            protocol_writer: None,
            compact_config,
            compact_state,
            compact_level: CompactLevel::default(),
            toon_enabled: false,
            plan_state: Default::default(),
            plan_active_flag: None,
            cache_detector: super::CacheBreakDetector::new(),
            commands: crate::commands::default_registry(),
        }
    }

    fn tool_use_msg(id: &str, name: &str) -> Message {
        Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: id.to_string(),
                name: name.to_string(),
                input: json!({}),
                extra: None,
            }],
        )
    }

    fn tool_use_msg_with_two_calls(first_id: &str, second_id: &str) -> Message {
        Message::new(
            Role::Assistant,
            vec![
                ContentBlock::ToolUse {
                    id: first_id.to_string(),
                    name: "Read".to_string(),
                    input: json!({}),
                    extra: None,
                },
                ContentBlock::ToolUse {
                    id: second_id.to_string(),
                    name: "ExecCommand".to_string(),
                    input: json!({}),
                    extra: None,
                },
            ],
        )
    }

    fn tool_result_msg(id: &str, content: &str) -> Message {
        Message::new(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content: content.to_string(),
                is_error: false,
            }],
        )
    }

    #[test]
    fn abort_current_turn_closes_pending_tool_uses() {
        let output = Arc::new(RecordingOutput::default());
        let mut engine = make_compact_engine_with_output(
            CompactConfig::default(),
            CompactState::new(),
            vec![
                Message::new(
                    Role::User,
                    vec![ContentBlock::Text {
                        text: "run tools".to_string(),
                    }],
                ),
                tool_use_msg_with_two_calls("call_read", "call_bash"),
            ],
            output.clone(),
        );

        engine.abort_current_turn("Tool execution canceled by user");

        let last = engine.messages.last().expect("synthetic result message");
        assert_eq!(last.role, Role::User);
        assert_eq!(last.content.len(), 2);
        assert!(
            matches!(&last.content[0], ContentBlock::ToolResult { tool_use_id, content, is_error }
                if tool_use_id == "call_read" && content == "Tool execution canceled by user" && *is_error)
        );
        assert!(
            matches!(&last.content[1], ContentBlock::ToolResult { tool_use_id, content, is_error }
                if tool_use_id == "call_bash" && content == "Tool execution canceled by user" && *is_error)
        );
        assert_eq!(engine.compact_state.last_input_tokens, 14);

        let emitted = output.tool_results.lock().unwrap();
        assert_eq!(emitted.len(), 2);
        assert_eq!(
            emitted[0],
            (
                "call_read".into(),
                "Read".into(),
                true,
                "Tool execution canceled by user".into()
            )
        );
        assert_eq!(
            emitted[1],
            (
                "call_bash".into(),
                "ExecCommand".into(),
                true,
                "Tool execution canceled by user".into()
            )
        );
    }

    #[test]
    fn cache_full_miss_is_reported_as_info_not_error() {
        let output = Arc::new(RecordingOutput::default());
        let mut engine =
            make_compact_engine_with_output(CompactConfig::default(), CompactState::new(), vec![], output.clone());

        engine.cache_detector.record_request("prompt", &[]);
        engine.record_turn_usage(&TokenUsage {
            input_tokens: 10_000,
            output_tokens: 100,
            cache_creation_tokens: 2_000,
            cache_read_tokens: 8_000,
        });

        engine.cache_detector.record_request("prompt", &[]);
        engine.record_turn_usage(&TokenUsage {
            input_tokens: 10_000,
            output_tokens: 100,
            cache_creation_tokens: 10_000,
            cache_read_tokens: 0,
        });

        assert!(
            output.errors.lock().unwrap().is_empty(),
            "cache diagnostics should not emit terminal errors"
        );
        assert!(
            output
                .infos
                .lock()
                .unwrap()
                .iter()
                .any(|msg| msg == "Cache full miss: TtlExpiry"),
            "full cache misses should remain visible as diagnostics"
        );
    }

    #[test]
    fn provider_turn_total_replaces_previous_context_estimate() {
        let mut state = CompactState::new();
        state.last_input_tokens = 100_000;
        let mut engine = make_compact_engine(CompactConfig::default(), state, vec![]);

        engine.record_turn_usage(&TokenUsage {
            input_tokens: 693,
            output_tokens: 102,
            cache_read_tokens: 512,
            ..Default::default()
        });

        assert_eq!(engine.compact_state.last_input_tokens, 795);
    }

    #[test]
    fn all_tool_results_are_added_before_the_next_threshold_check() {
        let config = CompactConfig {
            context_window: 1_000,
            autocompact_threshold_pct: Some(60),
            ..Default::default()
        };
        let mut engine = make_compact_engine(config, CompactState::new(), vec![]);
        engine.record_turn_usage(&TokenUsage {
            input_tokens: 500,
            output_tokens: 50,
            ..Default::default()
        });
        assert!(!should_autocompact(
            engine.compact_state.last_input_tokens,
            &engine.compact_config
        ));

        let tool_results = vec![
            ContentBlock::ToolResult {
                tool_use_id: "call-1".into(),
                content: "a".repeat(200),
                is_error: false,
            },
            ContentBlock::ToolResult {
                tool_use_id: "call-2".into(),
                content: "b".repeat(200),
                is_error: false,
            },
        ];
        engine.record_tool_context_estimate(&tool_results, &[]);

        assert_eq!(engine.compact_state.last_input_tokens, 650);
        assert!(should_autocompact(
            engine.compact_state.last_input_tokens,
            &engine.compact_config
        ));
    }

    // -- Emergency check fires when at limit --

    #[tokio::test]
    async fn emergency_fires_when_at_limit() {
        let config = CompactConfig {
            context_window: 200_000,
            emergency_buffer: 3_000,
            ..Default::default()
        };
        let mut state = CompactState::new();
        state.last_input_tokens = 198_000; // >= 197k limit

        let mut engine = make_compact_engine(config, state, vec![]);
        let result = engine.run_compaction().await;

        match result {
            Err(super::AgentError::ContextTooLong { input_tokens, limit }) => {
                assert_eq!(input_tokens, 198_000);
                assert_eq!(limit, 197_000);
            }
            other => panic!("expected ContextTooLong, got: {:?}", other),
        }
    }

    // -- Emergency does not fire when below limit --

    #[tokio::test]
    async fn emergency_silent_below_limit() {
        let config = CompactConfig::default();
        let mut state = CompactState::new();
        state.last_input_tokens = 190_000; // below 197k

        let mut engine = make_compact_engine(config, state, vec![]);
        assert!(engine.run_compaction().await.is_ok());
    }

    // -- Microcompact runs when count trigger fires --

    #[tokio::test]
    async fn microcompact_clears_old_results() {
        // 12 tool results with keep_recent=3 (threshold=6) → should clear 9
        let mut messages = Vec::new();
        for i in 0..12 {
            let id = format!("t{i}");
            messages.push(tool_use_msg(&id, "Read"));
            messages.push(tool_result_msg(&id, &format!("data-{i}")));
        }

        let config = CompactConfig {
            micro_keep_recent: 3,
            ..Default::default()
        };
        let state = CompactState::new();

        let mut engine = make_compact_engine(config, state, messages);
        engine.run_compaction().await.unwrap();

        // Last 3 tool results should be preserved
        let cleared_count = engine
            .messages
            .iter()
            .flat_map(|m| &m.content)
            .filter(|b| matches!(b, ContentBlock::ToolResult { content, .. } if content == "[Tool result cleared]"))
            .count();

        assert_eq!(cleared_count, 9);
    }

    // -- Disabled config skips micro and auto but not emergency --

    #[tokio::test]
    async fn disabled_config_skips_micro_auto() {
        let mut messages = Vec::new();
        for i in 0..12 {
            let id = format!("t{i}");
            messages.push(tool_use_msg(&id, "Read"));
            messages.push(tool_result_msg(&id, &format!("data-{i}")));
        }

        let config = CompactConfig {
            enabled: false,
            micro_keep_recent: 3,
            ..Default::default()
        };
        let state = CompactState::new();

        let mut engine = make_compact_engine(config, state, messages);
        engine.run_compaction().await.unwrap();

        // Nothing should be cleared (microcompact skipped)
        let cleared_count = engine
            .messages
            .iter()
            .flat_map(|m| &m.content)
            .filter(|b| matches!(b, ContentBlock::ToolResult { content, .. } if content == "[Tool result cleared]"))
            .count();

        assert_eq!(cleared_count, 0, "microcompact should be skipped when disabled");
    }

    #[tokio::test]
    async fn disabled_config_still_fires_emergency() {
        let config = CompactConfig {
            enabled: false,
            context_window: 200_000,
            emergency_buffer: 3_000,
            ..Default::default()
        };
        let mut state = CompactState::new();
        state.last_input_tokens = 198_000;

        let mut engine = make_compact_engine(config, state, vec![]);
        let result = engine.run_compaction().await;

        assert!(
            matches!(result, Err(super::AgentError::ContextTooLong { .. })),
            "emergency should fire even when disabled"
        );
    }

    // -- Zero tokens on first turn does not trigger anything --

    #[tokio::test]
    async fn first_turn_zero_tokens_no_compaction() {
        let config = CompactConfig::default();
        let state = CompactState::new(); // context_tokens = 0

        let mut engine = make_compact_engine(config, state, vec![]);
        assert!(engine.run_compaction().await.is_ok());
        assert_eq!(engine.compact_state.last_input_tokens, 0);
    }

    #[tokio::test]
    async fn autocompact_projects_images_without_mutating_failed_history() {
        let config = CompactConfig {
            context_window: 100,
            emergency_buffer: 10,
            autocompact_threshold_pct: Some(50),
            ..Default::default()
        };
        let mut state = CompactState::new();
        state.last_input_tokens = 60;
        let messages = vec![Message::new(
            Role::User,
            vec![
                ContentBlock::Text {
                    text: "Attachment path: /tmp/image.png".to_owned(),
                },
                ContentBlock::Image {
                    image_url: ImageUrl {
                        url: "data:image/png;base64,abcd".to_owned(),
                    },
                },
            ],
        )];
        let provider = Arc::new(RecordingRejectingProvider::default());
        let mut engine = make_compact_engine(config, state, messages);
        engine.provider = provider.clone();
        engine.compat.image_input = Some(ImageInputCapability::Unsupported);

        engine.run_compaction().await.unwrap();

        let requests = provider.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0]
                .messages
                .iter()
                .flat_map(|message| &message.content)
                .all(|block| !matches!(block, ContentBlock::Image { .. }))
        );
        assert!(
            engine.messages[0]
                .content
                .iter()
                .any(|block| matches!(block, ContentBlock::Image { .. }))
        );
    }

    // -- Circuit broken prevents autocompact, emergency still fires --

    #[tokio::test]
    async fn circuit_broken_skips_auto_but_emergency_fires() {
        let config = CompactConfig {
            context_window: 200_000,
            emergency_buffer: 3_000,
            max_failures: 3,
            ..Default::default()
        };
        let mut state = CompactState::new();
        state.last_input_tokens = 198_000; // triggers both auto and emergency
        state.consecutive_failures = 3; // circuit broken

        let mut engine = make_compact_engine(config, state, vec![]);
        let result = engine.run_compaction().await;

        // Auto is skipped due to circuit breaker; emergency fires
        assert!(matches!(result, Err(super::AgentError::ContextTooLong { .. })));
    }
}

// ---------------------------------------------------------------------------
// Phase 3 tests — plan mode integration in apply_context_modifiers()
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests_plan_mode {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use aion_providers::error::ProviderError;
    use aion_providers::provider::LlmProvider;
    use aion_tools::registry::ToolRegistry;
    use aion_types::llm::{LlmEvent, LlmRequest};
    use aion_types::skill_types::{ContextModifier, PlanModeTransition};

    use super::{CompactLevel, ProviderCompat};
    use crate::compact::state::CompactState;
    use crate::confirm::ToolConfirmer;
    use crate::output::OutputSink;
    use crate::plan::state::PlanState;

    struct NullOutput;
    impl OutputSink for NullOutput {
        fn emit_text_delta(&self, _: &str, _: &str) {}
        fn emit_thinking(&self, _: &str, _: &str) {}
        fn emit_tool_call(&self, _: &str, _: &str, _: &str) {}
        fn emit_tool_result(&self, _: &str, _: &str, _: bool, _: &str) {}
        fn emit_stream_start(&self, _: &str) {}
        fn emit_stream_end(&self, _: &str, _: usize, _: u64, _: u64, _: u64, _: u64) {}
        fn emit_error(&self, _: &str) {}
        fn emit_info(&self, _: &str) {}
    }

    struct NullProvider;
    #[async_trait::async_trait]
    impl LlmProvider for NullProvider {
        async fn stream(&self, _: &LlmRequest) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            Ok(rx)
        }
    }

    fn make_plan_engine(allow_list: Vec<String>) -> super::AgentEngine {
        let flag = Arc::new(AtomicBool::new(false));
        super::AgentEngine {
            provider: Arc::new(NullProvider),
            model: "test-model".to_string(),
            max_tokens: Some(4096),
            thinking: None,
            compat: ProviderCompat::anthropic_defaults(),
            system_prompt: String::new(),
            reasoning_effort: None,
            messages: vec![],
            total_usage: Default::default(),
            msg_id: String::new(),
            max_turns_per_run: Some(10),
            max_tool_call_malformed_turns: 3,
            max_tool_call_failure_turns: 3,
            tools: ToolRegistry::new(),
            tool_policy: Default::default(),
            confirmer: Arc::new(Mutex::new(ToolConfirmer::new(true, allow_list.clone()))),
            allow_list,
            hooks: None,
            session_manager: None,
            current_session: None,
            output: Arc::new(NullOutput),
            approval_manager: None,
            protocol_writer: None,
            compact_config: aion_config::compact::CompactConfig::default(),
            compact_state: CompactState::new(),
            compact_level: CompactLevel::default(),
            toon_enabled: false,
            plan_state: PlanState::default(),
            plan_active_flag: Some(flag),
            cache_detector: super::CacheBreakDetector::new(),
            commands: crate::commands::default_registry(),
        }
    }

    // --- TC-3.5-03: Enter transition activates plan mode ---

    #[test]
    fn enter_transition_activates_plan_mode() {
        let mut engine = make_plan_engine(vec!["Read".into(), "ExecCommand".into()]);
        let modifiers = vec![Some(ContextModifier {
            plan_mode_transition: Some(PlanModeTransition::Enter),
            ..Default::default()
        })];

        engine.apply_context_modifiers(&modifiers);

        assert!(engine.plan_state.is_active, "plan mode should be active");
        assert_eq!(
            engine.plan_state.pre_plan_allow_list,
            vec!["Read".to_string(), "ExecCommand".to_string()],
            "pre_plan_allow_list should capture original allow_list"
        );
    }

    // --- TC-3.5-03 supplement: shared flag updated on enter ---

    #[test]
    fn enter_transition_updates_shared_flag() {
        let mut engine = make_plan_engine(vec![]);
        let flag = engine.plan_active_flag.clone().unwrap();
        assert!(!flag.load(Ordering::Acquire));

        engine.apply_context_modifiers(&[Some(ContextModifier {
            plan_mode_transition: Some(PlanModeTransition::Enter),
            ..Default::default()
        })]);

        assert!(flag.load(Ordering::Acquire), "shared flag should be true");
    }

    // --- TC-3.5-04: Exit transition deactivates plan mode and restores allow_list ---

    #[test]
    fn exit_transition_deactivates_and_restores() {
        let mut engine = make_plan_engine(vec!["Read".into(), "ExecCommand".into()]);

        // Enter plan mode first
        engine.apply_context_modifiers(&[Some(ContextModifier {
            plan_mode_transition: Some(PlanModeTransition::Enter),
            ..Default::default()
        })]);
        assert!(engine.plan_state.is_active);

        // Modify allow_list while in plan mode (simulating a skill adding tools)
        engine.allow_list.push("NewTool".into());

        // Exit plan mode
        engine.apply_context_modifiers(&[Some(ContextModifier {
            plan_mode_transition: Some(PlanModeTransition::Exit { plan_content: None }),
            ..Default::default()
        })]);

        assert!(!engine.plan_state.is_active, "plan mode should be inactive");
        assert_eq!(
            engine.allow_list,
            vec!["Read".to_string(), "ExecCommand".to_string()],
            "allow_list should be restored to pre-plan state"
        );
    }

    // --- TC-3.5-04 supplement: shared flag updated on exit ---

    #[test]
    fn exit_transition_updates_shared_flag() {
        let mut engine = make_plan_engine(vec![]);
        let flag = engine.plan_active_flag.clone().unwrap();

        // Enter
        engine.apply_context_modifiers(&[Some(ContextModifier {
            plan_mode_transition: Some(PlanModeTransition::Enter),
            ..Default::default()
        })]);
        assert!(flag.load(Ordering::Acquire));

        // Exit
        engine.apply_context_modifiers(&[Some(ContextModifier {
            plan_mode_transition: Some(PlanModeTransition::Exit { plan_content: None }),
            ..Default::default()
        })]);
        assert!(!flag.load(Ordering::Acquire), "shared flag should be false after exit");
    }

    // --- TC-3.5-05: No transition does not affect plan state ---

    #[test]
    fn no_transition_does_not_affect_plan_state() {
        let mut engine = make_plan_engine(vec![]);

        engine.apply_context_modifiers(&[Some(ContextModifier {
            model: Some("new-model".into()),
            plan_mode_transition: None,
            ..Default::default()
        })]);

        assert_eq!(engine.model, "new-model");
        assert!(!engine.plan_state.is_active, "plan state should remain inactive");
    }

    // --- Enter + other modifiers applied together ---

    #[test]
    fn enter_with_model_override_both_applied() {
        let mut engine = make_plan_engine(vec![]);

        engine.apply_context_modifiers(&[Some(ContextModifier {
            model: Some("planning-model".into()),
            plan_mode_transition: Some(PlanModeTransition::Enter),
            ..Default::default()
        })]);

        assert!(engine.plan_state.is_active);
        assert_eq!(engine.model, "planning-model");
    }

    // --- No plan_active_flag set does not panic ---

    #[test]
    fn enter_without_flag_does_not_panic() {
        let mut engine = make_plan_engine(vec![]);
        engine.plan_active_flag = None;

        engine.apply_context_modifiers(&[Some(ContextModifier {
            plan_mode_transition: Some(PlanModeTransition::Enter),
            ..Default::default()
        })]);

        assert!(engine.plan_state.is_active);
    }
}

#[cfg(test)]
mod tests_handle_command {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use aion_config::compact::CompactConfig;
    use aion_protocol::events::ToolCategory;
    use aion_providers::error::ProviderError;
    use aion_providers::provider::LlmProvider;
    use aion_tools::Tool;
    use aion_tools::registry::ToolRegistry;
    use aion_types::llm::{LlmEvent, LlmRequest};
    use aion_types::message::{ContentBlock, ImageInputCapability, ImageUrl, Message, Role, StopReason, TokenUsage};
    use aion_types::tool::ToolResult;
    use async_trait::async_trait;
    use serde_json::{Value, json};
    use tokio::sync::mpsc::{Receiver, channel};

    use super::{AgentEngine, AgentError, CacheBreakDetector, CompactLevel, ProviderCompat};
    use crate::commands::default_registry;
    use crate::compact::state::CompactState;
    use crate::confirm::ToolConfirmer;
    use crate::output::OutputSink;
    use crate::turn::TurnKind;

    struct NullOutput;
    impl OutputSink for NullOutput {
        fn emit_text_delta(&self, _: &str, _: &str) {}
        fn emit_thinking(&self, _: &str, _: &str) {}
        fn emit_tool_call(&self, _: &str, _: &str, _: &str) {}
        fn emit_tool_result(&self, _: &str, _: &str, _: bool, _: &str) {}
        fn emit_stream_start(&self, _: &str) {}
        fn emit_stream_end(&self, _: &str, _: usize, _: u64, _: u64, _: u64, _: u64) {}
        fn emit_error(&self, _: &str) {}
        fn emit_info(&self, _: &str) {}
    }

    struct NullProvider;
    #[async_trait::async_trait]
    impl LlmProvider for NullProvider {
        async fn stream(&self, _: &LlmRequest) -> Result<Receiver<LlmEvent>, ProviderError> {
            let (_tx, rx) = channel(1);
            Ok(rx)
        }
    }

    fn make_engine() -> AgentEngine {
        AgentEngine {
            provider: Arc::new(NullProvider),
            model: "test-model".to_string(),
            max_tokens: Some(4096),
            thinking: None,
            compat: ProviderCompat::anthropic_defaults(),
            system_prompt: String::new(),
            reasoning_effort: None,
            messages: vec![],
            total_usage: Default::default(),
            msg_id: String::new(),
            max_turns_per_run: Some(10),
            max_tool_call_malformed_turns: 3,
            max_tool_call_failure_turns: 3,
            tools: ToolRegistry::new(),
            tool_policy: Default::default(),
            confirmer: Arc::new(Mutex::new(ToolConfirmer::new(true, vec![]))),
            allow_list: vec![],
            hooks: None,
            session_manager: None,
            current_session: None,
            output: Arc::new(NullOutput),
            approval_manager: None,
            protocol_writer: None,
            compact_config: CompactConfig::default(),
            compact_state: CompactState::new(),
            compact_level: CompactLevel::default(),
            toon_enabled: false,
            plan_state: Default::default(),
            plan_active_flag: None,
            cache_detector: CacheBreakDetector::new(),
            commands: default_registry(),
        }
    }

    #[tokio::test]
    async fn handle_command_quit() {
        let mut engine = make_engine();
        let err = engine.handle_command("/quit").await.unwrap_err();
        assert!(matches!(err, AgentError::UserAborted));
    }

    #[tokio::test]
    async fn handle_command_exit_alias() {
        let mut engine = make_engine();
        let err = engine.handle_command("/exit").await.unwrap_err();
        assert!(matches!(err, AgentError::UserAborted));
    }

    #[tokio::test]
    async fn handle_command_unknown() {
        let mut engine = make_engine();
        let result = engine.handle_command("/nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn handle_command_clear() {
        let mut engine = make_engine();
        engine.messages.push(Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "hello".to_string(),
            }],
        ));
        assert_eq!(engine.messages.len(), 1);

        let result = engine.handle_command("/clear").await;
        let result = result.unwrap().expect("clear command should be handled");
        assert_eq!(result.turns, 0);
        assert!(engine.messages.is_empty());
        assert_eq!(engine.compact_state.last_input_tokens, 0);
    }

    #[tokio::test]
    async fn handle_command_with_args() {
        let mut engine = make_engine();
        let result = engine
            .handle_command("/help compact")
            .await
            .unwrap()
            .expect("help command should be handled");
        assert_eq!(result.turns, 0);
    }

    #[tokio::test]
    async fn handle_command_not_a_command() {
        let mut engine = make_engine();
        let result = engine.handle_command("hello world").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn run_intercepts_help_returns_zero_turns() {
        let mut engine = make_engine();
        let result = engine.run("/help", "msg-1").await.unwrap();
        assert_eq!(result.turns, 0);
        assert_eq!(result.usage.input_tokens, 0);
    }

    #[tokio::test]
    async fn run_intercepts_quit_returns_user_aborted() {
        let mut engine = make_engine();
        let err = engine.run("/quit", "msg-1").await.unwrap_err();
        assert!(matches!(err, AgentError::UserAborted));
    }

    #[test]
    fn slash_command_list_returns_all() {
        let engine = make_engine();
        let list = engine.slash_command_list();
        assert!(list.len() >= 4);
        let names: Vec<&str> = list.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"help"));
        assert!(names.contains(&"compact"));
        assert!(names.contains(&"clear"));
        assert!(names.contains(&"quit"));
    }

    // --- run() text-only behavior ---

    struct SingleResponseProvider;
    #[async_trait]
    impl LlmProvider for SingleResponseProvider {
        async fn stream(&self, _: &LlmRequest) -> Result<Receiver<LlmEvent>, ProviderError> {
            let (tx, rx) = channel(2);
            let _ = tx.send(LlmEvent::TextDelta("hello".to_string())).await;
            let _ = tx
                .send(LlmEvent::Done {
                    stop_reason: StopReason::EndTurn,
                    usage: TokenUsage::default(),
                })
                .await;
            Ok(rx)
        }
    }

    #[derive(Default)]
    struct RetryingToolProvider {
        requests: Mutex<Vec<LlmRequest>>,
    }

    #[async_trait]
    impl LlmProvider for RetryingToolProvider {
        async fn stream(&self, request: &LlmRequest) -> Result<Receiver<LlmEvent>, ProviderError> {
            let request_index = {
                let mut requests = self.requests.lock().unwrap();
                let request_index = requests.len();
                requests.push(request.clone());
                request_index
            };
            let (tx, rx) = channel(4);

            if request_index < 3 {
                let _ = tx.send(LlmEvent::TextDelta("I will retry".to_string())).await;
                let _ = tx
                    .send(LlmEvent::ToolUse {
                        id: format!("call-{request_index}"),
                        name: "FailingTool".to_string(),
                        input: json!({ "resource": "same" }),
                        extra: None,
                    })
                    .await;
                let _ = tx
                    .send(LlmEvent::Done {
                        stop_reason: StopReason::ToolUse,
                        usage: TokenUsage::default(),
                    })
                    .await;
            } else {
                let _ = tx
                    .send(LlmEvent::TextDelta(
                        "The tool is blocked; please resolve its error before retrying.".to_string(),
                    ))
                    .await;
                let _ = tx
                    .send(LlmEvent::Done {
                        stop_reason: StopReason::EndTurn,
                        usage: TokenUsage::default(),
                    })
                    .await;
            }

            Ok(rx)
        }
    }

    struct FailingTool {
        executions: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Tool for FailingTool {
        fn name(&self) -> &str {
            "FailingTool"
        }

        fn description(&self) -> &str {
            "always fails for loop-guard testing"
        }

        fn input_schema(&self) -> Value {
            json!({ "type": "object" })
        }

        fn is_concurrency_safe(&self, _: &Value) -> bool {
            true
        }

        async fn execute(&self, _: Value) -> ToolResult {
            self.executions.fetch_add(1, Ordering::SeqCst);
            ToolResult {
                content: "resource remains unavailable".to_string(),
                is_error: true,
            }
        }

        fn category(&self) -> ToolCategory {
            ToolCategory::Info
        }
    }

    fn make_engine_with_provider(provider: Arc<dyn LlmProvider>) -> AgentEngine {
        let mut engine = make_engine();
        engine.provider = provider;
        engine.max_turns_per_run = Some(1);
        engine
    }

    #[tokio::test]
    async fn run_treats_aion_files_marker_as_plain_text() {
        let mut engine = make_engine_with_provider(Arc::new(SingleResponseProvider));
        let input = "discuss [[AION_FILES]] and file.txt";
        let _ = engine.run(input, "msg-1").await;

        let user_msg = engine
            .messages
            .iter()
            .find(|m| m.role == Role::User)
            .expect("user message");
        assert_eq!(user_msg.content.len(), 1);
        match &user_msg.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, input),
            _ => panic!("expected plain text block, got {:?}", user_msg.content[0]),
        }
    }

    #[tokio::test]
    async fn visible_retry_text_does_not_reset_tool_failure_finalization() {
        let provider = Arc::new(RetryingToolProvider::default());
        let executions = Arc::new(AtomicUsize::new(0));
        let mut engine = make_engine_with_provider(provider.clone());
        engine.max_turns_per_run = Some(10);
        engine.tools.register(Box::new(FailingTool {
            executions: executions.clone(),
        }));

        let result = engine.run("finish the task", "msg-tool-loop").await.unwrap();

        assert_eq!(executions.load(Ordering::SeqCst), 3);
        assert_eq!(result.stop_reason, StopReason::EndTurn);
        assert_eq!(result.turns, 3);
        assert!(result.text.contains("tool is blocked"));

        let requests = provider.requests.lock().unwrap();
        assert_eq!(requests.len(), 4);
        assert!(requests[..3].iter().all(|request| !request.tools.is_empty()));
        assert!(requests[3].tools.is_empty());
        assert!(requests[2].messages.iter().any(|message| {
            message.content.iter().any(|block| {
                matches!(
                    block,
                    ContentBlock::ToolResult { content, is_error: true, .. }
                        if content.contains("same tool call has failed 2/3 times")
                )
            })
        }));
        assert!(requests[3].messages.iter().any(|message| {
            message.content.iter().any(|block| {
                matches!(
                    block,
                    ContentBlock::ToolResult { content, is_error: true, .. }
                        if content.contains("resource remains unavailable")
                )
            })
        }));
    }

    #[tokio::test]
    async fn run_with_blocks_accepts_image_blocks() {
        let mut engine = make_engine_with_provider(Arc::new(SingleResponseProvider));
        engine.compat.image_input = Some(ImageInputCapability::Supported);
        let blocks = vec![
            ContentBlock::Text {
                text: "look at this".to_string(),
            },
            ContentBlock::Image {
                image_url: ImageUrl {
                    url: "data:image/png;base64,abcd".to_string(),
                },
            },
        ];
        let _ = engine.run_with_blocks(blocks.clone(), "msg-2").await;

        let user_msg = engine
            .messages
            .iter()
            .find(|m| m.role == Role::User)
            .expect("user message");
        assert_eq!(user_msg.content.len(), 2);
        assert!(matches!(user_msg.content[0], ContentBlock::Text { .. }));
        assert!(matches!(user_msg.content[1], ContentBlock::Image { .. }));
    }

    #[tokio::test]
    async fn run_with_blocks_keeps_unsupported_image_in_history() {
        let mut engine = make_engine_with_provider(Arc::new(SingleResponseProvider));
        engine.compat.image_input = Some(ImageInputCapability::Unsupported);
        let blocks = vec![ContentBlock::Image {
            image_url: ImageUrl {
                url: "data:image/png;base64,abcd".to_owned(),
            },
        }];

        engine.run_with_blocks(blocks, "msg-3").await.unwrap();

        assert!(engine.messages.iter().any(|message| {
            message
                .content
                .iter()
                .any(|block| matches!(block, ContentBlock::Image { .. }))
        }));
    }

    #[test]
    fn build_request_omits_invalid_image_without_mutating_history() {
        let mut engine = make_engine();
        engine.compat.image_input = Some(ImageInputCapability::Supported);
        engine.messages.push(Message::new(
            Role::User,
            vec![ContentBlock::Image {
                image_url: ImageUrl {
                    url: "data:image/png;base64,!!!".to_owned(),
                },
            }],
        ));

        let request = engine.build_request(TurnKind::Normal);

        assert!(matches!(request.messages[0].content[0], ContentBlock::Text { .. }));
        assert!(matches!(engine.messages[0].content[0], ContentBlock::Image { .. }));
    }

    #[test]
    fn model_switch_projects_historical_snapshot_for_current_capability() {
        let mut engine = make_engine();
        engine.compat.image_input = Some(ImageInputCapability::Supported);
        engine.messages.push(Message::new(
            Role::User,
            vec![
                ContentBlock::Text {
                    text: "Attachment path: /tmp/image.png".to_owned(),
                },
                ContentBlock::Image {
                    image_url: ImageUrl {
                        url: "data:image/png;base64,abcd".to_owned(),
                    },
                },
            ],
        ));
        engine.model = "text-only-model".to_owned();
        engine.compat.image_input = Some(ImageInputCapability::Unsupported);

        let request = engine.build_request(TurnKind::Normal);

        assert_eq!(request.model, "text-only-model");
        assert!(
            request.messages[0]
                .content
                .iter()
                .all(|block| !matches!(block, ContentBlock::Image { .. }))
        );
        assert!(
            request.messages[0]
                .content
                .iter()
                .any(|block| matches!(block, ContentBlock::Text { text } if text.contains("/tmp/image.png")))
        );
        assert!(
            request.messages[0]
                .content
                .iter()
                .any(|block| matches!(block, ContentBlock::Text { text } if text.contains("does not support vision")))
        );
        assert_eq!(engine.messages.len(), 1);
        assert!(
            engine.messages[0]
                .content
                .iter()
                .any(|block| matches!(block, ContentBlock::Image { .. }))
        );
    }

    #[test]
    fn unknown_capability_uses_text_only_request_projection() {
        let mut engine = make_engine();
        engine.messages.push(Message::new(
            Role::User,
            vec![ContentBlock::Image {
                image_url: ImageUrl {
                    url: "data:image/png;base64,abcd".to_owned(),
                },
            }],
        ));

        let request = engine.build_request(TurnKind::Normal);

        assert!(matches!(request.messages[0].content[0], ContentBlock::Text { .. }));
        assert!(matches!(engine.messages[0].content[0], ContentBlock::Image { .. }));
    }
}

// ---------------------------------------------------------------------------
// Refactor unit tests — merge_tool_results() / TurnGuards
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests_loop_helpers {
    use aion_types::message::{ContentBlock, StopReason, TokenUsage};
    use serde_json::json;

    use super::{AgentError, build_assistant_content, merge_tool_results, tool_call_malformed_fingerprint};
    use crate::stream::StreamOutcome;
    use crate::tool_call::{
        DEFAULT_MAX_ALL_ERROR_TOOL_ROUNDS, DEFAULT_MAX_TOOL_CALL_FAILURE, ToolCallFailureFingerprint,
        ToolCallMalformedReason, tool_call_failure_fingerprint,
    };
    use crate::turn::{FinalizationReason, ToolLoopWarning, TurnGuardAction, TurnGuards, TurnKind, TurnOutcome};

    fn tool_use(id: &str, name: &str) -> ContentBlock {
        tool_use_with_input(id, name, json!({}))
    }

    fn tool_use_with_input(id: &str, name: &str, input: serde_json::Value) -> ContentBlock {
        ContentBlock::ToolUse {
            id: id.to_string(),
            name: name.to_string(),
            input,
            extra: None,
        }
    }

    fn failed_exec_fingerprint() -> Option<ToolCallFailureFingerprint> {
        tool_call_failure_fingerprint(&[tool_use("call", "ExecCommand")])
    }

    fn executed_result(id: &str) -> ContentBlock {
        ContentBlock::ToolResult {
            tool_use_id: id.to_string(),
            content: format!("ok:{id}"),
            is_error: false,
        }
    }

    /// Mixed malformed + executable calls must re-interleave so each result
    /// lands at its originating call's index, with executed results consumed
    /// in order for the non-malformed slots.
    #[test]
    fn merge_interleaves_malformed_and_executed_in_call_order() {
        let calls = vec![tool_use("bad", ""), tool_use("ok1", "Read"), tool_use("ok2", "Glob")];
        let reasons = vec![Some(ToolCallMalformedReason::EmptyFunctionName), None, None];
        let executed = vec![executed_result("ok1"), executed_result("ok2")];
        let modifiers = vec![None, None];

        let denied = vec![None, None, None];
        let (results, mods) = merge_tool_results(&calls, &reasons, &denied, executed, modifiers);

        assert_eq!(results.len(), 3);
        // Slot 0: synthetic malformed error result for "bad".
        assert!(matches!(
            &results[0],
            ContentBlock::ToolResult { tool_use_id, is_error: true, .. } if tool_use_id == "bad"
        ));
        // Slots 1,2: executed results, consumed in order.
        assert!(matches!(
            &results[1],
            ContentBlock::ToolResult { tool_use_id, is_error: false, .. } if tool_use_id == "ok1"
        ));
        assert!(matches!(
            &results[2],
            ContentBlock::ToolResult { tool_use_id, is_error: false, .. } if tool_use_id == "ok2"
        ));
        assert_eq!(mods.len(), 3);
    }

    #[test]
    fn merge_all_malformed_needs_no_executed_results() {
        let calls = vec![tool_use("bad1", ""), tool_use("bad2", "")];
        let reasons = vec![
            Some(ToolCallMalformedReason::EmptyFunctionName),
            Some(ToolCallMalformedReason::EmptyFunctionName),
        ];

        let denied = vec![None, None];
        let (results, mods) = merge_tool_results(&calls, &reasons, &denied, Vec::new(), Vec::new());

        assert_eq!(results.len(), 2);
        assert!(
            results
                .iter()
                .all(|r| matches!(r, ContentBlock::ToolResult { is_error: true, .. }))
        );
        assert!(mods.iter().all(Option::is_none));
    }

    #[test]
    fn merge_interleaves_policy_denial_without_executing_it() {
        let calls = vec![tool_use("denied", "ExecCommand"), tool_use("ok", "Read")];
        let reasons = vec![None, None];
        let denied = vec![Some("ExecCommand".to_string()), None];
        let executed = vec![executed_result("ok")];

        let (results, mods) = merge_tool_results(&calls, &reasons, &denied, executed, vec![None]);

        assert!(matches!(
            &results[0],
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error: true,
            } if tool_use_id == "denied" && content.contains("not available")
        ));
        assert!(matches!(
            &results[1],
            ContentBlock::ToolResult { tool_use_id, is_error: false, .. } if tool_use_id == "ok"
        ));
        assert!(mods.iter().all(Option::is_none));
    }

    #[test]
    fn turn_budget_reached_respects_limit_and_none() {
        let mut guards = TurnGuards::new(Some(2), 3, DEFAULT_MAX_TOOL_CALL_FAILURE);
        assert_eq!(guards.turn_budget_reached(), None);
        guards.record_counted_turn();
        guards.record_counted_turn();
        assert_eq!(guards.turn_budget_reached(), Some(2));

        // No limit configured → never reached.
        let mut unlimited = TurnGuards::new(None, 3, DEFAULT_MAX_TOOL_CALL_FAILURE);
        for _ in 0..1_000 {
            unlimited.record_counted_turn();
        }
        assert_eq!(unlimited.turn_budget_reached(), None);
    }

    #[test]
    fn after_tool_round_warns_then_finalizes_repeated_exact_failure() {
        let mut guards = TurnGuards::new(Some(100), 3, DEFAULT_MAX_TOOL_CALL_FAILURE);
        assert!(matches!(
            guards.after_tool_round(None, failed_exec_fingerprint(), true),
            TurnGuardAction::Continue
        ));
        assert!(matches!(
            guards.after_tool_round(None, failed_exec_fingerprint(), true),
            TurnGuardAction::Warn(ToolLoopWarning::ExactFailure { count: 2, limit: 3 })
        ));
        assert!(matches!(
            guards.after_tool_round(None, failed_exec_fingerprint(), true),
            TurnGuardAction::Finalize(FinalizationReason::ToolFailure)
        ));
    }

    #[test]
    fn after_tool_round_resets_tool_call_failure_streak_on_success() {
        let mut guards = TurnGuards::new(Some(100), 3, DEFAULT_MAX_TOOL_CALL_FAILURE);
        assert!(matches!(
            guards.after_tool_round(None, failed_exec_fingerprint(), true),
            TurnGuardAction::Continue
        ));
        // A non-error tool round resets the streak.
        assert!(matches!(
            guards.after_tool_round(None, None, false),
            TurnGuardAction::Continue
        ));
        assert_eq!(guards.tool_call_failure_count(), 0);
        assert_eq!(guards.all_error_tool_round_count(), 0);
        // So a single subsequent tool-call-failure round must not trip the breaker.
        assert!(matches!(
            guards.after_tool_round(None, failed_exec_fingerprint(), true),
            TurnGuardAction::Continue
        ));
    }

    #[test]
    fn after_tool_round_does_not_trip_exact_breaker_for_different_tool_inputs() {
        let mut guards = TurnGuards::new(Some(100), 3, DEFAULT_MAX_TOOL_CALL_FAILURE);

        for index in 0..2 {
            let fingerprint = tool_call_failure_fingerprint(&[tool_use_with_input(
                &format!("call-{index}"),
                "ExecCommand",
                json!({ "cmd": format!("command-{index}") }),
            )]);

            assert!(matches!(
                guards.after_tool_round(None, fingerprint, true),
                TurnGuardAction::Continue
            ));
            assert_eq!(guards.tool_call_failure_count(), 1);
        }
    }

    #[test]
    fn after_tool_round_warns_then_finalizes_variable_all_error_rounds() {
        let mut guards = TurnGuards::new(Some(100), 3, DEFAULT_MAX_TOOL_CALL_FAILURE);

        for index in 1..=DEFAULT_MAX_ALL_ERROR_TOOL_ROUNDS {
            let fingerprint = tool_call_failure_fingerprint(&[tool_use_with_input(
                &format!("call-{index}"),
                "ExecCommand",
                json!({ "cmd": format!("unique-command-{index}") }),
            )]);
            let action = guards.after_tool_round(None, fingerprint, true);

            match index {
                3 => assert!(matches!(
                    action,
                    TurnGuardAction::Warn(ToolLoopWarning::AllErrorRounds { count: 3, limit: 8 })
                )),
                DEFAULT_MAX_ALL_ERROR_TOOL_ROUNDS => assert!(matches!(
                    action,
                    TurnGuardAction::Finalize(FinalizationReason::ToolFailure)
                )),
                _ => assert!(matches!(action, TurnGuardAction::Continue)),
            }
        }
    }

    #[test]
    fn after_tool_round_warns_then_finalizes_alternating_cycle() {
        let mut guards = TurnGuards::new(Some(100), 3, DEFAULT_MAX_TOOL_CALL_FAILURE);

        for index in 0..6 {
            let variant = index % 2;
            let fingerprint = tool_call_failure_fingerprint(&[tool_use_with_input(
                &format!("call-{index}"),
                "ExecCommand",
                json!({ "cmd": format!("command-{variant}") }),
            )]);
            let action = guards.after_tool_round(None, fingerprint, true);

            match index {
                2 => assert!(matches!(
                    action,
                    TurnGuardAction::Warn(ToolLoopWarning::AllErrorRounds { .. })
                )),
                3 => assert!(matches!(
                    action,
                    TurnGuardAction::Warn(ToolLoopWarning::Cycle {
                        period: 2,
                        repetitions: 2,
                        limit: 3,
                    })
                )),
                5 => assert!(matches!(
                    action,
                    TurnGuardAction::Finalize(FinalizationReason::ToolFailure)
                )),
                _ => assert!(matches!(action, TurnGuardAction::Continue)),
            }
        }
    }

    #[test]
    fn zero_failure_limit_disables_all_tool_failure_guards() {
        let mut guards = TurnGuards::new(Some(100), 3, 0);

        for _ in 0..20 {
            assert!(matches!(
                guards.after_tool_round(None, failed_exec_fingerprint(), true),
                TurnGuardAction::Continue
            ));
        }
    }

    #[test]
    fn after_tool_round_requests_finalize_when_budget_is_exhausted() {
        let mut guards = TurnGuards::new(Some(1), 3, DEFAULT_MAX_TOOL_CALL_FAILURE);
        guards.record_counted_turn();
        assert!(matches!(
            guards.after_tool_round(None, None, false),
            TurnGuardAction::Finalize(FinalizationReason::TurnBudget)
        ));
    }

    #[test]
    fn after_tool_round_stop_breaker_takes_priority_over_finalize() {
        let mut guards = TurnGuards::new(Some(1), 1, DEFAULT_MAX_TOOL_CALL_FAILURE);
        guards.record_counted_turn();

        let calls = vec![tool_use("bad", "")];
        let reasons = vec![Some(ToolCallMalformedReason::EmptyFunctionName)];
        let fingerprint = tool_call_malformed_fingerprint(&calls, &reasons);

        assert!(matches!(
            guards.after_tool_round(fingerprint, None, false),
            TurnGuardAction::Stop(AgentError::ToolCallMalformed { count: 1, limit: 1 })
        ));
    }

    #[test]
    fn turn_kind_finalization_has_control_prompt_and_disables_tools() {
        assert!(TurnKind::Normal.control_prompt().is_none());
        assert!(!TurnKind::Normal.disable_tools());
        assert_eq!(TurnKind::Normal.diagnostic_phase(), "normal");

        let kind = TurnKind::Finalization(FinalizationReason::TurnBudget);
        assert!(kind.disable_tools());
        assert_eq!(kind.diagnostic_phase(), "turn_budget_finalization");
        assert!(
            kind.control_prompt()
                .expect("finalization must have a control prompt")
                .contains("Do not call any more tools")
        );
    }

    #[test]
    fn turn_kind_max_tokens_prompt_names_truncation() {
        let kind = TurnKind::Finalization(FinalizationReason::MaxTokens);
        let prompt = kind
            .control_prompt()
            .expect("max token continuation must have a prompt");

        assert_eq!(kind.diagnostic_phase(), "max_tokens_finalization");
        assert!(prompt.contains("previous response was cut off"));
        assert!(prompt.contains("Finish the answer"));
    }

    #[test]
    fn turn_kind_tool_failure_prompt_explains_blocker_and_disables_tools() {
        let kind = TurnKind::Finalization(FinalizationReason::ToolFailure);
        let prompt = kind
            .control_prompt()
            .expect("tool failure finalization must have a prompt");

        assert!(kind.disable_tools());
        assert_eq!(kind.diagnostic_phase(), "tool_failure_finalization");
        assert!(prompt.contains("Do not call any more tools"));
        assert!(prompt.contains("concrete blocker"));
        assert!(prompt.contains("Do not mention internal retry counters"));
    }

    #[test]
    fn turn_kind_empty_final_prompt_requests_visible_answer() {
        let kind = TurnKind::Finalization(FinalizationReason::EmptyFinal);
        let prompt = kind.control_prompt().expect("empty final nudge must have a prompt");

        assert_eq!(kind.diagnostic_phase(), "empty_final_retry");
        assert!(prompt.contains("visible answer text"));
        assert!(prompt.contains("Do not send reasoning only"));
    }

    fn stream_outcome(assistant_text: &str, stop_reason: StopReason, tool_calls: Vec<ContentBlock>) -> StreamOutcome {
        StreamOutcome {
            assistant_text: assistant_text.to_string(),
            thinking_text: String::new(),
            thinking_signature: None,
            provider_items: Vec::new(),
            tool_calls,
            stop_reason,
            usage: TokenUsage::default(),
        }
    }

    #[test]
    fn turn_outcome_classifies_tool_round_before_final_text() {
        let outcome = stream_outcome(
            "I will inspect this.",
            StopReason::EndTurn,
            vec![tool_use("call-1", "Read")],
        );

        assert!(matches!(TurnOutcome::from_stream(outcome), TurnOutcome::ToolRound(_)));
    }

    #[test]
    fn assistant_content_keeps_provider_items_before_visible_content() {
        let mut outcome = stream_outcome("Done", StopReason::ToolUse, vec![tool_use("call-1", "Read")]);
        outcome.provider_items.push(ContentBlock::ProviderItem {
            provider: "openai_responses".to_string(),
            item: json!({"id": "rs_1", "type": "reasoning"}),
        });

        let content = build_assistant_content(&outcome);

        assert!(matches!(content[0], ContentBlock::ProviderItem { .. }));
        assert!(matches!(content[1], ContentBlock::Text { .. }));
        assert!(matches!(content[2], ContentBlock::ToolUse { .. }));
    }

    #[test]
    fn turn_outcome_classifies_visible_end_turn_as_final() {
        let outcome = stream_outcome("Done", StopReason::EndTurn, Vec::new());

        assert!(matches!(TurnOutcome::from_stream(outcome), TurnOutcome::Final(_)));
    }

    #[test]
    fn turn_outcome_classifies_max_tokens_as_truncated_even_with_text() {
        let outcome = stream_outcome("I will now write the file", StopReason::MaxTokens, Vec::new());

        assert!(matches!(TurnOutcome::from_stream(outcome), TurnOutcome::Truncated(_)));
    }

    #[test]
    fn turn_outcome_classifies_empty_end_turn_as_empty_final() {
        let outcome = stream_outcome("   ", StopReason::EndTurn, Vec::new());

        assert!(matches!(TurnOutcome::from_stream(outcome), TurnOutcome::EmptyFinal(_)));
    }
}

#[cfg(test)]
mod tests_tool_policy_enforcement {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use aion_protocol::events::ToolCategory;
    use aion_providers::error::ProviderError;
    use aion_providers::provider::LlmProvider;
    use aion_tools::Tool;
    use aion_tools::registry::ToolRegistry;
    use aion_types::llm::{LlmEvent, LlmRequest};
    use aion_types::message::{ContentBlock, ImageInputCapability};
    use aion_types::tool::ToolResult;
    use async_trait::async_trait;
    use serde_json::{Value, json};

    use super::{AgentEngine, CacheBreakDetector, CompactLevel, CompactState, ProviderCompat};
    use crate::commands::default_registry;
    use crate::confirm::ToolConfirmer;
    use crate::output::OutputSink;
    use crate::tool_policy::ToolPolicy;
    use crate::turn::TurnKind;

    struct NullOutput;

    impl OutputSink for NullOutput {
        fn emit_text_delta(&self, _: &str, _: &str) {}
        fn emit_thinking(&self, _: &str, _: &str) {}
        fn emit_tool_call(&self, _: &str, _: &str, _: &str) {}
        fn emit_tool_result(&self, _: &str, _: &str, _: bool, _: &str) {}
        fn emit_stream_start(&self, _: &str) {}
        fn emit_stream_end(&self, _: &str, _: usize, _: u64, _: u64, _: u64, _: u64) {}
        fn emit_error(&self, _: &str) {}
        fn emit_info(&self, _: &str) {}
    }

    struct NullProvider;

    #[async_trait]
    impl LlmProvider for NullProvider {
        async fn stream(&self, _: &LlmRequest) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            Ok(rx)
        }
    }

    struct CountingTool {
        name: &'static str,
        executions: Arc<AtomicUsize>,
        requires_image_input: bool,
    }

    #[async_trait]
    impl Tool for CountingTool {
        fn name(&self) -> &str {
            self.name
        }

        fn description(&self) -> &str {
            "count executions"
        }

        fn input_schema(&self) -> Value {
            json!({"type": "object"})
        }

        fn is_concurrency_safe(&self, _: &Value) -> bool {
            true
        }

        async fn execute(&self, _: Value) -> ToolResult {
            self.executions.fetch_add(1, Ordering::SeqCst);
            ToolResult {
                content: "ok".to_string(),
                is_error: false,
            }
        }

        fn requires_image_input(&self) -> bool {
            self.requires_image_input
        }

        fn category(&self) -> ToolCategory {
            ToolCategory::Info
        }
    }

    fn make_engine(read_executions: Arc<AtomicUsize>, exec_executions: Arc<AtomicUsize>) -> AgentEngine {
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(CountingTool {
            name: "Read",
            executions: read_executions,
            requires_image_input: false,
        }));
        tools.register(Box::new(CountingTool {
            name: "ExecCommand",
            executions: exec_executions,
            requires_image_input: false,
        }));

        AgentEngine {
            provider: Arc::new(NullProvider),
            compat: ProviderCompat::anthropic_defaults(),
            thinking: None,
            system_prompt: String::new(),
            model: "test-model".to_string(),
            reasoning_effort: None,
            messages: Vec::new(),
            total_usage: Default::default(),
            msg_id: "test-message".to_string(),
            max_tokens: Some(4096),
            max_turns_per_run: Some(10),
            max_tool_call_malformed_turns: 3,
            max_tool_call_failure_turns: 3,
            tools,
            tool_policy: ToolPolicy::allow_only(["Read"]),
            confirmer: Arc::new(Mutex::new(ToolConfirmer::new(true, vec![]))),
            allow_list: vec![],
            hooks: None,
            session_manager: None,
            current_session: None,
            output: Arc::new(NullOutput),
            approval_manager: None,
            protocol_writer: None,
            compact_config: Default::default(),
            compact_state: CompactState::new(),
            compact_level: CompactLevel::default(),
            toon_enabled: false,
            plan_state: Default::default(),
            plan_active_flag: None,
            cache_detector: CacheBreakDetector::new(),
            commands: default_registry(),
        }
    }

    fn tool_use(id: &str, name: &str) -> ContentBlock {
        ContentBlock::ToolUse {
            id: id.to_string(),
            name: name.to_string(),
            input: json!({}),
            extra: None,
        }
    }

    #[test]
    fn build_request_only_advertises_policy_allowed_tools() {
        let mut engine = make_engine(Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));

        let request = engine.build_request(TurnKind::Normal);
        let tool_names: Vec<_> = request.tools.iter().map(|tool| tool.name.as_str()).collect();

        assert_eq!(tool_names, vec!["Read"]);
    }

    #[test]
    fn build_request_only_advertises_image_tool_to_supported_models() {
        let image_executions = Arc::new(AtomicUsize::new(0));
        let mut engine = make_engine(Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        engine.tools.register(Box::new(CountingTool {
            name: "ViewImage",
            executions: image_executions,
            requires_image_input: true,
        }));
        engine.tool_policy = ToolPolicy::allow_only(["Read", "ViewImage"]);
        engine.compat.image_input = Some(ImageInputCapability::Unsupported);

        let text_only_request = engine.build_request(TurnKind::Normal);
        assert!(text_only_request.tools.iter().all(|tool| tool.name != "ViewImage"));

        engine.compat.image_input = Some(ImageInputCapability::Supported);
        let vision_request = engine.build_request(TurnKind::Normal);
        assert!(vision_request.tools.iter().any(|tool| tool.name == "ViewImage"));
    }

    #[tokio::test]
    async fn execute_tool_round_rejects_denied_tools_before_execution() {
        let read_executions = Arc::new(AtomicUsize::new(0));
        let exec_executions = Arc::new(AtomicUsize::new(0));
        let mut engine = make_engine(Arc::clone(&read_executions), Arc::clone(&exec_executions));
        let calls = vec![tool_use("denied", "ExecCommand"), tool_use("allowed", "Read")];

        let output = engine.execute_tool_round(&calls).await.unwrap();

        assert_eq!(exec_executions.load(Ordering::SeqCst), 0);
        assert_eq!(read_executions.load(Ordering::SeqCst), 1);
        assert!(matches!(
            &output.tool_results[0],
            ContentBlock::ToolResult { is_error: true, content, .. } if content.contains("not available")
        ));
        assert!(matches!(
            &output.tool_results[1],
            ContentBlock::ToolResult { is_error: false, .. }
        ));
        assert!(!output.all_tool_results_error);
    }
}
