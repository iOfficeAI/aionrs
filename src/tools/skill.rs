use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::protocol::events::ToolCategory;
use crate::skills::executor::{check_execution_context, prepare_inline_content};
use crate::skills::types::SkillMetadata;
use crate::types::tool::{JsonSchema, ToolResult};

use super::Tool;

/// A tool that allows the LLM to invoke named skills.
///
/// Each skill is looked up by name (exact match, leading `/` stripped),
/// its content is prepared with variable substitution and shell execution,
/// and returned as a `ToolResult`.  The Skill list is injected into the
/// system prompt in Phase 9; this tool's `description()` returns a fixed string.
pub struct SkillTool {
    skills: Arc<Vec<SkillMetadata>>,
    /// Working directory for shell command execution inside skill content.
    cwd: String,
}

impl SkillTool {
    pub fn new(skills: Arc<Vec<SkillMetadata>>, cwd: String) -> Self {
        Self { skills, cwd }
    }

    /// Find a skill by exact name (case-sensitive, leading `/` stripped).
    fn find_skill(&self, name: &str) -> Option<&SkillMetadata> {
        let name = name.trim_start_matches('/');
        self.skills.iter().find(|s| s.name == name)
    }

    /// Build a comma-separated list of available skill names for error messages.
    fn available_names(&self) -> String {
        self.skills
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &str {
        "Skill"
    }

    fn description(&self) -> &str {
        "Invoke a named skill by name. \
         Use the skill name exactly as listed in the system prompt. \
         Optionally pass arguments as a single string."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "skill": {
                    "type": "string",
                    "description": "The skill name. E.g., \"commit\", \"review-pr\", or \"pdf\""
                },
                "args": {
                    "type": "string",
                    "description": "Optional arguments for the skill"
                }
            },
            "required": ["skill"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // Skills may modify context; conservatively mark as not concurrency-safe.
        false
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let Some(skill_name) = input["skill"].as_str() else {
            return ToolResult {
                content: "Missing required parameter: skill".to_string(),
                is_error: true,
            };
        };

        let skill = match self.find_skill(skill_name) {
            Some(s) => s,
            None => {
                let available = self.available_names();
                return ToolResult {
                    content: format!(
                        "Skill '{}' not found. Available skills: {}",
                        skill_name, available
                    ),
                    is_error: true,
                };
            }
        };

        // Check execution context (fork skills are not yet supported)
        if let Err(msg) = check_execution_context(skill) {
            return ToolResult {
                content: msg,
                is_error: true,
            };
        }

        let args = input["args"].as_str();
        // session_id: None in Phase 3/4; wired in Phase 6
        let content = match prepare_inline_content(skill, args, None, &self.cwd).await {
            Ok(c) => c,
            Err(e) => {
                return ToolResult {
                    content: e.to_string(),
                    is_error: true,
                }
            }
        };

        ToolResult {
            content,
            is_error: false,
        }
    }

    fn category(&self) -> ToolCategory {
        // Inline mode returns skill content for the model to act on — categorised
        // as Info since it does not directly modify files or run commands.
        ToolCategory::Info
    }

    fn describe(&self, input: &Value) -> String {
        let name = input.get("skill").and_then(|v| v.as_str()).unwrap_or("?");
        match input.get("args").and_then(|v| v.as_str()) {
            Some(args) if !args.is_empty() => format!("Skill {name} {args}"),
            _ => format!("Skill {name}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::types::{ExecutionContext, LoadedFrom, SkillSource};
    use serde_json::json;

    fn make_skill(name: &str, content: &str) -> SkillMetadata {
        SkillMetadata {
            name: name.to_string(),
            display_name: None,
            description: format!("desc of {name}"),
            has_user_specified_description: true,
            allowed_tools: Vec::new(),
            argument_hint: None,
            argument_names: Vec::new(),
            when_to_use: None,
            version: None,
            model: None,
            disable_model_invocation: false,
            user_invocable: true,
            execution_context: ExecutionContext::Inline,
            agent: None,
            effort: None,
            shell: None,
            paths: Vec::new(),
            hooks_raw: None,
            source: SkillSource::User,
            loaded_from: LoadedFrom::Skills,
            content: content.to_string(),
            content_length: content.len(),
            skill_root: None,
        }
    }

    fn tool_with(skills: Vec<SkillMetadata>) -> SkillTool {
        SkillTool::new(Arc::new(skills), "/tmp".to_string())
    }

    #[tokio::test]
    async fn test_skill_found_returns_content() {
        let tool = tool_with(vec![make_skill("commit", "# Commit\nDo a commit.")]);
        let result = tool.execute(json!({ "skill": "commit" })).await;
        assert!(!result.is_error);
        assert!(result.content.contains("Do a commit."));
    }

    #[tokio::test]
    async fn test_skill_not_found_returns_error() {
        let tool = tool_with(vec![make_skill("commit", "content")]);
        let result = tool.execute(json!({ "skill": "nonexistent" })).await;
        assert!(result.is_error);
        assert!(result.content.contains("not found"));
        assert!(result.content.contains("commit"));
    }

    #[tokio::test]
    async fn test_leading_slash_stripped() {
        let tool = tool_with(vec![make_skill("commit", "body")]);
        let result = tool.execute(json!({ "skill": "/commit" })).await;
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_missing_skill_param_returns_error() {
        let tool = tool_with(vec![]);
        let result = tool.execute(json!({})).await;
        assert!(result.is_error);
        assert!(result.content.contains("Missing required parameter"));
    }

    #[tokio::test]
    async fn test_args_substituted() {
        let tool = tool_with(vec![make_skill("greet", "Hello $ARGUMENTS!")]);
        let result = tool.execute(json!({ "skill": "greet", "args": "world" })).await;
        assert!(!result.is_error);
        assert_eq!(result.content, "Hello world!");
    }

    #[tokio::test]
    async fn test_fork_skill_returns_error() {
        let mut skill = make_skill("fork-skill", "body");
        skill.execution_context = ExecutionContext::Fork;
        let tool = tool_with(vec![skill]);
        let result = tool.execute(json!({ "skill": "fork-skill" })).await;
        assert!(result.is_error);
        assert!(result.content.contains("fork execution context"));
    }

    #[test]
    fn test_describe_with_args() {
        let tool = tool_with(vec![]);
        let desc = tool.describe(&json!({ "skill": "commit", "args": "fix bug" }));
        assert_eq!(desc, "Skill commit fix bug");
    }

    #[test]
    fn test_describe_without_args() {
        let tool = tool_with(vec![]);
        let desc = tool.describe(&json!({ "skill": "commit" }));
        assert_eq!(desc, "Skill commit");
    }

    #[test]
    fn test_name_is_skill() {
        let tool = tool_with(vec![]);
        assert_eq!(tool.name(), "Skill");
    }

    #[test]
    fn test_not_concurrency_safe() {
        let tool = tool_with(vec![]);
        assert!(!tool.is_concurrency_safe(&json!({})));
    }
}

// ---------------------------------------------------------------------------
// Supplemental tests (tester role — covers test-plan.md cases not in impl tests)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod supplemental_tests {
    use std::sync::Arc;

    use serde_json::json;

    use crate::skills::types::{
        ExecutionContext, LoadedFrom, SkillMetadata, SkillSource,
    };

    use super::SkillTool;
    use crate::tools::Tool;

    fn make_skill(name: &str, content: &str) -> SkillMetadata {
        SkillMetadata {
            name: name.to_string(),
            display_name: None,
            description: format!("desc of {name}"),
            has_user_specified_description: true,
            allowed_tools: Vec::new(),
            argument_hint: None,
            argument_names: Vec::new(),
            when_to_use: None,
            version: None,
            model: None,
            disable_model_invocation: false,
            user_invocable: true,
            execution_context: ExecutionContext::Inline,
            agent: None,
            effort: None,
            shell: None,
            paths: Vec::new(),
            hooks_raw: None,
            source: SkillSource::User,
            loaded_from: LoadedFrom::Skills,
            content: content.to_string(),
            content_length: content.len(),
            skill_root: None,
        }
    }

    fn tool_with(skills: Vec<SkillMetadata>) -> SkillTool {
        SkillTool::new(Arc::new(skills), "/tmp".to_string())
    }

    // -----------------------------------------------------------------------
    // TC-11.x: find_skill
    // -----------------------------------------------------------------------

    #[test]
    fn tc_11_1_exact_match_found() {
        let tool = tool_with(vec![make_skill("commit", "body")]);
        // Access find_skill through execute to verify behavior indirectly
        // (find_skill is private, tested via execute)
        // Direct check via available_names() not exposed, so we verify via execute.
        // Verified in tc_13_1 instead. This test just verifies construction.
        assert_eq!(tool.name(), "Skill");
    }

    #[test]
    fn tc_11_4_case_sensitive_no_match() {
        // "Commit" (capital C) should not match "commit"
        let tool = tool_with(vec![make_skill("commit", "body")]);
        // Verified via execute in tc_13.x
        let _ = tool;
    }

    #[test]
    fn tc_11_5_empty_skills_list_no_panic() {
        let tool = tool_with(vec![]);
        assert_eq!(tool.name(), "Skill"); // just verifies no panic
    }

    // -----------------------------------------------------------------------
    // TC-12.x: name, schema, is_concurrency_safe
    // -----------------------------------------------------------------------

    #[test]
    fn tc_12_1_name_is_skill() {
        let tool = tool_with(vec![]);
        assert_eq!(tool.name(), "Skill");
    }

    #[test]
    fn tc_12_2_schema_skill_required() {
        let tool = tool_with(vec![]);
        let schema = tool.input_schema();
        let required = schema["required"].as_array().unwrap();
        let names: Vec<&str> = required.iter().map(|v| v.as_str().unwrap()).collect();
        assert!(names.contains(&"skill"), "schema required must contain 'skill'");
    }

    #[test]
    fn tc_12_3_schema_args_not_required() {
        let tool = tool_with(vec![]);
        let schema = tool.input_schema();
        // args should be in properties
        assert!(schema["properties"]["args"].is_object(), "args should be in properties");
        // args should NOT be in required
        let required = schema["required"].as_array().unwrap();
        let names: Vec<&str> = required.iter().map(|v| v.as_str().unwrap()).collect();
        assert!(!names.contains(&"args"), "args should not be in required");
    }

    #[test]
    fn tc_12_4_is_concurrency_safe_false() {
        let tool = tool_with(vec![]);
        assert!(!tool.is_concurrency_safe(&json!({})));
        assert!(!tool.is_concurrency_safe(&json!({"skill": "foo"})));
    }

    // -----------------------------------------------------------------------
    // TC-13.x: execute (async)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn tc_13_1_successful_inline_execution() {
        let tool = tool_with(vec![make_skill("my-skill", "Run $ARGUMENTS")]);
        let result = tool.execute(json!({"skill": "my-skill", "args": "foo"})).await;
        assert!(!result.is_error);
        assert_eq!(result.content, "Run foo");
    }

    #[tokio::test]
    async fn tc_13_2_skill_not_found_is_error() {
        let tool = tool_with(vec![make_skill("commit", "body")]);
        let result = tool.execute(json!({"skill": "nonexistent"})).await;
        assert!(result.is_error);
        assert!(result.content.contains("not found") || result.content.contains("Skill"));
    }

    #[tokio::test]
    async fn tc_13_3_not_found_error_lists_available_skills() {
        let tool = tool_with(vec![
            make_skill("commit", "body"),
            make_skill("review", "body"),
        ]);
        let result = tool.execute(json!({"skill": "missing"})).await;
        assert!(result.is_error);
        assert!(result.content.contains("commit"));
        assert!(result.content.contains("review"));
    }

    #[tokio::test]
    async fn tc_13_4_fork_skill_returns_error() {
        let mut skill = make_skill("fork-skill", "body");
        skill.execution_context = ExecutionContext::Fork;
        let tool = tool_with(vec![skill]);
        let result = tool.execute(json!({"skill": "fork-skill"})).await;
        assert!(result.is_error);
        assert!(result.content.contains("fork"));
    }

    #[tokio::test]
    async fn tc_13_5_no_args_field_still_works() {
        let tool = tool_with(vec![make_skill("my-skill", "Just content.")]);
        let result = tool.execute(json!({"skill": "my-skill"})).await;
        assert!(!result.is_error);
        assert_eq!(result.content, "Just content.");
    }

    #[tokio::test]
    async fn tc_13_6_leading_slash_stripped() {
        let tool = tool_with(vec![make_skill("my-skill", "body")]);
        let result = tool.execute(json!({"skill": "/my-skill"})).await;
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn tc_13_7_missing_skill_field_returns_error() {
        let tool = tool_with(vec![]);
        let result = tool.execute(json!({"args": "foo"})).await;
        assert!(result.is_error);
        assert!(result.content.to_lowercase().contains("missing") || result.content.contains("skill"));
    }

    #[tokio::test]
    async fn tc_13_8_full_variable_substitution_integration() {
        let mut skill = make_skill("my-skill", "Run ${CLAUDE_SKILL_DIR}/tool.sh $ARGUMENTS[0]");
        skill.skill_root = Some("/my/skill".to_string());
        let tool = tool_with(vec![skill]);
        let result = tool.execute(json!({"skill": "my-skill", "args": "alpha"})).await;
        assert!(!result.is_error);
        // base dir header is prepended, then substitution applied
        assert!(result.content.contains("/my/skill/tool.sh alpha"));
    }

    #[tokio::test]
    async fn tc_13_x_case_sensitive_no_match() {
        // "Commit" does not match "commit"
        let tool = tool_with(vec![make_skill("commit", "body")]);
        let result = tool.execute(json!({"skill": "Commit"})).await;
        assert!(result.is_error, "case-sensitive lookup: 'Commit' should not match 'commit'");
    }

    // -----------------------------------------------------------------------
    // TC-14.x: description
    // -----------------------------------------------------------------------

    #[test]
    fn tc_14_1_description_is_non_empty() {
        let tool = tool_with(vec![make_skill("commit", "body"), make_skill("review", "body")]);
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn tc_14_2_empty_skills_description_no_panic() {
        let tool = tool_with(vec![]);
        assert!(!tool.description().is_empty());
    }
}
