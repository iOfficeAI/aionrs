use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instructions_not_empty() {
        assert!(!plan_mode_instructions().is_empty());
    }

    #[test]
    fn instructions_mention_read_only_tools() {
        let text = plan_mode_instructions();
        assert!(text.contains("Read"), "should mention Read tool");
        assert!(text.contains("Grep"), "should mention Grep tool");
        assert!(text.contains("Glob"), "should mention Glob tool");
    }

    #[test]
    fn instructions_mention_exit_tool() {
        assert!(plan_mode_instructions().contains("ExitPlanMode"));
    }

    #[test]
    fn instructions_forbid_writes() {
        let text = plan_mode_instructions();
        assert!(text.contains("MUST NOT"));
        assert!(text.contains("Forbidden"));
    }

    #[test]
    fn instructions_guide_planning_workflow() {
        let text = plan_mode_instructions();
        assert!(text.contains("Understand"), "should have explore phase");
        assert!(text.contains("Design"), "should have design phase");
        assert!(text.contains("Write the plan"), "should have plan writing phase");
        assert!(text.contains("Submit for review"), "should have submission phase");
    }

    #[test]
    fn instructions_compose_in_response_not_write_file() {
        let text = plan_mode_instructions();
        assert!(
            text.contains("Compose your implementation plan in your response"),
            "should guide LLM to compose plan in response text"
        );
        assert!(
            !text.contains("Write to the plan file"),
            "should not mention writing to plan file (R-3.4-01 fix)"
        );
    }

    #[test]
    fn instructions_no_bb_brand() {
        let text = plan_mode_instructions();
        assert!(!text.contains("Claude"), "should not contain Claude brand");
        assert!(!text.contains("~/.claude"), "should not contain bb config path");
    }
}
