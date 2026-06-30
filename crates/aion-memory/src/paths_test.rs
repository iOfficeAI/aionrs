use super::*;

// ===========================================================================
// Unit tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::path::Path;

    // -- sanitize_path --------------------------------------------------------

    #[test]
    fn sanitize_simple_path() {
        assert_eq!(sanitize_path("/home/user/project"), "-home-user-project");
    }

    #[test]
    fn sanitize_preserves_alphanumeric() {
        assert_eq!(sanitize_path("abc123"), "abc123");
    }

    #[test]
    fn sanitize_replaces_special_chars() {
        assert_eq!(sanitize_path("a/b:c d"), "a-b-c-d");
    }

    #[test]
    fn sanitize_long_path_truncates_with_hash() {
        let long_path = "/".to_string() + &"a".repeat(300);
        let result = sanitize_path(&long_path);
        assert!(result.len() > MAX_SANITIZED_LENGTH); // truncated + hash
        assert!(result.len() < MAX_SANITIZED_LENGTH + 20); // hash isn't huge
        assert!(result.contains('-')); // has separator before hash
    }

    #[test]
    fn sanitize_two_long_paths_produce_different_results() {
        let path_a = "/".to_string() + &"a".repeat(300);
        let path_b = "/".to_string() + &"b".repeat(300);
        assert_ne!(sanitize_path(&path_a), sanitize_path(&path_b));
    }

    // -- contains_traversal ---------------------------------------------------

    #[test]
    fn traversal_detected() {
        assert!(contains_traversal("../foo"));
        assert!(contains_traversal("foo/../bar"));
        assert!(contains_traversal("/foo/.."));
        assert!(contains_traversal("foo\\..\\bar"));
    }

    #[test]
    fn traversal_not_detected_for_safe_paths() {
        assert!(!contains_traversal("/foo/bar"));
        assert!(!contains_traversal("foo.bar"));
        assert!(!contains_traversal("foo...bar"));
        assert!(!contains_traversal("/tmp/test.md"));
    }

    // -- validate_memory_path -------------------------------------------------

    #[test]
    fn validate_rejects_relative_path() {
        let err = validate_memory_path(Path::new("relative/path")).unwrap_err();
        assert!(matches!(err, MemoryError::PathValidation(_)));
        assert!(err.to_string().contains("absolute"));
    }

    #[cfg(unix)]
    #[test]
    fn validate_rejects_short_path() {
        let err = validate_memory_path(Path::new("/a")).unwrap_err();
        assert!(matches!(err, MemoryError::PathValidation(_)));
        assert!(err.to_string().contains("short"));
    }

    #[cfg(windows)]
    #[test]
    fn validate_rejects_short_path() {
        let err = validate_memory_path(Path::new("C:\\a")).unwrap_err();
        assert!(matches!(err, MemoryError::PathValidation(_)));
        assert!(err.to_string().contains("short"));
    }

    #[cfg(unix)]
    #[test]
    fn validate_rejects_traversal() {
        let err = validate_memory_path(Path::new("/tmp/../../../etc/passwd")).unwrap_err();
        assert!(matches!(err, MemoryError::PathValidation(_)));
        assert!(err.to_string().contains("traversal"));
    }

    #[cfg(windows)]
    #[test]
    fn validate_rejects_traversal() {
        let err = validate_memory_path(Path::new("C:\\tmp\\..\\..\\..\\etc\\passwd")).unwrap_err();
        assert!(matches!(err, MemoryError::PathValidation(_)));
        assert!(err.to_string().contains("traversal"));
    }

    #[cfg(unix)]
    #[test]
    fn validate_accepts_normal_absolute_path() {
        let result = validate_memory_path(Path::new("/tmp/memory/test.md"));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), PathBuf::from("/tmp/memory/test.md"));
    }

    #[cfg(windows)]
    #[test]
    fn validate_accepts_normal_absolute_path() {
        let result = validate_memory_path(Path::new("C:\\tmp\\memory\\test.md"));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), PathBuf::from("C:\\tmp\\memory\\test.md"));
    }

    // -- memory_entrypoint ----------------------------------------------------

    #[test]
    fn entrypoint_appends_memory_md() {
        let dir = Path::new("/base/memory");
        assert_eq!(memory_entrypoint(dir), PathBuf::from("/base/memory/MEMORY.md"));
    }

    // -- is_memory_path -------------------------------------------------------

    #[test]
    fn is_memory_path_inside() {
        // Use temp dir so paths actually exist for canonicalization
        let tmp = tempfile::tempdir().unwrap();
        let mem_dir = tmp.path().join("memory");
        fs::create_dir_all(&mem_dir).unwrap();
        let file = mem_dir.join("test.md");
        fs::write(&file, "").unwrap();

        assert!(is_memory_path(&file, &mem_dir));
    }

    #[test]
    fn is_memory_path_outside() {
        let tmp = tempfile::tempdir().unwrap();
        let mem_dir = tmp.path().join("memory");
        fs::create_dir_all(&mem_dir).unwrap();
        let outside = tmp.path().join("other.md");
        fs::write(&outside, "").unwrap();

        assert!(!is_memory_path(&outside, &mem_dir));
    }

    #[test]
    fn is_memory_path_nonexistent_returns_false() {
        // Non-existent paths with no common prefix
        assert!(!is_memory_path(
            Path::new("/nonexistent/a/b.md"),
            Path::new("/different/dir"),
        ));
    }

    #[test]
    fn is_memory_path_traversal_in_nonexistent_path_returns_false() {
        // Non-existent path with `..` must not bypass membership check
        // (regression test for review-1.3 ISSUE-1)
        assert!(!is_memory_path(
            Path::new("/base/memory/../../../etc/passwd"),
            Path::new("/base/memory"),
        ));
    }

    // -- ensure_memory_dir ----------------------------------------------------

    #[test]
    fn ensure_creates_nested_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let deep = tmp.path().join("a").join("b").join("c");
        assert!(!deep.exists());
        ensure_memory_dir(&deep).unwrap();
        assert!(deep.is_dir());
    }

    #[test]
    fn ensure_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("memory");
        ensure_memory_dir(&dir).unwrap();
        // Second call should not error
        ensure_memory_dir(&dir).unwrap();
        assert!(dir.is_dir());
    }

    // -- memory_base_dir (env override) ---------------------------------------

    #[test]
    #[serial(env)]
    fn base_dir_env_override() {
        let key = MEMORY_DIR_ENV;
        let original = std::env::var(key).ok();

        // SAFETY: #[serial(env)] ensures no concurrent env mutation.
        unsafe { std::env::set_var(key, "/custom/memory") };
        let result = memory_base_dir();
        assert_eq!(result, Some(PathBuf::from("/custom/memory")));

        restore_env(key, original);
    }

    #[test]
    #[serial(env)]
    fn base_dir_empty_env_falls_through() {
        let key = MEMORY_DIR_ENV;
        let original = std::env::var(key).ok();

        // SAFETY: #[serial(env)] ensures no concurrent env mutation.
        unsafe { std::env::set_var(key, "") };
        let result = memory_base_dir();
        // Should fall through to app_config_dir
        assert_ne!(result, Some(PathBuf::from("")));

        restore_env(key, original);
    }

    // -- auto_memory_dir ------------------------------------------------------

    #[test]
    #[serial(env)]
    fn auto_memory_dir_structure() {
        let key = MEMORY_DIR_ENV;
        let original = std::env::var(key).ok();

        // SAFETY: #[serial(env)] ensures no concurrent env mutation.
        unsafe { std::env::set_var(key, "/base") };
        let dir = auto_memory_dir(Path::new("/home/user/project")).unwrap();
        assert_eq!(dir, PathBuf::from("/base/projects/-home-user-project/memory"));

        restore_env(key, original);
    }

    fn restore_env(key: &str, saved: Option<String>) {
        // SAFETY: only called from #[serial(env)] tests.
        unsafe {
            match saved {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }

    // -- normalize_lexical ----------------------------------------------------

    #[test]
    fn normalize_collapses_dot() {
        let input = Path::new("/foo/./bar/./baz");
        assert_eq!(normalize_lexical(input), PathBuf::from("/foo/bar/baz"));
    }

    #[test]
    fn normalize_preserves_absolute() {
        let input = Path::new("/foo/bar");
        assert_eq!(normalize_lexical(input), PathBuf::from("/foo/bar"));
    }
}
