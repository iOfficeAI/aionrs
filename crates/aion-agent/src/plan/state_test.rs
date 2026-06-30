use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_inactive() {
        let state = PlanState::default();
        assert!(!state.is_active);
    }

    #[test]
    fn default_has_empty_allow_list() {
        let state = PlanState::default();
        assert!(state.pre_plan_allow_list.is_empty());
    }

    #[test]
    fn can_set_active_with_allow_list() {
        let state = PlanState {
            is_active: true,
            pre_plan_allow_list: vec!["Read".into(), "ExecCommand".into()],
        };
        assert!(state.is_active);
        assert_eq!(state.pre_plan_allow_list, vec!["Read", "ExecCommand"]);
    }

    #[test]
    fn clone_produces_independent_copy() {
        let original = PlanState {
            is_active: true,
            pre_plan_allow_list: vec!["Grep".into()],
        };
        let mut cloned = original.clone();
        cloned.is_active = false;
        cloned.pre_plan_allow_list.push("Read".into());

        // Original unchanged
        assert!(original.is_active);
        assert_eq!(original.pre_plan_allow_list, vec!["Grep"]);
    }
}
