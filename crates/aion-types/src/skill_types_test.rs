use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_modifier_default_has_no_plan_transition() {
        let cm = ContextModifier::default();
        assert!(cm.plan_mode_transition.is_none());
        assert!(cm.is_empty());
    }

    #[test]
    fn context_modifier_with_plan_transition_is_not_empty() {
        let cm = ContextModifier {
            plan_mode_transition: Some(PlanModeTransition::Enter),
            ..Default::default()
        };
        assert!(!cm.is_empty());
    }

    #[test]
    fn plan_mode_transition_enter_debug() {
        let t = PlanModeTransition::Enter;
        let dbg = format!("{:?}", t);
        assert!(dbg.contains("Enter"));
    }

    #[test]
    fn plan_mode_transition_exit_with_content() {
        let t = PlanModeTransition::Exit {
            plan_content: Some("# My Plan".to_string()),
        };
        let dbg = format!("{:?}", t);
        assert!(dbg.contains("Exit"));
        assert!(dbg.contains("My Plan"));
    }

    #[test]
    fn plan_mode_transition_exit_without_content() {
        let t = PlanModeTransition::Exit { plan_content: None };
        let dbg = format!("{:?}", t);
        assert!(dbg.contains("Exit"));
        assert!(dbg.contains("None"));
    }

    #[test]
    fn plan_mode_transition_equality() {
        assert_eq!(PlanModeTransition::Enter, PlanModeTransition::Enter);
        assert_ne!(
            PlanModeTransition::Enter,
            PlanModeTransition::Exit { plan_content: None }
        );
        assert_eq!(
            PlanModeTransition::Exit {
                plan_content: Some("x".into())
            },
            PlanModeTransition::Exit {
                plan_content: Some("x".into())
            }
        );
    }

    #[test]
    fn context_modifier_existing_fields_unaffected() {
        // Verify that adding plan_mode_transition doesn't break existing usage
        let cm = ContextModifier {
            model: Some("test-model".into()),
            effort: Some(EffortLevel::High),
            allowed_tools: vec!["ExecCommand".into()],
            plan_mode_transition: None,
        };
        assert!(!cm.is_empty());
        assert_eq!(cm.model.as_deref(), Some("test-model"));
        assert_eq!(cm.effort, Some(EffortLevel::High));
        assert_eq!(cm.allowed_tools, vec!["ExecCommand".to_string()]);
        assert!(cm.plan_mode_transition.is_none());
    }
}
