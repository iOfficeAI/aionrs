mod common;

use std::sync::Arc;

use aionrs::engine::{AgentEngine, AgentError};
use aionrs::output::terminal::TerminalSink;
use aionrs::output::OutputSink;
use aionrs::session::SessionManager;
use aionrs::tools::registry::ToolRegistry;
use aionrs::types::llm::LlmEvent;
use aionrs::types::message::{StopReason, TokenUsage};
use serde_json::json;
use tempfile::tempdir;

use common::{MockLlmProvider, MockTool, test_config};

// ---------------------------------------------------------------------------
// Helper: build a no-color OutputFormatter for silent test output
// ---------------------------------------------------------------------------
fn silent_output() -> Arc<dyn OutputSink> {
    Arc::new(TerminalSink::new(true))
}

// ---------------------------------------------------------------------------
// test_engine_text_response_ends_turn
//
// Verifies that when the LLM returns a pure text response the engine:
//   - captures the full text
//   - reports StopReason::EndTurn
//   - completes in a single turn
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_engine_text_response_ends_turn() {
    let provider = Arc::new(MockLlmProvider::with_text_response("Hello, world!"));
    let config = test_config();
    let registry = ToolRegistry::new();
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    let result = engine.run("Hi", "").await.expect("engine should succeed");

    assert_eq!(result.text, "Hello, world!");
    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert_eq!(result.turns, 1);
}

// ---------------------------------------------------------------------------
// test_engine_tool_use_executes_and_continues
//
// Verifies the agentic loop when the LLM first requests a tool then, after
// receiving the tool result, produces a final text answer.
//   - Turn 1: LLM emits ToolUse for "mock_tool"
//   - Turn 2: LLM emits TextDelta("Done") + EndTurn
//   - result.turns == 2 and result.text == "Done"
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_engine_tool_use_executes_and_continues() {
    let turn1 = vec![
        LlmEvent::ToolUse {
            id: "tool-1".to_string(),
            name: "mock_tool".to_string(),
            input: json!({}),
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            usage: TokenUsage {
                input_tokens: 80,
                output_tokens: 30,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        },
    ];
    let turn2 = vec![
        LlmEvent::TextDelta("Done".to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            usage: TokenUsage {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        },
    ];

    let provider = Arc::new(MockLlmProvider::with_turns(vec![turn1, turn2]));
    let config = test_config();
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("mock_tool", "tool output", false)));
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    let result = engine.run("Use the tool", "").await.expect("engine should succeed");

    assert_eq!(result.turns, 2);
    assert_eq!(result.text, "Done");
}

// ---------------------------------------------------------------------------
// test_engine_max_tokens_handling
//
// Verifies that a MaxTokens stop reason is surfaced correctly when the LLM
// hits its token limit mid-response.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_engine_max_tokens_handling() {
    let events = vec![
        LlmEvent::TextDelta("partial".to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::MaxTokens,
            usage: TokenUsage {
                input_tokens: 200,
                output_tokens: 100,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        },
    ];

    let provider = Arc::new(MockLlmProvider::with_events(events));
    let config = test_config();
    let registry = ToolRegistry::new();
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    let result = engine.run("Give me a long answer", "").await.expect("engine should succeed");

    assert_eq!(result.stop_reason, StopReason::MaxTokens);
    assert_eq!(result.text, "partial");
}

// ---------------------------------------------------------------------------
// test_engine_message_accumulation
//
// Verifies that consecutive calls to `run` accumulate messages across turns.
// Session persistence is used to observe the messages externally since
// engine.messages is private.
//
// After two independent `run` calls the persisted session must contain
// exactly 4 messages: [user, assistant, user, assistant].
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_engine_message_accumulation() {
    let dir = tempdir().expect("tempdir should be created");

    // Provider needs two responses (one per run() call)
    let provider = Arc::new(MockLlmProvider::with_turns(vec![
        vec![
            LlmEvent::TextDelta("Response 1".to_string()),
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
            LlmEvent::TextDelta("Response 2".to_string()),
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

    let mut config = test_config();
    config.session.enabled = true;
    config.session.directory = dir.path().to_string_lossy().into_owned();

    let registry = ToolRegistry::new();
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider, config.clone(), registry, output);

    // Initialize session so save_session() has a session to persist
    engine
        .init_session("test-provider", "/tmp", None)
        .expect("init_session should succeed");

    engine.run("First message", "").await.expect("first run should succeed");
    engine.run("Second message", "").await.expect("second run should succeed");

    // Load the persisted session and count accumulated messages
    let session_manager = SessionManager::new(dir.path().to_path_buf(), 10);
    let session = session_manager
        .load("latest")
        .expect("session should be loadable");

    // Expected layout: user, assistant, user, assistant
    assert_eq!(
        session.messages.len(),
        4,
        "expected 4 messages (user+assistant for each run), got {}",
        session.messages.len()
    );
}

// ---------------------------------------------------------------------------
// test_engine_token_usage_tracking
//
// Verifies that token usage is accumulated correctly across multiple turns.
//   - Turn 1: ToolUse with usage(80 in, 30 out)
//   - Turn 2: EndTurn  with usage(100 in, 50 out)
//   - Expected total: input=180, output=80
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_engine_token_usage_tracking() {
    let turn1 = vec![
        LlmEvent::ToolUse {
            id: "tool-1".to_string(),
            name: "mock_tool".to_string(),
            input: json!({}),
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            usage: TokenUsage {
                input_tokens: 80,
                output_tokens: 30,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        },
    ];
    let turn2 = vec![
        LlmEvent::TextDelta("Final answer".to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            usage: TokenUsage {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        },
    ];

    let provider = Arc::new(MockLlmProvider::with_turns(vec![turn1, turn2]));
    let config = test_config();
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("mock_tool", "result", false)));
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    let result = engine.run("Do work", "").await.expect("engine should succeed");

    assert_eq!(result.usage.input_tokens, 180, "input tokens should accumulate across turns");
    assert_eq!(result.usage.output_tokens, 80, "output tokens should accumulate across turns");
}

// ---------------------------------------------------------------------------
// test_engine_max_turns_returns_ok
//
// Verifies that the engine returns Ok with StopReason::MaxTurns when the
// LLM keeps requesting tools beyond the configured max_turns limit.
//
// With max_turns=1 the engine executes one turn.  If that turn has tool
// calls it processes them, then loops back and hits the limit.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_engine_max_turns_returns_ok() {
    let tool_use_turn = || {
        vec![
            LlmEvent::ToolUse {
                id: "tool-1".to_string(),
                name: "mock_tool".to_string(),
                input: json!({}),
            },
            LlmEvent::Done {
                stop_reason: StopReason::ToolUse,
                usage: TokenUsage {
                    input_tokens: 50,
                    output_tokens: 20,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                },
            },
        ]
    };

    let provider = Arc::new(MockLlmProvider::with_turns(vec![tool_use_turn(), tool_use_turn()]));

    let mut config = test_config();
    config.max_turns = Some(1);

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("mock_tool", "result", false)));
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    let result = engine
        .run("Keep calling tools", "")
        .await
        .expect("should return Ok, not Err");

    assert_eq!(result.stop_reason, StopReason::MaxTurns);
    assert_eq!(result.turns, 1);
}

// ---------------------------------------------------------------------------
// test_engine_api_error_handling
//
// Verifies that an LlmEvent::Error propagates as AgentError::ApiError with
// the original error message intact.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_engine_api_error_handling() {
    let events = vec![LlmEvent::Error("test error".to_string())];

    let provider = Arc::new(MockLlmProvider::with_events(events));
    let config = test_config();
    let registry = ToolRegistry::new();
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    let err = engine
        .run("Hello", "")
        .await
        .map(|_| panic!("expected error, got Ok"))
        .unwrap_err();

    match err {
        AgentError::ApiError(msg) => assert_eq!(msg, "test error"),
        other => panic!("expected ApiError(\"test error\"), got: {:?}", other),
    }
}
