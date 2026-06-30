use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_safe() {
        assert_eq!(CompactLevel::default(), CompactLevel::Safe);
    }

    #[test]
    fn display_fromstr_roundtrip() {
        for level in [CompactLevel::Off, CompactLevel::Safe, CompactLevel::Full] {
            let s = level.to_string();
            let parsed: CompactLevel = s.parse().unwrap();
            assert_eq!(parsed, level);
        }
    }

    #[test]
    fn case_insensitive_parsing() {
        assert_eq!("OFF".parse::<CompactLevel>().unwrap(), CompactLevel::Off);
        assert_eq!("Safe".parse::<CompactLevel>().unwrap(), CompactLevel::Safe);
        assert_eq!("FULL".parse::<CompactLevel>().unwrap(), CompactLevel::Full);
    }

    #[test]
    fn invalid_input_error() {
        let err = "unknown".parse::<CompactLevel>().unwrap_err();
        assert!(err.contains("unknown compaction level"));
    }

    #[test]
    fn serde_roundtrip() {
        for level in [CompactLevel::Off, CompactLevel::Safe, CompactLevel::Full] {
            let json = serde_json::to_string(&level).unwrap();
            let back: CompactLevel = serde_json::from_str(&json).unwrap();
            assert_eq!(back, level);
        }
    }

    #[test]
    fn serde_lowercase_format() {
        assert_eq!(serde_json::to_string(&CompactLevel::Off).unwrap(), "\"off\"");
        assert_eq!(serde_json::to_string(&CompactLevel::Safe).unwrap(), "\"safe\"");
        assert_eq!(serde_json::to_string(&CompactLevel::Full).unwrap(), "\"full\"");
    }
}
