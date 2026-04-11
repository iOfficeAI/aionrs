//! Integration tests for Spawn tool description (TC-4.2-07).
//!
//! Verifies the enhanced Spawn tool description contains capacity limits
//! and usage guidance as specified in the test plan.

mod common;

use std::sync::Arc;

use aion_agent::spawn_tool::SpawnTool;
use aion_agent::spawner::AgentSpawner;
use aion_tools::Tool;
use common::{MockLlmProvider, test_config};

fn make_spawn_tool() -> SpawnTool {
    let provider = Arc::new(MockLlmProvider::with_text_response("ok"));
    let spawner = Arc::new(AgentSpawner::new(provider, test_config()));
    SpawnTool::new(spawner)
}

// --- TC-4.2-07: Spawn tool description contains capacity limits ---

#[test]
fn spawn_description_mentions_max_agents() {
    let tool = make_spawn_tool();
    let desc = tool.description();
    assert!(
        desc.contains('5'),
        "Spawn description should mention the 5 sub-agent limit"
    );
}

#[test]
fn spawn_description_mentions_parallel() {
    let tool = make_spawn_tool();
    let desc = tool.description();
    assert!(
        desc.contains("parallel"),
        "Spawn description should mention parallel execution"
    );
}

// --- R-4.1-01: Spawn description should document max_turns and max_tokens ---

#[test]
fn spawn_description_mentions_max_turns() {
    let tool = make_spawn_tool();
    let desc = tool.description();
    assert!(
        desc.contains("10"),
        "Spawn description should mention the 10 turn limit per sub-agent"
    );
}

#[test]
fn spawn_description_mentions_max_tokens() {
    let tool = make_spawn_tool();
    let desc = tool.description();
    assert!(
        desc.contains("4096"),
        "Spawn description should mention the 4096 token limit per sub-agent"
    );
}
