use super::*;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::TempDir;

    use super::{is_path_gitignored, is_prompt_type};
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
            source: SkillSource::Project,
            loaded_from: LoadedFrom::Skills,
            content: String::new(),
            content_length: 0,
            skill_root: None,
        }
    }

    // --- is_prompt_type ---

    #[test]
    fn is_prompt_type_always_returns_true() {
        let skill = make_skill("any-skill");
        assert!(is_prompt_type(&skill));
    }

    // --- is_path_gitignored ---

    // Not gitignored in a non-git dir → fail open → returns false.
    #[tokio::test]
    async fn is_path_gitignored_returns_false_outside_git_repo() {
        let tmp = TempDir::new().unwrap();
        // No `git init` → not a git repo → git check-ignore fails → fail open
        let cwd = tmp.path().to_str().unwrap();
        let target = tmp.path().join("somefile.rs");
        fs::write(&target, "").unwrap();

        let result = is_path_gitignored(&target, cwd).await;
        assert!(!result);
    }

    // Gitignored path in a real git repo → returns true.
    #[tokio::test]
    async fn is_path_gitignored_returns_true_for_ignored_path() {
        let tmp = TempDir::new().unwrap();
        let cwd = tmp.path().to_str().unwrap();

        let init_ok = std::process::Command::new("git")
            .args(["init"])
            .current_dir(tmp.path())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if !init_ok {
            // git not available — skip
            return;
        }

        fs::write(tmp.path().join(".gitignore"), "ignored_dir/\n").unwrap();
        let ignored = tmp.path().join("ignored_dir");
        fs::create_dir_all(&ignored).unwrap();

        let result = is_path_gitignored(&ignored, cwd).await;
        assert!(result, "ignored_dir/ should be detected as gitignored");
    }

    // Non-ignored path in a real git repo → returns false.
    #[tokio::test]
    async fn is_path_gitignored_returns_false_for_tracked_path() {
        let tmp = TempDir::new().unwrap();
        let cwd = tmp.path().to_str().unwrap();

        let init_ok = std::process::Command::new("git")
            .args(["init"])
            .current_dir(tmp.path())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if !init_ok {
            return;
        }

        // Empty .gitignore — nothing is ignored
        fs::write(tmp.path().join(".gitignore"), "").unwrap();
        let tracked = tmp.path().join("normal_dir");
        fs::create_dir_all(&tracked).unwrap();

        let result = is_path_gitignored(&tracked, cwd).await;
        assert!(!result);
    }

    // Path that doesn't exist → git check-ignore exits non-zero → fail open → false.
    #[tokio::test]
    async fn is_path_gitignored_returns_false_for_nonexistent_path() {
        let tmp = TempDir::new().unwrap();
        let cwd = tmp.path().to_str().unwrap();
        let nonexistent = Path::new("/nonexistent/path/xyz");

        let result = is_path_gitignored(nonexistent, cwd).await;
        assert!(!result);
    }
}

// ---------------------------------------------------------------------------
// Supplemental tests (tester role — covers test-plan.md cases)
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "discovery_supplemental_test.rs"]
mod discovery_supplemental_test;
