use super::*;

#[cfg(test)]
mod phase7_tests {
    use super::{ForkOverrides, SubAgentConfig, ToolPolicy, build_tool_registry, effective_child_tool_policy};

    #[test]
    fn tc_7_1_fork_overrides_default_values() {
        let o = ForkOverrides::default();
        assert!(o.model.is_none());
        assert!(o.effort.is_none());
        assert!(o.allowed_tools.is_empty());
    }

    #[test]
    fn tc_7_40_build_tool_registry_unrestricted_registers_all() {
        let registry = build_tool_registry(&ToolPolicy::Unrestricted, &std::env::temp_dir(), &[]);
        for name in &["Read", "Write", "Edit", "ExecCommand", "Grep", "Glob"] {
            assert!(registry.get(name).is_some(), "tool '{name}' should be registered");
        }
    }

    #[test]
    fn tc_7_43_build_tool_registry_filters_to_policy() {
        let policy = ToolPolicy::allow_only(["ExecCommand", "Read"]);
        let registry = build_tool_registry(&policy, &std::env::temp_dir(), &[]);
        assert!(registry.get("ExecCommand").is_some());
        assert!(registry.get("Read").is_some());
        assert!(registry.get("Write").is_none());
    }

    #[test]
    fn fork_overrides_can_only_narrow_parent_policy() {
        let parent = ToolPolicy::allow_only(["Read", "Grep", "Spawn"]);
        let allowed_tools = vec!["Read".to_string(), "ExecCommand".to_string()];

        let child = effective_child_tool_policy(&parent, &allowed_tools);

        assert!(child.allows("Read"));
        assert!(!child.allows("Grep"));
        assert!(!child.allows("ExecCommand"));
        assert!(!child.allows("Write"));
    }

    #[test]
    fn empty_fork_override_inherits_parent_policy() {
        let parent = ToolPolicy::allow_only(["Read", "Grep", "Spawn"]);

        let child = effective_child_tool_policy(&parent, &[]);

        assert_eq!(child, parent);
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
