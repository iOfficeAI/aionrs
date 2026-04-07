pub mod bundled;
pub mod conditional;
pub mod context_modifier;
pub mod discovery;
pub mod executor;
pub mod frontmatter;
pub mod loader;
pub mod paths;
pub mod permissions;
pub mod prompt;
pub mod shell;
pub mod substitution;
pub mod types;

#[cfg(test)]
mod permissions_supplemental_tests;

#[cfg(test)]
mod bundled_supplemental_tests;
