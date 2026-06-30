use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_color_codes() {
        let input = "\x1b[31mError\x1b[0m: something failed";
        assert_eq!(strip_ansi(input), "Error: something failed");
    }

    #[test]
    fn strip_ansi_bold_and_nested() {
        let input = "\x1b[1m\x1b[32mCompiling\x1b[0m aion-compact v0.1.0";
        assert_eq!(strip_ansi(input), "Compiling aion-compact v0.1.0");
    }

    #[test]
    fn strip_ansi_no_codes_unchanged() {
        let input = "plain text without any codes";
        assert_eq!(strip_ansi(input), input);
    }

    #[test]
    fn strip_ansi_cursor_movement() {
        let input = "\x1b[2K\x1b[1G> prompt";
        assert_eq!(strip_ansi(input), "> prompt");
    }

    #[test]
    fn strip_ansi_empty_input() {
        assert_eq!(strip_ansi(""), "");
    }

    // --- collapse_cr_lines ---

    #[test]
    fn collapse_cr_overwrites() {
        let input = "Downloading... 10%\rDownloading... 50%\rDownloading... 100%\nDone.";
        assert_eq!(collapse_cr_lines(input), "Downloading... 100%\nDone.");
    }

    #[test]
    fn collapse_cr_preserves_crlf_line_content() {
        let input = "Exit code: 0\nSTDOUT:\nmessage\r\nSTDERR:";
        assert_eq!(collapse_cr_lines(input), "Exit code: 0\nSTDOUT:\nmessage\nSTDERR:");
    }

    #[test]
    fn collapse_cr_preserves_overwrite_semantics_with_crlf() {
        let input = "Downloading... 10%\rDownloading... 100%\r\nDone\r\nLast";
        assert_eq!(collapse_cr_lines(input), "Downloading... 100%\nDone\nLast");
    }

    #[test]
    fn collapse_cr_no_cr_unchanged() {
        let input = "line1\nline2\nline3";
        assert_eq!(collapse_cr_lines(input), input);
    }

    // --- merge_blank_lines ---

    #[test]
    fn merge_consecutive_blank_lines() {
        let input = "a\n\n\n\n\nb";
        assert_eq!(merge_blank_lines(input), "a\n\nb");
    }

    #[test]
    fn merge_blank_lines_preserves_single() {
        let input = "a\n\nb\n\nc";
        assert_eq!(merge_blank_lines(input), input);
    }

    #[test]
    fn merge_blank_lines_whitespace_only_lines() {
        let input = "a\n   \n  \n\nb";
        assert_eq!(merge_blank_lines(input), "a\n\nb");
    }

    // --- trim_trailing_whitespace ---

    #[test]
    fn trim_trailing_spaces() {
        let input = "hello   \nworld\t\t\nfoo";
        assert_eq!(trim_trailing_whitespace(input), "hello\nworld\nfoo");
    }

    #[test]
    fn trim_trailing_no_trailing() {
        let input = "clean\nlines";
        assert_eq!(trim_trailing_whitespace(input), input);
    }

    // --- sanitize (combined safe layer) ---

    #[test]
    fn sanitize_applies_all() {
        let input = "\x1b[32mCompiling\x1b[0m foo   \n\n\n\nbar\rbar done\n";
        let result = sanitize(input);
        assert!(!result.contains("\x1b["));
        assert!(!result.contains("\n\n\n"));
        assert!(!result.contains("   \n"));
        assert!(result.contains("bar done"));
    }

    #[test]
    fn sanitize_preserves_windows_command_stdout() {
        let input = "Exit code: 0\nSTDOUT:\nmessage\r\nSTDERR:";
        let result = sanitize(input);
        assert_eq!(result, "Exit code: 0\nSTDOUT:\nmessage\nSTDERR:");
    }

    #[test]
    fn sanitize_preserves_unix_command_stdout() {
        let input = "Exit code: 0\nSTDOUT:\nmessage\nSTDERR:";
        let result = sanitize(input);
        assert_eq!(result, input);
    }
}
