pub mod fold;
pub mod json;
pub mod level;
pub mod sanitize;
pub mod toon;

pub use level::CompactLevel;
pub use toon::toon_format_instructions;

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
#[path = "lib_test.rs"]
mod lib_test;
