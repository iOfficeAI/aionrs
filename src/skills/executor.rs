use crate::skills::substitution::substitute_arguments;
use crate::skills::types::{ExecutionContext, SkillMetadata};

/// Prepare skill content for inline execution.
///
/// Steps:
/// 1. If the skill has a known `skill_root`, prepend a base-directory header.
/// 2. Perform variable substitution (arguments + env vars).
///
/// The `session_id` is `None` in Phase 3; it will be wired in Phase 6.
pub fn prepare_inline_content(
    skill: &SkillMetadata,
    args: Option<&str>,
    session_id: Option<&str>,
) -> String {
    // Prepend base directory header so the model can resolve relative paths
    // (e.g. `./schemas/foo.json`). Matches TS `processPromptSlashCommand`.
    let base = match skill.skill_root.as_deref() {
        Some(root) => {
            let normalized = normalize_path_separators(root);
            format!("Base directory for this skill: {normalized}\n\n{}", skill.content)
        }
        None => skill.content.clone(),
    };

    substitute_arguments(
        &base,
        args,
        &skill.argument_names,
        skill.skill_root.as_deref(),
        session_id,
    )
}

/// Normalize path separators to forward slashes.
/// On non-Windows platforms this is a no-op; included for portability.
fn normalize_path_separators(path: &str) -> String {
    if cfg!(windows) {
        path.replace('\\', "/")
    } else {
        path.to_owned()
    }
}

/// Check whether a skill can be executed in inline mode.
/// Returns an error string if it cannot (e.g. fork-only skill).
pub fn check_execution_context(skill: &SkillMetadata) -> Result<(), String> {
    if skill.execution_context == ExecutionContext::Fork {
        return Err(format!(
            "Skill '{}' requires fork execution context, which is not yet supported \
             (planned for Phase 7). Use an inline skill instead.",
            skill.name
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::types::{
        ExecutionContext, EffortLevel, LoadedFrom, SkillMetadata, SkillSource,
    };

    fn make_skill(content: &str, skill_root: Option<&str>) -> SkillMetadata {
        SkillMetadata {
            name: "test".to_string(),
            display_name: None,
            description: String::new(),
            has_user_specified_description: false,
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
            skill_root: skill_root.map(str::to_owned),
        }
    }

    #[test]
    fn test_prepare_inline_no_args() {
        let skill = make_skill("Do the thing.", None);
        let result = prepare_inline_content(&skill, None, None);
        assert_eq!(result, "Do the thing.");
    }

    #[test]
    fn test_prepare_inline_with_base_directory_header() {
        let skill = make_skill("Content here.", Some("/my/skill/dir"));
        let result = prepare_inline_content(&skill, None, None);
        assert!(
            result.starts_with("Base directory for this skill: /my/skill/dir\n\n"),
            "expected base directory header, got: {result}"
        );
        assert!(result.contains("Content here."));
    }

    #[test]
    fn test_prepare_inline_substitutes_arguments() {
        let skill = make_skill("Target: $ARGUMENTS", None);
        let result = prepare_inline_content(&skill, Some("foo"), None);
        assert_eq!(result, "Target: foo");
    }

    #[test]
    fn test_prepare_inline_substitutes_skill_dir() {
        let skill = make_skill("Dir: ${CLAUDE_SKILL_DIR}", Some("/skills/mine"));
        let result = prepare_inline_content(&skill, None, None);
        // Header + substituted dir
        assert!(result.contains("Dir: /skills/mine"));
    }

    #[test]
    fn test_prepare_inline_substitutes_session_id() {
        let skill = make_skill("Session: ${CLAUDE_SESSION_ID}", None);
        let result = prepare_inline_content(&skill, None, Some("sess-abc"));
        assert!(result.contains("Session: sess-abc"));
    }

    #[test]
    fn test_check_execution_context_inline_ok() {
        let skill = make_skill("", None);
        assert!(check_execution_context(&skill).is_ok());
    }

    #[test]
    fn test_check_execution_context_fork_err() {
        let mut skill = make_skill("", None);
        skill.execution_context = ExecutionContext::Fork;
        let err = check_execution_context(&skill).unwrap_err();
        assert!(err.contains("fork execution context"));
    }
}

// ---------------------------------------------------------------------------
// Supplemental tests (tester role — covers test-plan.md cases not in impl tests)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod supplemental_tests {
    use super::*;
    use crate::skills::types::{ExecutionContext, LoadedFrom, SkillMetadata, SkillSource};

    fn make_skill_full(
        name: &str,
        content: &str,
        skill_root: Option<&str>,
        argument_names: Vec<String>,
        context: ExecutionContext,
    ) -> SkillMetadata {
        SkillMetadata {
            name: name.to_string(),
            display_name: None,
            description: String::new(),
            has_user_specified_description: false,
            allowed_tools: Vec::new(),
            argument_hint: None,
            argument_names,
            when_to_use: None,
            version: None,
            model: None,
            disable_model_invocation: false,
            user_invocable: true,
            execution_context: context,
            agent: None,
            effort: None,
            shell: None,
            paths: Vec::new(),
            hooks_raw: None,
            source: SkillSource::User,
            loaded_from: LoadedFrom::Skills,
            content: content.to_string(),
            content_length: content.len(),
            skill_root: skill_root.map(str::to_owned),
        }
    }

    // TC-10.1: basic prepare_inline_content call
    #[test]
    fn tc_10_1_prepare_inline_substitutes_arguments() {
        let skill = make_skill_full("s", "Search $ARGUMENTS", None, vec![], ExecutionContext::Inline);
        let result = prepare_inline_content(&skill, Some("rust"), None);
        assert_eq!(result, "Search rust");
    }

    // TC-10.2: no args, no placeholder → content unchanged
    #[test]
    fn tc_10_2_no_args_no_placeholder_unchanged() {
        let skill = make_skill_full("s", "Just content.", None, vec![], ExecutionContext::Inline);
        let result = prepare_inline_content(&skill, None, None);
        assert_eq!(result, "Just content.");
    }

    // TC-10.3: skill_root causes base directory header to be prepended
    #[test]
    fn tc_10_3_skill_root_prepends_header() {
        let skill = make_skill_full(
            "s",
            "${CLAUDE_SKILL_DIR}/script.sh",
            Some("/path/to/skill"),
            vec![],
            ExecutionContext::Inline,
        );
        let result = prepare_inline_content(&skill, None, None);
        // Header should be prepended
        assert!(
            result.starts_with("Base directory for this skill: /path/to/skill"),
            "expected header, got: {result}"
        );
        // ${CLAUDE_SKILL_DIR} should be substituted
        assert!(result.contains("/path/to/skill/script.sh"));
    }

    // TC-10.x: session_id substitution wired through
    #[test]
    fn tc_10_x_session_id_substituted() {
        let skill = make_skill_full("s", "${CLAUDE_SESSION_ID}", None, vec![], ExecutionContext::Inline);
        let result = prepare_inline_content(&skill, None, Some("sess-xyz"));
        assert_eq!(result, "sess-xyz");
    }

    // TC-10.x: argument_names from metadata are used
    #[test]
    fn tc_10_x_argument_names_from_metadata() {
        // $query maps to index 0; "main function" parses to ["main", "function"],
        // so $query is replaced with "main" (the first argument).
        let names = vec!["query".to_string()];
        let skill = make_skill_full("s", "Find $query in codebase", None, names, ExecutionContext::Inline);
        let result = prepare_inline_content(&skill, Some("main function"), None);
        assert_eq!(result, "Find main in codebase");
    }

    // TC-10.x: fork context check
    #[test]
    fn tc_10_x_check_context_fork_returns_err() {
        let skill = make_skill_full("fork-skill", "body", None, vec![], ExecutionContext::Fork);
        let result = check_execution_context(&skill);
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("fork-skill"));
        assert!(msg.contains("fork execution context"));
    }

    // TC-10.x: inline context check returns Ok
    #[test]
    fn tc_10_x_check_context_inline_returns_ok() {
        let skill = make_skill_full("inline-skill", "body", None, vec![], ExecutionContext::Inline);
        assert!(check_execution_context(&skill).is_ok());
    }
}
