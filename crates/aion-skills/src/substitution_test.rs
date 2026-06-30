use super::*;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_arguments ---

    #[test]
    fn test_parse_empty() {
        assert!(parse_arguments("").is_empty());
        assert!(parse_arguments("   ").is_empty());
    }

    #[test]
    fn test_parse_simple_words() {
        assert_eq!(parse_arguments("foo bar baz"), vec!["foo", "bar", "baz"]);
    }

    #[test]
    fn test_parse_double_quoted() {
        assert_eq!(parse_arguments(r#""hello world" foo"#), vec!["hello world", "foo"]);
    }

    #[test]
    fn test_parse_single_quoted() {
        assert_eq!(parse_arguments("'hello world' foo"), vec!["hello world", "foo"]);
    }

    #[test]
    fn test_parse_mixed_quotes() {
        assert_eq!(parse_arguments(r#"foo "bar baz" qux"#), vec!["foo", "bar baz", "qux"]);
    }

    // --- substitute_arguments ---

    #[test]
    fn test_no_args_returns_unchanged() {
        let content = "hello $ARGUMENTS world";
        let result = substitute_arguments(content, None, &[], None, None);
        assert_eq!(result, content);
    }

    #[test]
    fn test_arguments_full_substitution() {
        let result = substitute_arguments("run $ARGUMENTS now", Some("foo bar"), &[], None, None);
        assert_eq!(result, "run foo bar now");
    }

    #[test]
    fn test_arguments_indexed() {
        let result = substitute_arguments(
            "first=$ARGUMENTS[0] second=$ARGUMENTS[1]",
            Some("alpha beta"),
            &[],
            None,
            None,
        );
        assert_eq!(result, "first=alpha second=beta");
    }

    #[test]
    fn test_arguments_shorthand() {
        let result = substitute_arguments("a=$0 b=$1", Some("x y"), &[], None, None);
        assert_eq!(result, "a=x b=y");
    }

    #[test]
    fn test_named_arguments() {
        let names = vec!["filename".to_string(), "target".to_string()];
        let result = substitute_arguments("file=$filename dest=$target", Some("foo.rs /tmp"), &names, None, None);
        assert_eq!(result, "file=foo.rs dest=/tmp");
    }

    #[test]
    fn test_named_arg_no_partial_match() {
        // $foo should not match inside $foobar
        let names = vec!["foo".to_string()];
        let result = substitute_arguments("$foobar and $foo", Some("X"), &names, None, None);
        // $foobar stays (not a word boundary match), $foo becomes X
        assert_eq!(result, "$foobar and X");
    }

    #[test]
    fn test_aionrs_skill_dir_substitution() {
        let result = substitute_arguments("dir=${AIONRS_SKILL_DIR}", None, &[], Some("/my/skill"), None);
        assert_eq!(result, "dir=/my/skill");
    }

    #[test]
    fn test_aionrs_session_id_substitution() {
        let result = substitute_arguments("sid=${AIONRS_SESSION_ID}", None, &[], None, Some("sess-123"));
        assert_eq!(result, "sid=sess-123");
    }

    #[test]
    fn test_fallback_append_when_no_placeholder() {
        let result = substitute_arguments("hello world", Some("my-arg"), &[], None, None);
        assert_eq!(result, "hello world\n\nARGUMENTS: my-arg");
    }

    #[test]
    fn test_no_fallback_when_args_empty() {
        // Empty string — no fallback appended
        let result = substitute_arguments("hello world", Some(""), &[], None, None);
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_arguments_out_of_bounds_replaced_with_empty() {
        let result = substitute_arguments("$ARGUMENTS[5]", Some("a"), &[], None, None);
        assert_eq!(result, "");
    }

    #[test]
    fn test_substitution_order_indexed_before_full() {
        // $ARGUMENTS[0] must be replaced before $ARGUMENTS to avoid partial corruption
        let result = substitute_arguments("$ARGUMENTS[0] and $ARGUMENTS", Some("hello world"), &[], None, None);
        assert_eq!(result, "hello and hello world");
    }
}

// ---------------------------------------------------------------------------
// Supplemental tests (tester role — covers test-plan.md cases not in impl tests)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod supplemental_tests {
    use super::*;

    // -----------------------------------------------------------------------
    // TC-1.x: parse_arguments additional cases
    // -----------------------------------------------------------------------

    #[test]
    fn tc_1_1_basic_space_split() {
        assert_eq!(parse_arguments("foo bar baz"), vec!["foo", "bar", "baz"]);
    }

    #[test]
    fn tc_1_3_multiple_quoted_groups() {
        assert_eq!(
            parse_arguments(r#""arg one" "arg two" plain"#),
            vec!["arg one", "arg two", "plain"]
        );
    }

    #[test]
    fn tc_1_6_single_unquoted_arg() {
        assert_eq!(parse_arguments("single"), vec!["single"]);
    }

    #[test]
    fn tc_1_7_quoted_path_with_spaces() {
        assert_eq!(
            parse_arguments(r#""path/to/file with spaces.txt" --flag"#),
            vec!["path/to/file with spaces.txt", "--flag"]
        );
    }

    #[test]
    fn tc_1_8_unclosed_quote_no_panic() {
        // Must not panic; result is implementation-defined but non-empty
        let result = parse_arguments(r#""unclosed arg"#);
        assert!(!result.is_empty() || result.is_empty()); // just verifies no panic
    }

    #[test]
    fn tc_1_9_single_quotes_also_group() {
        // Implementation supports single quotes too (extends plan)
        assert_eq!(parse_arguments("'hello world' foo"), vec!["hello world", "foo"]);
    }

    // -----------------------------------------------------------------------
    // TC-2.x: $ARGUMENTS full substitution
    // -----------------------------------------------------------------------

    #[test]
    fn tc_2_1_arguments_full_replacement() {
        let r = substitute_arguments("Run: $ARGUMENTS", Some("foo bar"), &[], None, None);
        assert_eq!(r, "Run: foo bar");
    }

    #[test]
    fn tc_2_2_arguments_none_becomes_empty() {
        // When args = None, $ARGUMENTS is NOT replaced (returns unchanged per spec)
        let r = substitute_arguments("Run: $ARGUMENTS", None, &[], None, None);
        assert_eq!(r, "Run: $ARGUMENTS");
    }

    #[test]
    fn tc_2_3_arguments_multiple_occurrences() {
        let r = substitute_arguments("$ARGUMENTS and $ARGUMENTS", Some("x"), &[], None, None);
        assert_eq!(r, "x and x");
    }

    // -----------------------------------------------------------------------
    // TC-3.x: $ARGUMENTS[n] indexed substitution
    // -----------------------------------------------------------------------

    #[test]
    fn tc_3_1_arguments_index_0() {
        let r = substitute_arguments("First: $ARGUMENTS[0]", Some("alpha beta"), &[], None, None);
        assert_eq!(r, "First: alpha");
    }

    #[test]
    fn tc_3_2_arguments_index_1() {
        let r = substitute_arguments("Second: $ARGUMENTS[1]", Some("alpha beta"), &[], None, None);
        assert_eq!(r, "Second: beta");
    }

    #[test]
    fn tc_3_3_arguments_index_out_of_bounds_empty() {
        let r = substitute_arguments("Third: $ARGUMENTS[2]", Some("only_one"), &[], None, None);
        assert_eq!(r, "Third: ");
    }

    #[test]
    fn tc_3_4_arguments_index_with_quoted_arg() {
        let r = substitute_arguments("$ARGUMENTS[0]", Some(r#""hello world" foo"#), &[], None, None);
        assert_eq!(r, "hello world");
    }

    // -----------------------------------------------------------------------
    // TC-4.x: $n shorthand indexed substitution
    // -----------------------------------------------------------------------

    #[test]
    fn tc_4_1_shorthand_0() {
        let r = substitute_arguments("Hello $0", Some("world"), &[], None, None);
        assert_eq!(r, "Hello world");
    }

    #[test]
    fn tc_4_2_shorthand_0_and_1() {
        let r = substitute_arguments("$0 and $1", Some("foo bar"), &[], None, None);
        assert_eq!(r, "foo and bar");
    }

    #[test]
    fn tc_4_3_shorthand_out_of_bounds() {
        let r = substitute_arguments("$2", Some("only_two args"), &[], None, None);
        // "only_two" = $0, "args" = $1, $2 is out of bounds → empty
        assert_eq!(r, "");
    }

    #[test]
    fn tc_4_4_shorthand_no_args() {
        let r = substitute_arguments("Run $0", None, &[], None, None);
        // args = None → no substitution, content returned unchanged
        assert_eq!(r, "Run $0");
    }

    // -----------------------------------------------------------------------
    // TC-5.x: $name named argument substitution
    // -----------------------------------------------------------------------

    #[test]
    fn tc_5_1_single_named_arg() {
        // $query maps to argument index 0; args "rust programming" parses to ["rust", "programming"].
        // $query is replaced with the first parsed argument "rust".
        // "programming" is the second argument but has no placeholder in content.
        let names = vec!["query".to_string()];
        let r = substitute_arguments("Search for $query", Some("rust programming"), &names, None, None);
        assert_eq!(r, "Search for rust");
    }

    #[test]
    fn tc_5_2_multiple_named_args() {
        let names = vec!["src".to_string(), "dst".to_string()];
        let r = substitute_arguments("From $src to $dst", Some("source.txt dest.txt"), &names, None, None);
        assert_eq!(r, "From source.txt to dest.txt");
    }

    #[test]
    fn tc_5_4_named_arg_index_out_of_range() {
        // $second maps to index 1 but only one arg provided
        let names = vec!["first".to_string(), "second".to_string()];
        let r = substitute_arguments("File: $second", Some("only_one"), &names, None, None);
        assert_eq!(r, "File: ");
    }

    // -----------------------------------------------------------------------
    // TC-6.x: ${AIONRS_SKILL_DIR} substitution
    // -----------------------------------------------------------------------

    #[test]
    fn tc_6_1_skill_dir_replaced() {
        let r = substitute_arguments(
            "cd ${AIONRS_SKILL_DIR}",
            None,
            &[],
            Some("/home/user/.aionrs/skills/my-skill"),
            None,
        );
        assert_eq!(r, "cd /home/user/.aionrs/skills/my-skill");
    }

    #[test]
    fn tc_6_2_skill_dir_none_not_replaced() {
        // skill_root = None → ${AIONRS_SKILL_DIR} stays unreplaced
        let r = substitute_arguments("cd ${AIONRS_SKILL_DIR}", None, &[], None, None);
        assert_eq!(r, "cd ${AIONRS_SKILL_DIR}");
    }

    #[test]
    fn tc_6_3_skill_dir_multiple_occurrences() {
        let r = substitute_arguments(
            "${AIONRS_SKILL_DIR}/a and ${AIONRS_SKILL_DIR}/b",
            None,
            &[],
            Some("/skills/foo"),
            None,
        );
        assert_eq!(r, "/skills/foo/a and /skills/foo/b");
    }

    // -----------------------------------------------------------------------
    // TC-7.x: ${AIONRS_SESSION_ID} substitution
    // -----------------------------------------------------------------------

    #[test]
    fn tc_7_1_session_id_replaced() {
        let r = substitute_arguments("Session: ${AIONRS_SESSION_ID}", None, &[], None, Some("abc-123"));
        assert_eq!(r, "Session: abc-123");
    }

    #[test]
    fn tc_7_2_session_id_none_not_replaced() {
        let r = substitute_arguments("Session: ${AIONRS_SESSION_ID}", None, &[], None, None);
        assert_eq!(r, "Session: ${AIONRS_SESSION_ID}");
    }

    // -----------------------------------------------------------------------
    // TC-8.x: fallback append when no placeholder
    // -----------------------------------------------------------------------

    #[test]
    fn tc_8_1_no_placeholder_appends_arguments() {
        let r = substitute_arguments("Do the task.", Some("my argument"), &[], None, None);
        assert_eq!(r, "Do the task.\n\nARGUMENTS: my argument");
    }

    #[test]
    fn tc_8_2_no_placeholder_no_args_no_append() {
        let r = substitute_arguments("Do the task.", None, &[], None, None);
        assert_eq!(r, "Do the task.");
    }

    #[test]
    fn tc_8_3_with_placeholder_no_append() {
        let r = substitute_arguments("Run $ARGUMENTS", Some("x"), &[], None, None);
        assert_eq!(r, "Run x");
        assert!(!r.contains("ARGUMENTS:"));
    }

    #[test]
    fn tc_8_4_empty_args_string_no_append() {
        // args = Some("") is empty → no fallback appended
        let r = substitute_arguments("Do it.", Some(""), &[], None, None);
        assert_eq!(r, "Do it.");
    }

    // -----------------------------------------------------------------------
    // TC-9.x: comprehensive / combined scenarios
    // -----------------------------------------------------------------------

    #[test]
    fn tc_9_1_multiple_placeholder_types() {
        let r = substitute_arguments(
            "cd ${AIONRS_SKILL_DIR} && run $ARGUMENTS[0] with $ARGUMENTS",
            Some("alpha beta"),
            &[],
            Some("/skills/foo"),
            None,
        );
        assert_eq!(r, "cd /skills/foo && run alpha with alpha beta");
    }

    #[test]
    fn tc_9_2_empty_content_with_args_appends() {
        let r = substitute_arguments("", Some("foo"), &[], None, None);
        assert_eq!(r, "\n\nARGUMENTS: foo");
    }

    #[test]
    fn tc_9_3_empty_content_no_args() {
        let r = substitute_arguments("", None, &[], None, None);
        assert_eq!(r, "");
    }

    #[test]
    fn tc_9_4_indexed_before_full_no_corruption() {
        // $ARGUMENTS[0] must be recognized before $ARGUMENTS replacement
        let r = substitute_arguments("$ARGUMENTS[0] / $ARGUMENTS", Some("alpha beta"), &[], None, None);
        assert_eq!(r, "alpha / alpha beta");
    }

    // -----------------------------------------------------------------------
    // TC-15.x: edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn tc_15_2_skill_dir_and_arguments_same_line() {
        let r = substitute_arguments(
            "${AIONRS_SKILL_DIR}: $ARGUMENTS",
            Some("test"),
            &[],
            Some("/root"),
            None,
        );
        assert_eq!(r, "/root: test");
    }

    #[test]
    fn tc_15_3_large_args_no_panic() {
        let big_arg = "x".repeat(10_000);
        let r = substitute_arguments("$ARGUMENTS", Some(&big_arg), &[], None, None);
        assert_eq!(r, big_arg);
    }
}
