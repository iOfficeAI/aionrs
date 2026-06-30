use super::*;

// ===========================================================================
// Unit tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -- format_size ----------------------------------------------------------

    #[test]
    fn format_size_bytes() {
        assert_eq!(format_size(500), "500 B");
    }

    #[test]
    fn format_size_kilobytes() {
        assert_eq!(format_size(25_000), "24.4 KB");
    }

    #[test]
    fn format_size_zero() {
        assert_eq!(format_size(0), "0 B");
    }

    // -- truncate_index: no truncation ----------------------------------------

    #[test]
    fn no_truncation_small_content() {
        let content = "- [A](a.md) — summary\n- [B](b.md) — summary\n";
        let result = truncate_index(content);
        assert!(!result.was_truncated);
        assert_eq!(result.line_count, 2);
        assert_eq!(result.content, content.trim());
    }

    #[test]
    fn no_truncation_empty() {
        let result = truncate_index("");
        assert!(!result.was_truncated);
        assert_eq!(result.line_count, 0);
        assert_eq!(result.byte_count, 0);
        assert_eq!(result.content, "");
    }

    #[test]
    fn no_truncation_whitespace_only() {
        let result = truncate_index("   \n  \n  ");
        assert!(!result.was_truncated);
        assert_eq!(result.content, "");
    }

    #[test]
    fn no_truncation_exactly_200_lines() {
        let content = (0..200).map(|i| format!("- line {i}")).collect::<Vec<_>>().join("\n");
        let result = truncate_index(&content);
        assert!(!result.was_truncated);
        assert_eq!(result.line_count, 200);
    }

    #[test]
    fn no_truncation_exactly_25000_bytes() {
        // 100 lines (under 200 limit) totalling exactly 25000 bytes.
        // 100 lines joined by 99 newlines: each line = (25000 - 99) / 100 = 249 chars,
        // remainder 1 added to last line.
        let per_line = (MAX_INDEX_BYTES - 99) / 100; // 249
        let remainder = MAX_INDEX_BYTES - 99 - per_line * 100;
        let mut lines: Vec<String> = (0..100).map(|_| "x".repeat(per_line)).collect();
        if remainder > 0 {
            lines.last_mut().unwrap().push_str(&"x".repeat(remainder));
        }
        let content = lines.join("\n");
        assert_eq!(content.len(), MAX_INDEX_BYTES);
        let result = truncate_index(&content);
        assert!(!result.was_truncated);
    }

    // -- truncate_index: line truncation --------------------------------------

    #[test]
    fn line_truncation_250_lines() {
        let lines: Vec<String> = (0..250).map(|i| format!("- line {i}")).collect();
        let content = lines.join("\n");
        let result = truncate_index(&content);

        assert!(result.was_truncated);
        assert_eq!(result.line_count, 250);
        // Content should contain only first 200 lines (before warning)
        let content_before_warning = result.content.split("\n\n> WARNING:").next().unwrap();
        let output_lines: Vec<&str> = content_before_warning.split('\n').collect();
        assert_eq!(output_lines.len(), 200);
        assert!(result.content.contains("250 lines"));
        assert!(result.content.contains("WARNING"));
    }

    #[test]
    fn line_truncation_201_lines() {
        let lines: Vec<String> = (0..201).map(|i| format!("- line {i}")).collect();
        let content = lines.join("\n");
        let result = truncate_index(&content);

        assert!(result.was_truncated);
        assert_eq!(result.line_count, 201);
    }

    // -- truncate_index: byte truncation --------------------------------------

    #[test]
    fn byte_truncation_long_lines() {
        // 100 lines, each 300 bytes = 30000 bytes > 25000
        let lines: Vec<String> = (0..100).map(|i| format!("{i:03}: {}", "x".repeat(296))).collect();
        let content = lines.join("\n");
        assert!(content.len() > MAX_INDEX_BYTES);

        let result = truncate_index(&content);
        assert!(result.was_truncated);
        assert_eq!(result.line_count, 100);
        // Warning should mention byte size, not line count
        assert!(result.content.contains("index entries are too long"));
    }

    #[test]
    fn byte_truncation_cuts_at_newline() {
        // Create content just over the byte limit
        let line = "a".repeat(250);
        let lines: Vec<String> = (0..110).map(|_| line.clone()).collect();
        let content = lines.join("\n");
        assert!(content.len() > MAX_INDEX_BYTES);

        let result = truncate_index(&content);
        assert!(result.was_truncated);

        // Content before warning should end at a line boundary
        let before_warning = result.content.split("\n\n> WARNING:").next().unwrap();
        // Every line should be complete (not cut mid-content)
        for line in before_warning.lines() {
            assert!(
                line.len() == 250 || line.is_empty(),
                "unexpected line length: {} for {:?}",
                line.len(),
                &line[..line.len().min(40)]
            );
        }
    }

    // -- truncate_index: both limits ------------------------------------------

    #[test]
    fn both_line_and_byte_truncation() {
        // 300 lines of 200 bytes each = 60000 bytes; both limits exceeded
        let lines: Vec<String> = (0..300).map(|i| format!("{i:03}: {}", "y".repeat(196))).collect();
        let content = lines.join("\n");

        let result = truncate_index(&content);
        assert!(result.was_truncated);
        assert_eq!(result.line_count, 300);
        // Warning should mention both lines and bytes
        assert!(result.content.contains("300 lines"));
        assert!(result.content.contains("KB"));
    }

    // -- truncate_index: single long line (no newline to cut at) ---------------

    #[test]
    fn single_long_line_fallback() {
        let content = "z".repeat(30_000);
        let result = truncate_index(&content);

        assert!(result.was_truncated);
        // Should truncate at MAX_INDEX_BYTES
        let before_warning = result.content.split("\n\n> WARNING:").next().unwrap();
        assert_eq!(before_warning.len(), MAX_INDEX_BYTES);
    }

    // -- truncate_index: preserves content integrity ---------------------------

    #[test]
    fn truncation_preserves_first_200_lines() {
        let lines: Vec<String> = (0..250)
            .map(|i| format!("- [{i}](file_{i}.md) \u{2014} memory number {i}"))
            .collect();
        let content = lines.join("\n");
        let result = truncate_index(&content);

        // First line should be present
        assert!(result.content.contains("- [0](file_0.md)"));
        // Line 199 should be present
        assert!(result.content.contains("- [199](file_199.md)"));
        // Line 200 should NOT be present (0-indexed, so that's the 201st)
        assert!(!result.content.contains("- [200](file_200.md)"));
    }

    // -- append entry (unit-level, using temp files) --------------------------

    #[test]
    fn append_to_new_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("MEMORY.md");

        append_index_entry(&path, "Role", "user_role.md", "user role info").unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "- [Role](user_role.md) \u{2014} user role info\n");
    }

    #[test]
    fn append_to_existing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("MEMORY.md");
        fs::write(&path, "- [A](a.md) \u{2014} first\n").unwrap();

        append_index_entry(&path, "B", "b.md", "second").unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "- [A](a.md) \u{2014} first\n- [B](b.md) \u{2014} second\n");
    }

    #[test]
    fn append_to_file_without_trailing_newline() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("MEMORY.md");
        fs::write(&path, "- [A](a.md) \u{2014} first").unwrap();

        append_index_entry(&path, "B", "b.md", "second").unwrap();

        let content = fs::read_to_string(&path).unwrap();
        // Should have a newline between entries
        assert!(content.contains("first\n- [B]"));
    }

    #[test]
    fn append_creates_parent_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("sub").join("dir").join("MEMORY.md");

        append_index_entry(&path, "Test", "test.md", "testing").unwrap();
        assert!(path.exists());
    }

    // -- remove entry (unit-level, using temp files) --------------------------

    #[test]
    fn remove_existing_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("MEMORY.md");
        fs::write(
            &path,
            "- [A](a.md) \u{2014} first\n- [B](b.md) \u{2014} second\n- [C](c.md) \u{2014} third\n",
        )
        .unwrap();

        remove_index_entry(&path, "b.md").unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "- [A](a.md) \u{2014} first\n- [C](c.md) \u{2014} third\n");
    }

    #[test]
    fn remove_nonexistent_entry_is_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("MEMORY.md");
        let original = "- [A](a.md) \u{2014} first\n";
        fs::write(&path, original).unwrap();

        remove_index_entry(&path, "nonexistent.md").unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, original);
    }

    #[test]
    fn remove_from_nonexistent_file_is_ok() {
        let path = Path::new("/nonexistent/MEMORY.md");
        // Should not error
        remove_index_entry(path, "anything.md").unwrap();
    }

    #[test]
    fn remove_last_entry_leaves_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("MEMORY.md");
        fs::write(&path, "- [A](a.md) \u{2014} only\n").unwrap();

        remove_index_entry(&path, "a.md").unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "");
    }

    // -- read_index (unit-level) ----------------------------------------------

    #[test]
    fn read_nonexistent_returns_empty() {
        let result = read_index(Path::new("/nonexistent/MEMORY.md"));
        assert_eq!(result, "");
    }

    #[test]
    fn read_existing_returns_content() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("MEMORY.md");
        fs::write(&path, "# Index\n- [A](a.md)\n").unwrap();

        let result = read_index(&path);
        assert_eq!(result, "# Index\n- [A](a.md)\n");
    }
}
