mod api;
pub mod fold;
pub mod json;
pub mod level;
pub mod sanitize;
pub mod toon;

pub use api::{compact_output, compact_output_toon};
pub use level::CompactLevel;
pub use toon::toon_format_instructions;
