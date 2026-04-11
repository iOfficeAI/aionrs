use std::path::PathBuf;

/// Runtime state for Plan Mode.
///
/// Tracks whether the agent is currently in plan mode, the tool allow-list
/// that was active before plan mode was entered (for restoration on exit),
/// and an optional path to the plan file being composed.
#[derive(Debug, Clone, Default)]
pub struct PlanState {
    /// Whether plan mode is currently active.
    pub is_active: bool,

    /// The tool allow-list that was in effect before entering plan mode.
    /// Restored when the agent exits plan mode.
    pub pre_plan_allow_list: Vec<String>,

    /// Path to the plan file, set when entering plan mode.
    pub plan_file_path: Option<PathBuf>,
}

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
    fn default_has_no_plan_file() {
        let state = PlanState::default();
        assert!(state.plan_file_path.is_none());
    }

    #[test]
    fn can_set_active_with_allow_list() {
        let state = PlanState {
            is_active: true,
            pre_plan_allow_list: vec!["Read".into(), "Bash".into()],
            plan_file_path: Some(PathBuf::from("/tmp/plan.md")),
        };
        assert!(state.is_active);
        assert_eq!(state.pre_plan_allow_list, vec!["Read", "Bash"]);
        assert_eq!(
            state.plan_file_path.as_deref(),
            Some(std::path::Path::new("/tmp/plan.md"))
        );
    }

    #[test]
    fn clone_produces_independent_copy() {
        let original = PlanState {
            is_active: true,
            pre_plan_allow_list: vec!["Grep".into()],
            plan_file_path: None,
        };
        let mut cloned = original.clone();
        cloned.is_active = false;
        cloned.pre_plan_allow_list.push("Read".into());

        // Original unchanged
        assert!(original.is_active);
        assert_eq!(original.pre_plan_allow_list, vec!["Grep"]);
    }
}
