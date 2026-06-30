use super::*;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ExecutionContext, LoadedFrom, SkillMetadata, SkillSource};

    fn make_skill(name: &str) -> SkillMetadata {
        SkillMetadata {
            name: name.to_string(),
            display_name: None,
            description: String::new(),
            has_user_specified_description: false,
            allowed_tools: vec![],
            argument_hint: None,
            argument_names: vec![],
            when_to_use: None,
            version: None,
            model: None,
            disable_model_invocation: false,
            user_invocable: true,
            execution_context: ExecutionContext::Inline,
            agent: None,
            effort: None,
            shell: None,
            paths: vec![],
            hooks_raw: None,
            source: SkillSource::User,
            loaded_from: LoadedFrom::Skills,
            content: String::new(),
            content_length: 0,
            skill_root: None,
        }
    }

    // P5-1: parse exact match
    #[test]
    fn p5_1_parse_exact() {
        let rule = PermissionRule::parse("commit");
        assert_eq!(rule, PermissionRule::Exact("commit".to_string()));
        assert!(rule.matches("commit"));
        assert!(!rule.matches("commit-all"));
    }

    // P5-2: parse prefix match
    #[test]
    fn p5_2_parse_prefix() {
        let rule = PermissionRule::parse("db:*");
        assert_eq!(rule, PermissionRule::Prefix("db:".to_string()));
        assert!(rule.matches("db:migrate"));
        assert!(rule.matches("db:seed"));
        assert!(!rule.matches("database"));
    }

    // P5-3: deny rule blocks skill
    #[test]
    fn p5_3_deny_blocks_skill() {
        let checker = SkillPermissionChecker::new(vec!["dangerous".to_string()], vec![], false);
        let skill = make_skill("dangerous");
        assert_eq!(checker.check(&skill), SkillPermission::Deny);
    }

    // P5-4: allow rule passes skill
    #[test]
    fn p5_4_allow_passes_skill() {
        let mut skill = make_skill("commit");
        // Give it hooks so safe-properties wouldn't fire
        skill.hooks_raw = Some(serde_json::json!({}));
        let checker = SkillPermissionChecker::new(vec![], vec!["commit".to_string()], false);
        assert_eq!(checker.check(&skill), SkillPermission::Allow);
    }

    // P5-5: deny takes priority over allow
    #[test]
    fn p5_5_deny_over_allow() {
        let mut skill = make_skill("commit");
        skill.hooks_raw = Some(serde_json::json!({}));
        let checker = SkillPermissionChecker::new(vec!["commit".to_string()], vec!["commit".to_string()], false);
        assert_eq!(checker.check(&skill), SkillPermission::Deny);
    }

    // P5-6: no hooks, no allowed_tools → Allow (safe-properties)
    #[test]
    fn p5_6_safe_properties_allow() {
        let checker = SkillPermissionChecker::new(vec![], vec![], false);
        let skill = make_skill("read-only");
        assert_eq!(checker.check(&skill), SkillPermission::Allow);
    }

    // P5-7: has hooks → Ask
    #[test]
    fn p5_7_hooks_require_ask() {
        let mut skill = make_skill("hooked");
        skill.hooks_raw = Some(serde_json::json!({ "pre": "echo hi" }));
        let checker = SkillPermissionChecker::new(vec![], vec![], false);
        assert!(matches!(checker.check(&skill), SkillPermission::Ask { .. }));
    }

    // P5-8: has allowed_tools → Ask
    #[test]
    fn p5_8_allowed_tools_require_ask() {
        let mut skill = make_skill("tooled");
        skill.allowed_tools = vec!["ExecCommand".to_string()];
        let checker = SkillPermissionChecker::new(vec![], vec![], false);
        assert!(matches!(checker.check(&skill), SkillPermission::Ask { .. }));
    }

    // P5-9: no rule match + has hooks → Ask
    #[test]
    fn p5_9_no_match_with_hooks_ask() {
        let mut skill = make_skill("unknown");
        skill.hooks_raw = Some(serde_json::json!({}));
        let checker = SkillPermissionChecker::new(vec!["other".to_string()], vec!["other".to_string()], false);
        assert!(matches!(checker.check(&skill), SkillPermission::Ask { .. }));
    }

    // P5-10: auto_approve converts Ask → Allow (but deny still blocks)
    #[test]
    fn p5_10_auto_approve_allows_but_not_deny() {
        let mut skill_hooked = make_skill("hooked");
        skill_hooked.hooks_raw = Some(serde_json::json!({}));

        let mut skill_denied = make_skill("denied");
        skill_denied.hooks_raw = Some(serde_json::json!({}));

        let checker = SkillPermissionChecker::new(
            vec!["denied".to_string()],
            vec![],
            true, // auto_approve
        );

        // hooked skill: would be Ask, but auto_approve converts to Allow
        assert_eq!(checker.check(&skill_hooked), SkillPermission::Allow);
        // denied skill: deny always wins
        assert_eq!(checker.check(&skill_denied), SkillPermission::Deny);
    }

    // P5-13: prefix boundary — "db:*" does not match "database"
    #[test]
    fn p5_13_prefix_boundary() {
        let rule = PermissionRule::parse("db:*");
        assert!(!rule.matches("database"));
        assert!(!rule.matches("db"));
        assert!(rule.matches("db:migrate"));
        assert!(rule.matches("db:"));
    }

    // P5-15: empty deny/allow → all go through safe-properties
    #[test]
    fn p5_15_empty_rules_safe_properties() {
        let checker = SkillPermissionChecker::new(vec![], vec![], false);

        // Safe skill (no hooks, no allowed_tools) → Allow
        let safe = make_skill("safe");
        assert_eq!(checker.check(&safe), SkillPermission::Allow);

        // Unsafe skill (has hooks) → Ask
        let mut unsafe_skill = make_skill("unsafe");
        unsafe_skill.hooks_raw = Some(serde_json::json!({}));
        assert!(matches!(checker.check(&unsafe_skill), SkillPermission::Ask { .. }));
    }

    // Reason string mentions hooks
    #[test]
    fn ask_reason_mentions_hooks() {
        let mut skill = make_skill("hooked");
        skill.hooks_raw = Some(serde_json::json!({}));
        let checker = SkillPermissionChecker::new(vec![], vec![], false);
        if let SkillPermission::Ask { reason } = checker.check(&skill) {
            assert!(reason.contains("hooks"), "reason should mention hooks: {reason}");
        } else {
            panic!("expected Ask");
        }
    }

    // Reason string mentions allowed-tools
    #[test]
    fn ask_reason_mentions_allowed_tools() {
        let mut skill = make_skill("tooled");
        skill.allowed_tools = vec!["ExecCommand".to_string()];
        let checker = SkillPermissionChecker::new(vec![], vec![], false);
        if let SkillPermission::Ask { reason } = checker.check(&skill) {
            assert!(
                reason.contains("allowed-tools") || reason.contains("ExecCommand"),
                "reason should mention tool: {reason}"
            );
        } else {
            panic!("expected Ask");
        }
    }
}
