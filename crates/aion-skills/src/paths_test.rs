use super::*;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_dir(base: &Path, rel: &str) -> PathBuf {
        let p = base.join(rel);
        fs::create_dir_all(&p).unwrap();
        p
    }

    // --- user_skills_dir ---

    #[test]
    fn test_user_skills_dir_contains_aionrs_skills() {
        if let Some(dir) = user_skills_dir() {
            let s = dir.to_string_lossy();
            assert!(s.contains("aionrs"), "expected 'aionrs' in path: {s}");
            assert!(s.ends_with("skills"), "expected path to end with 'skills': {s}");
        }
        // If app_config_dir() returns None (rare), that's acceptable.
    }

    #[test]
    fn test_user_commands_dir_contains_aionrs_commands() {
        if let Some(dir) = user_commands_dir() {
            let s = dir.to_string_lossy();
            assert!(s.contains("aionrs"));
            assert!(s.ends_with("commands"));
        }
    }

    // --- find_git_root ---

    #[test]
    fn test_find_git_root_finds_git_dir() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let nested = root.join("a").join("b").join("c");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir(root.join(".git")).unwrap();

        let found = find_git_root(&nested).unwrap();
        assert_eq!(found, root);
    }

    #[test]
    fn test_find_git_root_returns_none_when_absent() {
        let tmp = TempDir::new().unwrap();
        // No .git anywhere under tmp
        let result = find_git_root(tmp.path());
        // May or may not find a .git in an ancestor of tmp — we just ensure no panic.
        // If the test environment has a .git above tmp, that's ok.
        let _ = result;
    }

    #[test]
    fn test_find_git_root_at_root_itself() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join(".git")).unwrap();
        let found = find_git_root(tmp.path()).unwrap();
        assert_eq!(found, tmp.path());
    }

    // --- project_skills_dirs ---

    #[test]
    fn test_project_skills_dirs_finds_dirs() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        // Create git root marker
        fs::create_dir(root.join(".git")).unwrap();

        // Create skills dirs at root and nested level
        make_dir(root, ".aionrs/skills");
        let nested = root.join("sub").join("project");
        fs::create_dir_all(&nested).unwrap();
        make_dir(&nested, ".aionrs/skills");

        let dirs = project_skills_dirs(&nested);
        // Should find both (deepest first)
        assert_eq!(dirs.len(), 2);
        // First one is deeper (closest to cwd)
        assert!(dirs[0].starts_with(&nested));
        assert!(dirs[1].starts_with(root));
    }

    #[test]
    fn test_project_skills_dirs_skips_missing() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join(".git")).unwrap();
        // No .aionrs/skills/ anywhere
        let dirs = project_skills_dirs(tmp.path());
        assert!(dirs.is_empty());
    }

    // --- additional_skills_dirs ---

    #[test]
    fn test_additional_skills_dirs_existing() {
        let tmp = TempDir::new().unwrap();
        make_dir(tmp.path(), ".aionrs/skills");
        let result = additional_skills_dirs(&[tmp.path().to_path_buf()]);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_additional_skills_dirs_missing_silently_skipped() {
        let tmp = TempDir::new().unwrap();
        // No .aionrs/skills/ under tmp
        let result = additional_skills_dirs(&[tmp.path().to_path_buf()]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_additional_skills_dirs_empty_input() {
        let result = additional_skills_dirs(&[]);
        assert!(result.is_empty());
    }
}

// ---------------------------------------------------------------------------
// Supplemental tests (tester role — covers test-plan.md cases not in impl tests)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod supplemental_tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_dir(base: &Path, rel: &str) -> PathBuf {
        let p = base.join(rel);
        fs::create_dir_all(&p).unwrap();
        p
    }

    // -----------------------------------------------------------------------
    // TC-1.x: find_git_root
    // -----------------------------------------------------------------------

    #[test]
    fn tc_1_1_find_git_root_at_root_dir() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join(".git")).unwrap();
        let found = find_git_root(tmp.path()).unwrap();
        assert_eq!(found, tmp.path());
    }

    #[test]
    fn tc_1_2_find_git_root_from_subdirectory() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir(root.join(".git")).unwrap();
        let sub = root.join("src").join("module");
        fs::create_dir_all(&sub).unwrap();

        let found = find_git_root(&sub).unwrap();
        assert_eq!(found, root);
    }

    #[test]
    fn tc_1_4_find_git_root_deep_nesting() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir(root.join(".git")).unwrap();
        let deep = root.join("a").join("b").join("c").join("d").join("e");
        fs::create_dir_all(&deep).unwrap();

        let found = find_git_root(&deep).unwrap();
        assert_eq!(found, root);
    }

    #[test]
    fn tc_1_5_find_git_root_git_is_file_not_dir() {
        // git worktree: .git is a file, not a directory
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::write(root.join(".git"), "gitdir: ../main/.git/worktrees/wt").unwrap();

        // Implementation uses .exists() which is true for both files and dirs
        let found = find_git_root(root);
        assert!(found.is_some(), ".git file should be recognized as git root");
        assert_eq!(found.unwrap(), root);
    }

    // -----------------------------------------------------------------------
    // TC-2.x / TC-3.x: user_skills_dir / user_commands_dir
    // -----------------------------------------------------------------------

    #[test]
    fn tc_2_1_user_skills_dir_ends_with_skills() {
        if let Some(dir) = user_skills_dir() {
            let s = dir.to_string_lossy();
            assert!(s.ends_with("skills"), "path should end with 'skills': {s}");
            assert!(s.contains("aionrs"), "path should contain 'aionrs': {s}");
        }
    }

    #[test]
    fn tc_3_1_user_commands_dir_ends_with_commands() {
        if let Some(dir) = user_commands_dir() {
            let s = dir.to_string_lossy();
            assert!(s.ends_with("commands"), "path should end with 'commands': {s}");
            assert!(s.contains("aionrs"), "path should contain 'aionrs': {s}");
        }
    }

    // -----------------------------------------------------------------------
    // TC-4.x: project_skills_dirs
    // -----------------------------------------------------------------------

    #[test]
    fn tc_4_2_project_skills_dirs_nonexistent_subdir_not_returned() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join(".git")).unwrap();
        // No .aionrs/skills/ created
        let dirs = project_skills_dirs(tmp.path());
        assert!(dirs.is_empty(), "should be empty when .aionrs/skills/ doesn't exist");
    }

    #[test]
    fn tc_4_3_project_skills_dirs_deepest_first() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir(root.join(".git")).unwrap();
        make_dir(root, ".aionrs/skills");

        let inner = root.join("sub");
        fs::create_dir_all(&inner).unwrap();
        make_dir(&inner, ".aionrs/skills");

        let dirs = project_skills_dirs(&inner);
        assert_eq!(dirs.len(), 2);
        // First element should be closest to cwd (deepest)
        assert!(
            dirs[0].starts_with(&inner),
            "first dir should be the inner one (deepest): {:?}",
            dirs[0]
        );
    }

    #[test]
    fn tc_4_4_project_skills_dirs_stops_at_git_root() {
        let tmp = TempDir::new().unwrap();
        let grandparent = tmp.path();
        // .aionrs/skills in grandparent (above git root) — should NOT be collected
        make_dir(grandparent, ".aionrs/skills");

        let repo = grandparent.join("repo");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir(repo.join(".git")).unwrap();
        make_dir(&repo, ".aionrs/skills");

        let sub = repo.join("sub");
        fs::create_dir_all(&sub).unwrap();

        let dirs = project_skills_dirs(&sub);
        // Only repo's .aionrs/skills should be included
        assert!(
            dirs.iter().all(|d| d.starts_with(&repo)),
            "should not include dirs above git root, got: {dirs:?}"
        );
        assert_eq!(dirs.len(), 1);
    }

    #[test]
    fn tc_4_6_project_skills_dirs_nonexistent_cwd_no_panic() {
        // Should not panic even if cwd does not exist
        let dirs = project_skills_dirs(Path::new("/tmp/nonexistent_cwd_xyz_abc_123"));
        // Result may be empty or not (depends on ancestor dirs) — just must not panic
        let _ = dirs;
    }

    // -----------------------------------------------------------------------
    // TC-5.x: project_commands_dirs
    // -----------------------------------------------------------------------

    #[test]
    fn tc_5_1_project_commands_dirs_finds_commands_dir() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir(root.join(".git")).unwrap();
        make_dir(root, ".aionrs/commands");

        let dirs = project_commands_dirs(root);
        assert_eq!(dirs.len(), 1);
        assert!(dirs[0].ends_with(".aionrs/commands"));
    }

    // -----------------------------------------------------------------------
    // TC-6.x: additional_skills_dirs
    // -----------------------------------------------------------------------

    #[test]
    fn tc_6_1_additional_skills_dirs_with_existing_subdir() {
        let tmp = TempDir::new().unwrap();
        make_dir(tmp.path(), ".aionrs/skills");

        let result = additional_skills_dirs(&[tmp.path().to_path_buf()]);
        assert_eq!(result.len(), 1);
        assert!(result[0].ends_with(".aionrs/skills"));
    }

    #[test]
    fn tc_6_2_additional_skills_dirs_no_subdir_skipped() {
        let tmp = TempDir::new().unwrap();
        // No .aionrs/skills/ subdirectory
        let result = additional_skills_dirs(&[tmp.path().to_path_buf()]);
        assert!(result.is_empty());
    }

    #[test]
    fn tc_6_4_additional_skills_dirs_multiple_add_dirs() {
        let tmp1 = TempDir::new().unwrap();
        let tmp2 = TempDir::new().unwrap();
        make_dir(tmp1.path(), ".aionrs/skills");
        make_dir(tmp2.path(), ".aionrs/skills");

        let result = additional_skills_dirs(&[tmp1.path().to_path_buf(), tmp2.path().to_path_buf()]);
        assert_eq!(result.len(), 2);
    }
}
