use crate::level::CompactLevel;
use crate::{fold, json, sanitize, toon};

pub fn compact_output(text: &str, level: CompactLevel) -> String {
    match level {
        CompactLevel::Off => text.to_string(),
        CompactLevel::Safe => sanitize::sanitize(text),
        CompactLevel::Full => {
            let text = sanitize::sanitize(text);
            let text = fold::fold_repeated_lines(&text);
            json::compact_json(&text)
        }
    }
}

pub fn compact_output_toon(text: &str) -> String {
    toon::try_toon_encode(text)
}

#[cfg(test)]
#[path = "api_test.rs"]
mod api_test;
