use super::*;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[cfg(not(windows))] // Path handling differs on Windows; skip these tests there
mod tests {
    use std::path::Path;

    use super::{compile_patterns, relativize};

    // --- compile_patterns ---

    #[test]
    fn compile_patterns_valid_returns_all() {
        let patterns = compile_patterns("skill", &["src/**/*.rs".to_string(), "*.ts".to_string()]);
        assert_eq!(patterns.len(), 2);
    }

    // NOTE: glob::Pattern accepts "!negation" as a valid (literal) pattern —
    // the "!" character is not special in glob::Pattern, only in gitignore.
    // compile_patterns skips patterns that glob::Pattern::new rejects (e.g.
    // patterns with unclosed brackets like "[bad"), not gitignore-style "!".
    #[test]
    fn compile_patterns_unclosed_bracket_skipped_no_panic() {
        // "[unclosed" is syntactically invalid for glob::Pattern — should be skipped
        let patterns = compile_patterns("skill", &["[unclosed".to_string()]);
        assert_eq!(patterns.len(), 0);
    }

    #[test]
    fn compile_patterns_mixed_keeps_valid_drops_syntactically_invalid() {
        let patterns = compile_patterns("skill", &["[bad".to_string(), "src/**/*.rs".to_string()]);
        assert_eq!(patterns.len(), 1);
        assert!(patterns[0].matches("src/lib.rs"));
    }

    #[test]
    fn compile_patterns_empty_input_returns_empty() {
        let patterns = compile_patterns("skill", &[]);
        assert!(patterns.is_empty());
    }

    // --- relativize ---

    #[test]
    fn relativize_absolute_under_cwd_returns_relative() {
        let cwd = Path::new("/project");
        let result = relativize("/project/src/lib.rs", cwd);
        assert_eq!(result, Some("src/lib.rs".to_string()));
    }

    #[test]
    fn relativize_absolute_outside_cwd_returns_none() {
        let cwd = Path::new("/project");
        let result = relativize("/other/file.rs", cwd);
        assert!(result.is_none());
    }

    #[test]
    fn relativize_empty_string_returns_none() {
        let cwd = Path::new("/project");
        let result = relativize("", cwd);
        assert!(result.is_none());
    }

    #[test]
    fn relativize_path_equal_to_cwd_returns_none() {
        // strip_prefix of cwd from itself → empty string → rejected
        let cwd = Path::new("/project");
        let result = relativize("/project", cwd);
        assert!(result.is_none());
    }

    #[test]
    fn relativize_relative_input_returned_as_is() {
        // Non-absolute paths are passed through (caller's responsibility to provide absolute)
        let cwd = Path::new("/project");
        let result = relativize("src/lib.rs", cwd);
        assert_eq!(result, Some("src/lib.rs".to_string()));
    }
}

// ---------------------------------------------------------------------------
// Supplemental tests (tester role — covers test-plan.md cases)
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "conditional_supplemental_test.rs"]
mod conditional_supplemental_test;
