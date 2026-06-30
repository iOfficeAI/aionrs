use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_formatter_no_color_mode() {
        // Verify construction with no_color=true does not panic
        let _formatter = OutputFormatter::new(true);
    }

    #[test]
    fn test_text_truncation_short_string_unchanged() {
        let result = truncate_display("hello", 10);
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_text_truncation_exact_length_unchanged() {
        let result = truncate_display("helloworld", 10);
        assert_eq!(result, "helloworld");
    }

    #[test]
    fn test_text_truncation_long_string_truncated() {
        let result = truncate_display("hello world this is long", 10);
        assert_eq!(result, "hello worl...");
    }

    #[test]
    fn test_text_truncation_empty_string() {
        let result = truncate_display("", 10);
        assert_eq!(result, "");
    }

    #[test]
    fn test_turn_stats_no_panic() {
        let formatter = OutputFormatter::new(true);
        // Verify turn_stats does not panic with various inputs.
        formatter.turn_stats(1, 100, 50, 0, 0);
        formatter.turn_stats(5, 1000, 500, 200, 300);
        formatter.turn_stats(0, 0, 0, 0, 0);
    }

    #[test]
    fn test_text_truncation_cjk_does_not_panic() {
        // Each CJK char is 3 bytes; byte-based slicing at max=200 would land
        // mid-character and panic without the char_indices fix.
        let cjk: String = "你好世界测试".chars().cycle().take(200).collect();
        let result = truncate_display(&cjk, 50);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_text_truncation_mixed_cjk_ascii_does_not_panic() {
        let mixed = "abc你好def世界ghi测试".repeat(20);
        let result = truncate_display(&mixed, 30);
        assert!(result.ends_with("..."));
    }
}
