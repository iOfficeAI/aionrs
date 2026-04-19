pub mod fold;
pub mod level;
pub mod sanitize;

pub use level::CompactionLevel;

pub fn compact_output(text: &str, level: CompactionLevel) -> String {
    match level {
        CompactionLevel::Off => text.to_string(),
        CompactionLevel::Safe => sanitize::sanitize(text),
        CompactionLevel::Full => sanitize::sanitize(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_returns_unchanged() {
        let input = "hello\x1b[31m world\n\n\nfoo";
        assert_eq!(compact_output(input, CompactionLevel::Off), input);
    }

    #[test]
    fn safe_strips_ansi() {
        let input = "\x1b[32mOK\x1b[0m done";
        let result = compact_output(input, CompactionLevel::Safe);
        assert_eq!(result, "OK done");
    }

    #[test]
    fn safe_merges_blank_lines() {
        let input = "a\n\n\n\nb";
        let result = compact_output(input, CompactionLevel::Safe);
        assert_eq!(result, "a\n\nb");
    }

    #[test]
    fn safe_collapses_cr() {
        let input = "50%\r100%\nDone";
        let result = compact_output(input, CompactionLevel::Safe);
        assert_eq!(result, "100%\nDone");
    }
}
