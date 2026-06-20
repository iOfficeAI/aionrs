// Core agent infrastructure: engine, session, orchestration, output sinks.

pub mod agents_md;
pub mod bootstrap;
pub mod cache_diagnostics;
pub mod commands;
pub mod compact;
pub mod confirm;
pub mod context;
pub mod engine;
pub mod error;
pub mod orchestration;
pub mod output;
pub mod plan;
pub mod public_error;
pub mod session;
pub mod skill_tool;
pub mod spawn_tool;
pub mod spawner;
mod tool_call;
pub mod vcr;

// Re-export the skills crate so existing callers (aion-cli, tests) can use
// `aion_agent::skills::` without changing their import paths.
pub use aion_skills as skills;
