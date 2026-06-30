use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_identical_consecutive_lines() {
        let input = "ok\nok\nok\nok\nok\ndone";
        let result = fold_repeated_lines(input);
        assert!(result.contains("[... 3 identical lines]"));
        assert!(result.contains("ok"));
        assert!(result.contains("done"));
    }

    #[test]
    fn fold_no_repeats_unchanged() {
        let input = "apple\nbanana\ncherry";
        assert_eq!(fold_repeated_lines(input), input);
    }

    #[test]
    fn fold_similar_prefix_lines() {
        let lines: Vec<String> = (0..10).map(|i| format!("Compiling crate-{i} v0.1.0")).collect();
        let input = lines.join("\n");
        let result = fold_repeated_lines(&input);
        assert!(result.contains("[... 8 similar lines]"));
        assert!(result.contains("Compiling crate-0"));
        assert!(result.contains("Compiling crate-9"));
    }

    #[test]
    fn fold_below_threshold_unchanged() {
        let input = "Compiling a v0.1.0\nCompiling b v0.1.0\ndone";
        assert_eq!(fold_repeated_lines(input), input);
    }

    #[test]
    fn fold_mixed_groups() {
        let mut lines = Vec::new();
        for i in 0..6 {
            lines.push(format!("Downloading dep-{i}..."));
        }
        lines.push("Install complete".to_string());
        for i in 0..5 {
            lines.push(format!("Compiling mod-{i}"));
        }
        let input = lines.join("\n");
        let result = fold_repeated_lines(&input);
        assert!(result.contains("[... 4 similar lines]"), "first group folded: {result}");
        assert!(result.contains("Install complete"));
        assert!(
            result.contains("[... 3 similar lines]"),
            "second group folded: {result}"
        );
    }

    #[test]
    fn fold_empty_input() {
        assert_eq!(fold_repeated_lines(""), "");
    }
}
