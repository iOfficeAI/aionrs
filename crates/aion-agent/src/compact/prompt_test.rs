use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    // ── build_compact_prompt ────────────────────────────────────────────

    #[test]
    fn prompt_contains_all_nine_sections() {
        let prompt = build_compact_prompt();
        for i in 1..=9 {
            assert!(prompt.contains(&format!("{i}.")), "Missing section {i}");
        }
    }

    #[test]
    fn prompt_forbids_tool_calls() {
        let prompt = build_compact_prompt();
        assert!(prompt.contains("Do NOT call any tools"));
        assert!(prompt.contains("CRITICAL"));
    }

    #[test]
    fn prompt_requires_analysis_and_summary_tags() {
        let prompt = build_compact_prompt();
        assert!(prompt.contains("<analysis>"));
        assert!(prompt.contains("<summary>"));
    }

    // ── format_compact_summary ──────────────────────────────────────────

    #[test]
    fn strips_analysis_extracts_summary() {
        let raw = "<analysis>thinking about things</analysis>\n<summary>the actual result</summary>";
        assert_eq!(format_compact_summary(raw), "Summary:\nthe actual result");
    }

    #[test]
    fn extracts_summary_without_analysis() {
        let raw = "<summary>result only</summary>";
        assert_eq!(format_compact_summary(raw), "Summary:\nresult only");
    }

    #[test]
    fn graceful_degradation_without_tags() {
        let raw = "plain text without any tags";
        assert_eq!(format_compact_summary(raw), "plain text without any tags");
    }

    #[test]
    fn handles_multiline_summary() {
        let raw = "<analysis>analysis\nwith lines</analysis>\n<summary>\nLine 1\nLine 2\n</summary>";
        let result = format_compact_summary(raw);
        assert!(result.starts_with("Summary:\n"));
        assert!(result.contains("Line 1"));
        assert!(result.contains("Line 2"));
    }

    #[test]
    fn empty_summary_tags_falls_back() {
        let raw = "<analysis>thinking</analysis>\n<summary></summary>";
        let result = format_compact_summary(raw);
        // Falls back since summary content is empty
        assert!(!result.is_empty());
    }

    // ── build_summary_content ───────────────────────────────────────────

    #[test]
    fn auto_summary_includes_continuation_instruction() {
        let content = build_summary_content("Summary:\ntest", true);
        assert!(content.contains("Continue the conversation"));
        assert!(content.contains("as if the break never happened"));
    }

    #[test]
    fn manual_summary_no_continuation_instruction() {
        let content = build_summary_content("Summary:\ntest", false);
        assert!(!content.contains("Continue the conversation"));
    }

    #[test]
    fn summary_content_includes_session_header() {
        let content = build_summary_content("Summary:\ntest", false);
        assert!(content.contains("This session is being continued"));
    }

    // ── strip_tag ───────────────────────────────────────────────────────

    #[test]
    fn strip_tag_removes_complete_tag() {
        let text = "before<foo>inside</foo>after";
        assert_eq!(strip_tag(text, "foo"), "beforeafter");
    }

    #[test]
    fn strip_tag_noop_when_tag_missing() {
        let text = "no tags here";
        assert_eq!(strip_tag(text, "foo"), "no tags here");
    }

    #[test]
    fn strip_tag_noop_when_reversed_order() {
        // Closing tag before opening tag should be treated as no-op
        let text = "before</foo>middle<foo>inside</foo>after";
        // The first </foo> is at position 6, first <foo> is at position 17
        // Since end < start, the text should be returned unchanged
        assert_eq!(strip_tag(text, "foo"), text);
    }

    // ── extract_tag_content ─────────────────────────────────────────────

    #[test]
    fn extract_existing_tag() {
        let text = "<summary>hello world</summary>";
        assert_eq!(extract_tag_content(text, "summary"), Some("hello world"));
    }

    #[test]
    fn extract_missing_tag() {
        let text = "no summary here";
        assert_eq!(extract_tag_content(text, "summary"), None);
    }

    // ── collapse_blank_lines ────────────────────────────────────────────

    #[test]
    fn collapses_multiple_blank_lines() {
        let text = "a\n\n\n\nb";
        let result = collapse_blank_lines(text);
        assert_eq!(result, "a\n\nb");
    }

    #[test]
    fn preserves_single_blank_line() {
        let text = "a\n\nb";
        assert_eq!(collapse_blank_lines(text), "a\n\nb");
    }
}
