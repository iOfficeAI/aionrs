// Plan mode system prompt instructions.
//
// These prompts guide the LLM's behavior while in plan mode: what tools to
// use, what actions are forbidden, and how to structure the resulting plan.

/// Instructions injected into the system prompt when plan mode is active.
///
/// Guides the LLM through a structured planning workflow:
/// 1. Explore the codebase with read-only tools
/// 2. Design the implementation approach
/// 3. Write the plan
/// 4. Call ExitPlanMode when ready for user review
pub fn plan_mode_instructions() -> &'static str {
    r#"# Plan Mode

Plan mode is active. You MUST NOT make any edits, run any non-read-only tools, or otherwise make any changes to the system. The only exception is writing to the plan file mentioned below.

## Allowed actions
- Read files, search code, and explore the codebase using read-only tools (Read, Grep, Glob)
- Write to the plan file (if a plan file path is provided)
- Ask clarifying questions

## Forbidden actions
- Editing, creating, or deleting files (except the plan file)
- Running shell commands that modify state
- Making commits or pushing changes

## Planning workflow

### Phase 1: Understand
Explore the codebase to understand the current architecture, relevant files, and existing patterns. Read key files and search for related code.

### Phase 2: Design
Based on your understanding, design the implementation approach:
- Identify which files need to be created or modified
- Reference existing functions and utilities that should be reused
- Consider edge cases and error handling

### Phase 3: Write the plan
Write a clear, actionable implementation plan including:
- **Context**: brief explanation of why this change is needed
- **Files to modify**: list each file and what changes are needed
- **Existing code to reuse**: reference functions and utilities with file paths
- **Verification**: how to test the changes end-to-end

### Phase 4: Submit for review
When your plan is complete, call ExitPlanMode to submit it for user review. Do not ask "Is this plan okay?" — calling ExitPlanMode is the way to request approval."#
}

/// Notice appended to the system prompt after the agent exits plan mode.
///
/// Informs the LLM that full tool access has been restored and it may
/// proceed with implementation.
pub fn plan_mode_exit_notice() -> &'static str {
    "Plan mode has been exited. Full tool access is restored. You can now make edits, run tools, and implement the plan."
}

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
        assert!(
            text.contains("Submit for review"),
            "should have submission phase"
        );
    }

    #[test]
    fn instructions_no_bb_brand() {
        let text = plan_mode_instructions();
        assert!(!text.contains("Claude"), "should not contain Claude brand");
        assert!(
            !text.contains("~/.claude"),
            "should not contain bb config path"
        );
    }

    #[test]
    fn exit_notice_not_empty() {
        assert!(!plan_mode_exit_notice().is_empty());
    }

    #[test]
    fn exit_notice_indicates_exit() {
        let text = plan_mode_exit_notice();
        assert!(text.contains("exited"), "should indicate plan mode exited");
        assert!(text.contains("restored"), "should indicate access restored");
    }
}
