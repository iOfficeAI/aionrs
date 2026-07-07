use super::*;

#[cfg(test)]
mod phase7_tests {
    use super::{ForkOverrides, SubAgentConfig, build_tool_registry};

    #[test]
    fn tc_7_1_fork_overrides_default_values() {
        let o = ForkOverrides::default();
        assert!(o.model.is_none());
        assert!(o.effort.is_none());
        assert!(o.allowed_tools.is_empty());
    }

    #[test]
    fn tc_7_40_build_tool_registry_empty_allowed_registers_all() {
        let registry = build_tool_registry(&[], &std::env::temp_dir(), &[]);
        for name in &["Read", "Write", "Edit", "ExecCommand", "Grep", "Glob"] {
            assert!(registry.get(name).is_some(), "tool '{name}' should be registered");
        }
    }

    #[test]
    fn tc_7_43_build_tool_registry_filters_to_allowed() {
        let allowed = vec!["ExecCommand".to_string(), "Read".to_string()];
        let registry = build_tool_registry(&allowed, &std::env::temp_dir(), &[]);
        assert!(registry.get("ExecCommand").is_some());
        assert!(registry.get("Read").is_some());
        assert!(registry.get("Write").is_none());
    }

    #[test]
    fn tc_7_sub_agent_config_original_fields_intact() {
        let config = SubAgentConfig {
            name: "test-agent".to_string(),
            prompt: "do the task".to_string(),
            max_turns: 5,
            max_tokens: 1024,
            system_prompt: Some("you are helpful".to_string()),
        };
        assert_eq!(config.name, "test-agent");
        assert_eq!(config.max_turns, 5);
    }
}
