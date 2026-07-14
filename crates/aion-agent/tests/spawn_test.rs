mod common;

use std::sync::{Arc, Mutex};

use aion_agent::spawner::{AgentSpawner, SubAgentConfig};
use aion_agent::tool_policy::ToolPolicy;
use aion_providers::{LlmProvider, ProviderError};
use aion_types::llm::{LlmEvent, LlmRequest};
use aion_types::message::{StopReason, TokenUsage};
use async_trait::async_trait;
use common::{MockLlmProvider, test_config};
use tokio::sync::mpsc;

struct ToolRecordingProvider {
    tool_names: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl LlmProvider for ToolRecordingProvider {
    async fn stream(&self, request: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let mut tool_names: Vec<_> = request.tools.iter().map(|tool| tool.name.clone()).collect();
        tool_names.sort();
        *self.tool_names.lock().unwrap() = tool_names;

        let (tx, rx) = mpsc::channel(2);
        tx.try_send(LlmEvent::TextDelta("done".to_string())).unwrap();
        tx.try_send(LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            usage: TokenUsage::default(),
        })
        .unwrap();
        Ok(rx)
    }
}

// ---------------------------------------------------------------------------
// Helper: build a minimal SubAgentConfig for testing
// ---------------------------------------------------------------------------

fn make_sub_config(name: &str) -> SubAgentConfig {
    SubAgentConfig {
        name: name.to_string(),
        prompt: format!("Task for {}", name),
        max_turns: 5,
        max_tokens: 1024,
        system_prompt: None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Single sub-agent executes and returns the expected text result.
#[tokio::test]
async fn test_spawn_single_agent() {
    let provider = Arc::new(MockLlmProvider::with_text_response("Sub-agent done"));
    let spawner = AgentSpawner::new(provider, test_config(), std::env::temp_dir(), ToolPolicy::Unrestricted);

    let result = spawner.spawn_one(make_sub_config("agent-1")).await;

    assert_eq!(result.text, "Sub-agent done");
    assert!(!result.is_error, "expected no error, got: {}", result.text);
    assert_eq!(result.turns, 1);
    assert_eq!(result.name, "agent-1");
}

#[tokio::test]
async fn restricted_parent_policy_limits_spawned_agent_tools() {
    let tool_names = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(ToolRecordingProvider {
        tool_names: Arc::clone(&tool_names),
    });
    let spawner = AgentSpawner::new(
        provider,
        test_config(),
        std::env::temp_dir(),
        ToolPolicy::allow_only(["Spawn", "Read", "Grep"]),
    );

    let result = spawner.spawn_one(make_sub_config("restricted-agent")).await;

    assert!(!result.is_error, "expected restricted sub-agent to complete");
    assert_eq!(*tool_names.lock().unwrap(), vec!["Grep", "Read"]);
}

/// Parallel sub-agents all complete successfully and return distinct results.
#[tokio::test]
async fn test_spawn_parallel_agents() {
    // Provide one turn sequence per sub-agent; each stream() call pops one entry.
    let make_turn = |text: &str| {
        vec![
            LlmEvent::TextDelta(text.to_string()),
            LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                },
            },
        ]
    };

    let provider = Arc::new(MockLlmProvider::with_turns(vec![
        make_turn("result-A"),
        make_turn("result-B"),
        make_turn("result-C"),
    ]));

    let spawner = AgentSpawner::new(provider, test_config(), std::env::temp_dir(), ToolPolicy::Unrestricted);

    let sub_configs = vec![
        make_sub_config("agent-A"),
        make_sub_config("agent-B"),
        make_sub_config("agent-C"),
    ];

    let results = spawner.spawn_parallel(sub_configs).await;

    assert_eq!(results.len(), 3, "expected 3 results from 3 sub-agents");

    for result in &results {
        assert!(
            !result.is_error,
            "sub-agent '{}' returned an error: {}",
            result.name, result.text
        );
    }

    // Each result should contain one of the expected texts (order may vary due
    // to concurrent scheduling, so we just verify the full set is covered).
    let texts: std::collections::HashSet<&str> = results.iter().map(|r| r.text.as_str()).collect();
    assert!(texts.contains("result-A"), "missing result-A");
    assert!(texts.contains("result-B"), "missing result-B");
    assert!(texts.contains("result-C"), "missing result-C");
}

/// The same provider Arc is reused across sequentially spawned sub-agents.
#[tokio::test]
async fn test_spawn_shares_provider() {
    // Two turns: one for each sequential sub-agent call.
    let provider = Arc::new(MockLlmProvider::with_turns(vec![
        vec![
            LlmEvent::TextDelta("first".to_string()),
            LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                },
            },
        ],
        vec![
            LlmEvent::TextDelta("second".to_string()),
            LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                },
            },
        ],
    ]));

    // Both sub-agents share the same underlying provider via Arc.
    let provider_dyn: Arc<dyn aion_providers::LlmProvider> = provider;
    let spawner = AgentSpawner::new(
        Arc::clone(&provider_dyn),
        test_config(),
        std::env::temp_dir(),
        ToolPolicy::Unrestricted,
    );

    let result1 = spawner.spawn_one(make_sub_config("seq-1")).await;
    let result2 = spawner.spawn_one(make_sub_config("seq-2")).await;

    assert!(!result1.is_error, "seq-1 errored: {}", result1.text);
    assert!(!result2.is_error, "seq-2 errored: {}", result2.text);
    assert_eq!(result1.text, "first");
    assert_eq!(result2.text, "second");
}

/// An LLM error event causes the sub-agent result to be marked as an error.
#[tokio::test]
async fn test_spawn_agent_error_captured() {
    // Emit an Error event — the engine converts this to AgentError::ApiError,
    // which spawner catches and stores in SubAgentResult::is_error.
    let provider = Arc::new(MockLlmProvider::with_events(vec![LlmEvent::Error(
        "provider failed".to_string(),
    )]));

    let spawner = AgentSpawner::new(provider, test_config(), std::env::temp_dir(), ToolPolicy::Unrestricted);

    let result = spawner.spawn_one(make_sub_config("error-agent")).await;

    assert!(result.is_error, "expected is_error=true");
    assert!(
        result.text.to_lowercase().contains("error"),
        "expected error message to contain 'error', got: {}",
        result.text
    );
}
