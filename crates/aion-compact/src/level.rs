use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CompactLevel {
    Off,
    #[default]
    Safe,
    Full,
}

impl fmt::Display for CompactLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Off => write!(f, "off"),
            Self::Safe => write!(f, "safe"),
            Self::Full => write!(f, "full"),
        }
    }
}

impl FromStr for CompactLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "off" => Ok(Self::Off),
            "safe" => Ok(Self::Safe),
            "full" => Ok(Self::Full),
            other => Err(format!(
                "unknown compaction level: '{other}' (expected: off, safe, full)"
            )),
        }
    }
}

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
