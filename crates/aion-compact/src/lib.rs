pub mod level;
pub mod sanitize;

pub use level::CompactionLevel;

/// Compact the given text according to the specified [`CompactionLevel`].
pub fn compact_output(text: &str, level: CompactionLevel) -> String {
    match level {
        CompactionLevel::Off => text.to_string(),
        CompactionLevel::Safe => text.to_string(),
        CompactionLevel::Full => text.to_string(),
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
}
