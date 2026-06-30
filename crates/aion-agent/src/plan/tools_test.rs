use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    fn make_shared_flag(active: bool) -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(active))
    }

    // --- EnterPlanModeTool unit tests ---

    #[test]
    fn enter_tool_name() {
        let tool = EnterPlanModeTool::new(make_shared_flag(false));
        assert_eq!(tool.name(), "EnterPlanMode");
    }

    #[test]
    fn enter_tool_category_is_info() {
        let tool = EnterPlanModeTool::new(make_shared_flag(false));
        assert!(matches!(tool.category(), ToolCategory::Info));
    }

    #[test]
    fn enter_tool_concurrency_safe() {
        let tool = EnterPlanModeTool::new(make_shared_flag(false));
        assert!(tool.is_concurrency_safe(&json!({})));
    }

    #[test]
    fn enter_tool_schema_has_no_required_fields() {
        let tool = EnterPlanModeTool::new(make_shared_flag(false));
        let schema = tool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.is_empty());
    }

    #[test]
    fn enter_tool_context_modifier_returns_enter() {
        let tool = EnterPlanModeTool::new(make_shared_flag(false));
        let modifier = tool.context_modifier_for(&json!({}));
        assert!(modifier.is_some());
        let cm = modifier.unwrap();
        assert_eq!(cm.plan_mode_transition, Some(PlanModeTransition::Enter));
        // Other fields are default
        assert!(cm.model.is_none());
        assert!(cm.effort.is_none());
        assert!(cm.allowed_tools.is_empty());
    }

    #[tokio::test]
    async fn enter_succeeds_when_not_active() {
        let tool = EnterPlanModeTool::new(make_shared_flag(false));
        let result = tool.execute(json!({})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("Entered plan mode"));
    }

    #[tokio::test]
    async fn enter_rejects_when_already_active() {
        let tool = EnterPlanModeTool::new(make_shared_flag(true));
        let result = tool.execute(json!({})).await;
        assert!(result.is_error);
        assert!(result.content.contains("Already in plan mode"));
    }

    #[test]
    fn enter_tool_describe() {
        let tool = EnterPlanModeTool::new(make_shared_flag(false));
        assert_eq!(tool.describe(&json!({})), "Enter plan mode");
    }

    // --- ExitPlanModeTool unit tests ---

    #[test]
    fn exit_tool_name() {
        let tool = ExitPlanModeTool::new(make_shared_flag(false));
        assert_eq!(tool.name(), "ExitPlanMode");
    }

    #[test]
    fn exit_tool_category_is_info() {
        let tool = ExitPlanModeTool::new(make_shared_flag(false));
        assert!(matches!(tool.category(), ToolCategory::Info));
    }

    #[test]
    fn exit_tool_concurrency_safe() {
        let tool = ExitPlanModeTool::new(make_shared_flag(false));
        assert!(tool.is_concurrency_safe(&json!({})));
    }

    #[test]
    fn exit_tool_schema_has_no_required_fields() {
        let tool = ExitPlanModeTool::new(make_shared_flag(false));
        let schema = tool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.is_empty());
    }

    #[test]
    fn exit_tool_context_modifier_returns_exit() {
        let tool = ExitPlanModeTool::new(make_shared_flag(false));
        let modifier = tool.context_modifier_for(&json!({}));
        assert!(modifier.is_some());
        let cm = modifier.unwrap();
        assert!(matches!(
            cm.plan_mode_transition,
            Some(PlanModeTransition::Exit { plan_content: None })
        ));
        // Other fields are default
        assert!(cm.model.is_none());
        assert!(cm.effort.is_none());
        assert!(cm.allowed_tools.is_empty());
    }

    #[tokio::test]
    async fn exit_succeeds_when_active() {
        let tool = ExitPlanModeTool::new(make_shared_flag(true));
        let result = tool.execute(json!({})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("Exited plan mode"));
    }

    #[tokio::test]
    async fn exit_rejects_when_not_active() {
        let tool = ExitPlanModeTool::new(make_shared_flag(false));
        let result = tool.execute(json!({})).await;
        assert!(result.is_error);
        assert!(result.content.contains("Not in plan mode"));
    }

    #[test]
    fn exit_tool_describe() {
        let tool = ExitPlanModeTool::new(make_shared_flag(false));
        assert_eq!(tool.describe(&json!({})), "Exit plan mode");
    }

    // --- Shared flag tests ---

    #[tokio::test]
    async fn shared_flag_reflects_state_changes() {
        let flag = make_shared_flag(false);
        let enter_tool = EnterPlanModeTool::new(Arc::clone(&flag));
        let exit_tool = ExitPlanModeTool::new(Arc::clone(&flag));

        // Initially not active — enter succeeds, exit fails
        let r = enter_tool.execute(json!({})).await;
        assert!(!r.is_error);
        let r = exit_tool.execute(json!({})).await;
        assert!(r.is_error);

        // Simulate engine setting the flag after processing Enter transition
        flag.store(true, Ordering::Release);

        // Now active — enter fails, exit succeeds
        let r = enter_tool.execute(json!({})).await;
        assert!(r.is_error);
        let r = exit_tool.execute(json!({})).await;
        assert!(!r.is_error);
    }
}
