pub mod bundled;
pub mod conditional;
pub mod context_modifier;
pub mod discovery;
pub mod executor;
pub mod frontmatter;
pub mod hooks;
pub mod loader;
pub mod mcp;
pub mod paths;
pub mod permissions;
pub mod prompt;
pub mod shell;
pub mod substitution;
pub mod types;
pub mod watcher;

#[cfg(test)]
#[path = "permissions_supplemental_test.rs"]
mod permissions_supplemental_test;

#[cfg(test)]
#[path = "integration_test.rs"]
mod integration_test;

#[cfg(test)]
#[path = "bundled_supplemental_test.rs"]
mod bundled_supplemental_test;

#[cfg(test)]
#[path = "watcher_integration_test.rs"]
mod watcher_integration_test;
