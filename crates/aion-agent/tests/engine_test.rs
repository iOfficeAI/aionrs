mod common;

use std::sync::{Arc, Mutex};

use aion_agent::engine::AgentEngine;
use aion_agent::error::AgentError;
use aion_agent::output::OutputSink;
use aion_agent::output::terminal::TerminalSink;
use aion_agent::session::SessionManager;
use aion_providers::{LlmProvider, ProviderError};
use aion_tools::registry::ToolRegistry;
use aion_types::llm::{LlmEvent, LlmRequest};
use aion_types::message::{ContentBlock, Message, Role, StopReason, TokenUsage};
use async_trait::async_trait;
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio::sync::mpsc;

use common::{MockLlmProvider, MockTool, test_config};

// ---------------------------------------------------------------------------
// Helper: build a no-color OutputFormatter for silent test output
// ---------------------------------------------------------------------------
fn silent_output() -> Arc<dyn OutputSink> {
    Arc::new(TerminalSink::new(true))
}

fn tool_call_malformed_turn(id: &str, name: &str, input: Value) -> Vec<LlmEvent> {
    vec![
        LlmEvent::ToolUse {
            id: id.to_string(),
            name: name.to_string(),
            input,
            extra: None,
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
}

fn tool_call_failure_turn(id: &str) -> Vec<LlmEvent> {
    vec![
        LlmEvent::ToolUse {
            id: id.to_string(),
            name: "mock_tool".to_string(),
            input: json!({}),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            usage: TokenUsage::default(),
        },
    ]
}

#[derive(Default)]
struct RecordingOutputSink {
    tool_calls: Mutex<Vec<(String, String)>>,
    tool_results: Mutex<Vec<(String, String, bool)>>,
}

impl OutputSink for RecordingOutputSink {
    fn emit_text_delta(&self, _text: &str, _msg_id: &str) {}
    fn emit_thinking(&self, _text: &str, _msg_id: &str) {}

    fn emit_tool_call(&self, tool_use_id: &str, name: &str, _input: &str) {
        self.tool_calls
            .lock()
            .unwrap()
            .push((tool_use_id.to_owned(), name.to_owned()));
    }

    fn emit_tool_result(&self, tool_use_id: &str, name: &str, is_error: bool, _content: &str) {
        self.tool_results
            .lock()
            .unwrap()
            .push((tool_use_id.to_owned(), name.to_owned(), is_error));
    }

    fn emit_stream_start(&self, _msg_id: &str) {}
    fn emit_stream_end(
        &self,
        _msg_id: &str,
        _turns: usize,
        _input_tokens: u64,
        _output_tokens: u64,
        _cache_creation_tokens: u64,
        _cache_read_tokens: u64,
    ) {
    }
    fn emit_error(&self, _msg: &str) {}
    fn emit_info(&self, _msg: &str) {}
}

struct RecordingRequestProvider {
    requests: Arc<Mutex<Vec<Vec<Message>>>>,
    responses: Mutex<Vec<Vec<LlmEvent>>>,
}

impl RecordingRequestProvider {
    fn new(responses: Vec<Vec<LlmEvent>>) -> Self {
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            responses: Mutex::new(responses),
        }
    }

    fn requests(&self) -> Arc<Mutex<Vec<Vec<Message>>>> {
        Arc::clone(&self.requests)
    }
}

#[async_trait]
impl LlmProvider for RecordingRequestProvider {
    async fn stream(&self, request: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.requests.lock().unwrap().push(request.messages.clone());
        let events = self.responses.lock().unwrap().remove(0);
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            for event in events {
                let _ = tx.send(event).await;
            }
        });
        Ok(rx)
    }
}

#[derive(Debug, Clone)]
struct RecordedRequest {
    messages: Vec<Message>,
    tool_count: usize,
}

struct FullRecordingRequestProvider {
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    responses: Mutex<Vec<Vec<LlmEvent>>>,
}

impl FullRecordingRequestProvider {
    fn new(responses: Vec<Vec<LlmEvent>>) -> Self {
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            responses: Mutex::new(responses),
        }
    }

    fn requests(&self) -> Arc<Mutex<Vec<RecordedRequest>>> {
        Arc::clone(&self.requests)
    }
}

#[async_trait]
impl LlmProvider for FullRecordingRequestProvider {
    async fn stream(&self, request: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.requests.lock().unwrap().push(RecordedRequest {
            messages: request.messages.clone(),
            tool_count: request.tools.len(),
        });
        let events = self.responses.lock().unwrap().remove(0);
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            for event in events {
                let _ = tx.send(event).await;
            }
        });
        Ok(rx)
    }
}

fn contains_empty_assistant_message(messages: &[Message]) -> bool {
    messages
        .iter()
        .any(|message| message.role == Role::Assistant && message.content.is_empty())
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

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output, std::env::temp_dir());
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
            extra: None,
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

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output, std::env::temp_dir());
    let result = engine.run("Use the tool", "").await.expect("engine should succeed");

    assert_eq!(result.turns, 2);
    assert_eq!(result.text, "Done");
}

#[tokio::test]
async fn test_engine_round_trips_thinking_signature_into_tool_followup_request() {
    let provider = Arc::new(RecordingRequestProvider::new(vec![
        vec![
            LlmEvent::ThinkingDelta("need a tool".to_string()),
            LlmEvent::ThinkingSignature("sig-123".to_string()),
            LlmEvent::ToolUse {
                id: "call_1".to_string(),
                name: "mock_tool".to_string(),
                input: json!({}),
                extra: None,
            },
            LlmEvent::Done {
                stop_reason: StopReason::ToolUse,
                usage: TokenUsage::default(),
            },
        ],
        vec![
            LlmEvent::TextDelta("done".to_string()),
            LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                usage: TokenUsage::default(),
            },
        ],
    ]));
    let requests = provider.requests();

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("mock_tool", "tool result", false)));

    let mut engine =
        AgentEngine::new_with_provider(provider, test_config(), registry, silent_output(), std::env::temp_dir());

    let result = engine.run("use tool", "").await.expect("engine should succeed");

    assert_eq!(result.text, "done");
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);

    let followup_messages = &requests[1];
    let assistant_message = followup_messages
        .iter()
        .find(|message| message.role == Role::Assistant)
        .expect("assistant message should be present");

    match &assistant_message.content[0] {
        ContentBlock::Thinking { thinking, signature } => {
            assert_eq!(thinking, "need a tool");
            assert_eq!(signature.as_deref(), Some("sig-123"));
        }
        other => panic!("expected thinking block, got {other:?}"),
    }
}

#[tokio::test]
async fn duplicate_tool_names_emit_distinct_tool_use_ids() {
    let turn1 = vec![
        LlmEvent::ToolUse {
            id: "call_a".to_string(),
            name: "Glob".to_string(),
            input: json!({"pattern": "*.rs"}),
            extra: None,
        },
        LlmEvent::ToolUse {
            id: "call_b".to_string(),
            name: "Glob".to_string(),
            input: json!({"pattern": "*.toml"}),
            extra: None,
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
    registry.register(Box::new(MockTool::new("Glob", "tool output", false)));
    let output = Arc::new(RecordingOutputSink::default());

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output.clone(), std::env::temp_dir());
    let result = engine.run("Use Glob twice", "").await.expect("engine should succeed");

    assert_eq!(result.text, "Done");
    assert_eq!(
        *output.tool_calls.lock().unwrap(),
        vec![
            ("call_a".to_string(), "Glob".to_string()),
            ("call_b".to_string(), "Glob".to_string()),
        ]
    );
    assert_eq!(
        *output.tool_results.lock().unwrap(),
        vec![
            ("call_a".to_string(), "Glob".to_string(), false),
            ("call_b".to_string(), "Glob".to_string(), false),
        ]
    );
}

// ---------------------------------------------------------------------------
// test_engine_max_tokens_handling
//
// Verifies that a MaxTokens stop reason triggers one tool-disabled
// continuation request and accumulates usage from both model calls.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_engine_max_tokens_handling() {
    let provider = Arc::new(FullRecordingRequestProvider::new(vec![
        vec![
            LlmEvent::TextDelta("partial ".to_string()),
            LlmEvent::Done {
                stop_reason: StopReason::MaxTokens,
                usage: TokenUsage {
                    input_tokens: 200,
                    output_tokens: 100,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                },
            },
        ],
        vec![
            LlmEvent::TextDelta("finished".to_string()),
            LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                usage: TokenUsage {
                    input_tokens: 20,
                    output_tokens: 10,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                },
            },
        ],
    ]));
    let requests = provider.requests();

    let config = test_config();
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("mock_tool", "tool output", false)));
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output, std::env::temp_dir());
    let result = engine
        .run("Give me a long answer", "")
        .await
        .expect("engine should succeed");

    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert_eq!(result.text, "partial finished");
    assert_eq!(result.turns, 1);
    assert_eq!(result.usage.input_tokens, 220);
    assert_eq!(result.usage.output_tokens, 110);

    let recorded = requests.lock().unwrap();
    assert_eq!(recorded.len(), 2);
    assert_eq!(recorded[1].tool_count, 0);
    let last_message = recorded[1]
        .messages
        .last()
        .expect("finalization request should include control prompt");
    assert_eq!(last_message.role, Role::User);
    assert!(
        matches!(
            &last_message.content[..],
            [ContentBlock::Text { text }] if text.contains("previous response was cut off")
        ),
        "finalization prompt should explain the max tokens continuation"
    );
}

#[tokio::test]
async fn empty_final_gets_one_visible_answer_nudge() {
    let provider = Arc::new(FullRecordingRequestProvider::new(vec![
        vec![LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            usage: TokenUsage::default(),
        }],
        vec![
            LlmEvent::TextDelta("Visible answer".to_string()),
            LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                usage: TokenUsage::default(),
            },
        ],
    ]));
    let requests = provider.requests();

    let config = test_config();
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("mock_tool", "tool output", false)));
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output, std::env::temp_dir());
    let result = engine.run("Answer visibly", "").await.expect("engine should succeed");

    assert_eq!(result.text, "Visible answer");
    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert_eq!(result.turns, 1);

    let recorded = requests.lock().unwrap();
    assert_eq!(recorded.len(), 2);
    assert_eq!(recorded[1].tool_count, 0);
    assert!(
        !contains_empty_assistant_message(&recorded[1].messages),
        "empty finalization request must not include empty assistant content"
    );
    let last_message = recorded[1]
        .messages
        .last()
        .expect("finalization request should include control prompt");
    assert_eq!(last_message.role, Role::User);
    assert!(
        matches!(
            &last_message.content[..],
            [ContentBlock::Text { text }] if text.contains("visible answer text")
        ),
        "empty finalization prompt should ask for visible answer text"
    );
}

#[tokio::test]
async fn empty_final_falls_back_after_one_empty_retry() {
    let dir = tempdir().expect("tempdir should be created");
    let provider = Arc::new(FullRecordingRequestProvider::new(vec![
        vec![LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            usage: TokenUsage::default(),
        }],
        vec![LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            usage: TokenUsage::default(),
        }],
    ]));

    let mut config = test_config();
    config.session.enabled = true;
    config.session.directory = dir.path().to_string_lossy().into_owned();
    let registry = ToolRegistry::new();
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output, std::env::temp_dir());
    engine
        .init_session("test-provider", "/tmp", None)
        .expect("init_session should succeed");
    let result = engine
        .run("Answer visibly", "")
        .await
        .expect("engine should fall back successfully");

    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert_eq!(result.turns, 1);
    assert!(result.text.contains("finished without visible answer text"));

    let session = SessionManager::new(dir.path().to_path_buf(), 10)
        .load("latest")
        .expect("session should be loadable");
    assert!(
        !contains_empty_assistant_message(&session.messages),
        "empty finalization fallback must not persist empty assistant content"
    );

    let last_message = session
        .messages
        .last()
        .expect("fallback assistant message should be persisted");
    assert_eq!(last_message.role, Role::Assistant);
    assert!(
        matches!(
            &last_message.content[..],
            [ContentBlock::Text { text }] if text == &result.text
        ),
        "fallback assistant message should be text-only"
    );
}

#[tokio::test]
async fn max_tokens_continuation_does_not_increment_reported_turns() {
    let provider = Arc::new(MockLlmProvider::with_turns(vec![
        vec![
            LlmEvent::TextDelta("partial ".to_string()),
            LlmEvent::Done {
                stop_reason: StopReason::MaxTokens,
                usage: TokenUsage::default(),
            },
        ],
        vec![
            LlmEvent::TextDelta("finished".to_string()),
            LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                usage: TokenUsage::default(),
            },
        ],
    ]));
    let mut config = test_config();
    config.max_turns = Some(1);
    let registry = ToolRegistry::new();
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output, std::env::temp_dir());
    let result = engine
        .run("Give me a long answer", "")
        .await
        .expect("engine should succeed");

    assert_eq!(result.text, "partial finished");
    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert_eq!(result.turns, 1);
}

#[tokio::test]
async fn max_tokens_finalization_tool_call_falls_back_without_persisting_tool_use() {
    let dir = tempdir().expect("tempdir should be created");
    let provider = Arc::new(FullRecordingRequestProvider::new(vec![
        vec![
            LlmEvent::TextDelta("partial ".to_string()),
            LlmEvent::Done {
                stop_reason: StopReason::MaxTokens,
                usage: TokenUsage::default(),
            },
        ],
        vec![
            LlmEvent::ToolUse {
                id: "bad-tool".to_string(),
                name: "mock_tool".to_string(),
                input: json!({}),
                extra: None,
            },
            LlmEvent::Done {
                stop_reason: StopReason::ToolUse,
                usage: TokenUsage::default(),
            },
        ],
    ]));
    let requests = provider.requests();

    let mut config = test_config();
    config.session.enabled = true;
    config.session.directory = dir.path().to_string_lossy().into_owned();

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("mock_tool", "tool output", false)));

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, silent_output(), std::env::temp_dir());
    engine
        .init_session("test-provider", "/tmp", None)
        .expect("init_session should succeed");

    let result = engine
        .run("Give me a long answer", "")
        .await
        .expect("engine should fall back successfully");

    assert_eq!(result.stop_reason, StopReason::MaxTokens);
    assert_eq!(result.turns, 1);
    assert!(
        !result.text.trim().is_empty(),
        "fallback result text should be non-empty"
    );

    let recorded = requests.lock().unwrap();
    assert_eq!(recorded.len(), 2);
    assert!(
        recorded[0].tool_count > 0,
        "normal request should include registered tools"
    );
    assert_eq!(recorded[1].tool_count, 0);
    drop(recorded);

    let session = SessionManager::new(dir.path().to_path_buf(), 10)
        .load("latest")
        .expect("session should be loadable");
    assert!(
        !session.messages.iter().any(|message| {
            message.role == Role::Assistant
                && message
                    .content
                    .iter()
                    .any(|block| matches!(block, ContentBlock::ToolUse { id, .. } if id == "bad-tool"))
        }),
        "invalid finalization tool call must not be persisted"
    );
    assert!(
        !session
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .any(|block| matches!(
                block,
                ContentBlock::Text { text } if text.contains("previous response was cut off")
            )),
        "temporary finalization control prompt must not be persisted"
    );

    let last_message = session
        .messages
        .last()
        .expect("fallback assistant message should be persisted");
    assert_eq!(last_message.role, Role::Assistant);
    assert!(
        matches!(
            &last_message.content[..],
            [ContentBlock::Text { text }] if text == &result.text
        ),
        "fallback assistant message should be text-only"
    );
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

    let mut engine = AgentEngine::new_with_provider(provider, config.clone(), registry, output, std::env::temp_dir());

    // Initialize session so save_session() has a session to persist
    engine
        .init_session("test-provider", "/tmp", None)
        .expect("init_session should succeed");

    engine.run("First message", "").await.expect("first run should succeed");
    engine
        .run("Second message", "")
        .await
        .expect("second run should succeed");

    // Load the persisted session and count accumulated messages
    let session_manager = SessionManager::new(dir.path().to_path_buf(), 10);
    let session = session_manager.load("latest").expect("session should be loadable");

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
            extra: None,
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

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output, std::env::temp_dir());
    let result = engine.run("Do work", "").await.expect("engine should succeed");

    assert_eq!(
        result.usage.input_tokens, 180,
        "input tokens should accumulate across turns"
    );
    assert_eq!(
        result.usage.output_tokens, 80,
        "output tokens should accumulate across turns"
    );
}

// ---------------------------------------------------------------------------
// test_engine_max_turns_runs_one_grace_finalization
//
// Verifies that exhausting max_turns after a tool round gets one tool-disabled
// finalization request before falling back to StopReason::MaxTurns.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_engine_max_turns_runs_one_grace_finalization() {
    let provider = Arc::new(FullRecordingRequestProvider::new(vec![
        vec![
            LlmEvent::ToolUse {
                id: "tool-1".to_string(),
                name: "mock_tool".to_string(),
                input: json!({}),
                extra: None,
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
        ],
        vec![
            LlmEvent::TextDelta("Final from existing tool result".to_string()),
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
    let requests = provider.requests();

    let mut config = test_config();
    config.max_turns = Some(1);

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("mock_tool", "result", false)));
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output, std::env::temp_dir());
    let result = engine
        .run("Keep calling tools", "")
        .await
        .expect("engine should succeed with grace finalization");

    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert_eq!(result.text, "Final from existing tool result");
    assert_eq!(result.turns, 1);

    let recorded = requests.lock().unwrap();
    assert_eq!(recorded.len(), 2);
    assert!(
        recorded[0].tool_count > 0,
        "normal request should include registered tools"
    );
    assert_eq!(recorded[1].tool_count, 0);
    let last_message = recorded[1]
        .messages
        .last()
        .expect("grace finalization request should include control prompt");
    assert_eq!(last_message.role, Role::User);
    assert!(
        matches!(
            &last_message.content[..],
            [ContentBlock::Text { text }] if text.contains("Do not call any more tools")
        ),
        "grace finalization prompt should forbid more tool calls"
    );
}

#[tokio::test]
async fn finalization_requests_can_be_asserted_without_tools() {
    let provider = Arc::new(FullRecordingRequestProvider::new(vec![vec![
        LlmEvent::TextDelta("done".to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            usage: TokenUsage::default(),
        },
    ]]));
    let requests = provider.requests();

    let mut engine = AgentEngine::new_with_provider(
        provider,
        test_config(),
        ToolRegistry::new(),
        silent_output(),
        std::env::temp_dir(),
    );
    let result = engine
        .run("Say done", "")
        .await
        .expect("engine should return the final text");

    assert_eq!(result.text, "done");
    let recorded = requests.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].tool_count, 0);
    assert_eq!(recorded[0].messages.len(), 1);
}

#[tokio::test]
async fn repeated_tool_call_failure_turns_finalize_without_more_tools() {
    let provider = Arc::new(FullRecordingRequestProvider::new(vec![
        tool_call_failure_turn("tool-1"),
        tool_call_failure_turn("tool-2"),
        tool_call_failure_turn("tool-3"),
        vec![
            LlmEvent::TextDelta("The tool is blocked by permission settings.".to_string()),
            LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                usage: TokenUsage::default(),
            },
        ],
    ]));
    let requests = provider.requests();

    let mut config = test_config();
    config.max_turns = Some(10);

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("mock_tool", "permission denied", true)));

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, silent_output(), std::env::temp_dir());
    let result = engine
        .run("keep retrying a failing tool", "")
        .await
        .expect("engine should finalize repeated tool-call-failure loops");

    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert_eq!(result.text, "The tool is blocked by permission settings.");
    assert_eq!(result.turns, 3);

    let recorded = requests.lock().unwrap();
    assert_eq!(recorded.len(), 4);
    assert!(recorded[..3].iter().all(|request| request.tool_count > 0));
    assert_eq!(recorded[3].tool_count, 0);
    assert!(recorded[3].messages.iter().any(|message| {
        message.content.iter().any(|block| {
            matches!(
                block,
                ContentBlock::ToolResult { content, is_error: true, .. }
                    if content.contains("permission denied")
            )
        })
    }));
}

#[tokio::test]
async fn repeated_tool_call_failure_threshold_one_finalizes_immediately() {
    let provider = Arc::new(FullRecordingRequestProvider::new(vec![
        tool_call_failure_turn("tool-1"),
        vec![
            LlmEvent::TextDelta("The tool failed because permission was denied.".to_string()),
            LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                usage: TokenUsage::default(),
            },
        ],
    ]));
    let requests = provider.requests();

    let mut config = test_config();
    config.max_turns = Some(10);
    config.max_tool_call_failure_turns = Some(1);

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("mock_tool", "permission denied", true)));

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, silent_output(), std::env::temp_dir());
    let result = engine
        .run("keep retrying a failing tool", "")
        .await
        .expect("engine should finalize repeated tool-call-failure loops");

    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert_eq!(result.text, "The tool failed because permission was denied.");
    assert_eq!(result.turns, 1);

    let recorded = requests.lock().unwrap();
    assert_eq!(recorded.len(), 2);
    assert!(recorded[0].tool_count > 0);
    assert_eq!(recorded[1].tool_count, 0);
}

#[tokio::test]
async fn repeated_tool_call_failure_disabled_runs_grace_finalization() {
    let provider = Arc::new(FullRecordingRequestProvider::new(vec![
        tool_call_failure_turn("tool-1"),
        tool_call_failure_turn("tool-2"),
        vec![
            LlmEvent::TextDelta("Final after tool-call failures".to_string()),
            LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                usage: TokenUsage::default(),
            },
        ],
    ]));
    let requests = provider.requests();

    let mut config = test_config();
    config.max_turns = Some(2);
    config.max_tool_call_failure_turns = Some(0);

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("mock_tool", "permission denied", true)));

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, silent_output(), std::env::temp_dir());
    let result = engine
        .run("keep retrying a failing tool", "")
        .await
        .expect("engine should stop cleanly");

    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert_eq!(result.text, "Final after tool-call failures");
    assert_eq!(result.turns, 2);

    let recorded = requests.lock().unwrap();
    assert_eq!(recorded.len(), 3);
    assert!(
        recorded[0].tool_count > 0,
        "normal requests should include registered tools"
    );
    assert_eq!(recorded[2].tool_count, 0);
    let last_message = recorded[2]
        .messages
        .last()
        .expect("grace finalization request should include control prompt");
    assert_eq!(last_message.role, Role::User);
    assert!(
        matches!(
            &last_message.content[..],
            [ContentBlock::Text { text }] if text.contains("Do not call any more tools")
        ),
        "grace finalization prompt should forbid more tool calls"
    );
}

#[tokio::test]
async fn repeated_tool_call_malformed_stops_on_default_third_turn() {
    let dir = tempdir().expect("tempdir should be created");
    let provider = Arc::new(RecordingRequestProvider::new(vec![
        tool_call_malformed_turn("bad", "", json!({})),
        tool_call_malformed_turn("bad", "", json!({})),
        tool_call_malformed_turn("bad", "", json!({})),
        vec![
            LlmEvent::TextDelta("should not be requested".to_string()),
            LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                usage: TokenUsage::default(),
            },
        ],
    ]));
    let requests = provider.requests();

    let mut config = test_config();
    config.max_tool_call_malformed_turns = None;
    config.session.enabled = true;
    config.session.directory = dir.path().to_string_lossy().into_owned();

    let mut engine = AgentEngine::new_with_provider(
        provider,
        config,
        ToolRegistry::new(),
        silent_output(),
        std::env::temp_dir(),
    );
    engine
        .init_session("test-provider", "/tmp", None)
        .expect("init_session should succeed");

    let err = engine
        .run("repeat malformed", "")
        .await
        .expect_err("engine should surface repeated tool-call-malformed loop");

    assert!(matches!(err, AgentError::ToolCallMalformed { count: 3, limit: 3 }));
    assert_eq!(
        requests.lock().unwrap().len(),
        3,
        "fourth provider request must not be sent"
    );

    let session = SessionManager::new(dir.path().to_path_buf(), 10)
        .load("latest")
        .expect("session should be loadable");
    let tool_uses = session
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter(|block| matches!(block, ContentBlock::ToolUse { .. }))
        .count();
    let tool_results: Vec<_> = session
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| {
            let ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } = block
            else {
                return None;
            };
            Some((tool_use_id, content, is_error))
        })
        .collect();

    assert_eq!(tool_uses, 3);
    assert_eq!(tool_results.len(), 3);
    assert!(
        tool_results.iter().all(|(id, content, is_error)| {
            id.as_str() == "bad" && **is_error && content.contains("Malformed tool call: empty function name")
        }),
        "tool-call malformed uses should have paired synthetic error results"
    );
}

#[tokio::test]
async fn repeated_tool_call_malformed_threshold_one_stops_immediately() {
    let provider = Arc::new(RecordingRequestProvider::new(vec![
        tool_call_malformed_turn("bad", "", json!({})),
        tool_call_malformed_turn("bad", "", json!({})),
    ]));
    let requests = provider.requests();

    let mut config = test_config();
    config.max_tool_call_malformed_turns = Some(1);

    let mut engine = AgentEngine::new_with_provider(
        provider,
        config,
        ToolRegistry::new(),
        silent_output(),
        std::env::temp_dir(),
    );
    let err = engine
        .run("repeat malformed", "")
        .await
        .expect_err("engine should surface repeated tool-call-malformed loop");

    assert!(matches!(err, AgentError::ToolCallMalformed { count: 1, limit: 1 }));
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn repeated_tool_call_malformed_disabled_runs_grace_finalization() {
    let provider = Arc::new(FullRecordingRequestProvider::new(vec![
        tool_call_malformed_turn("bad", "", json!({})),
        tool_call_malformed_turn("bad", "", json!({})),
        vec![
            LlmEvent::TextDelta("Final after malformed attempts".to_string()),
            LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                usage: TokenUsage::default(),
            },
        ],
    ]));
    let requests = provider.requests();

    let mut config = test_config();
    config.max_tool_call_malformed_turns = Some(0);
    config.max_turns = Some(2);

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("mock_tool", "unused", false)));

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, silent_output(), std::env::temp_dir());
    let result = engine
        .run("repeat malformed", "")
        .await
        .expect("engine should stop cleanly");

    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert_eq!(result.text, "Final after malformed attempts");
    assert_eq!(result.turns, 2);

    let recorded = requests.lock().unwrap();
    assert_eq!(recorded.len(), 3);
    assert!(
        recorded[0].tool_count > 0,
        "normal requests should include registered tools"
    );
    assert_eq!(recorded[2].tool_count, 0);
    let last_message = recorded[2]
        .messages
        .last()
        .expect("grace finalization request should include control prompt");
    assert_eq!(last_message.role, Role::User);
    assert!(
        matches!(
            &last_message.content[..],
            [ContentBlock::Text { text }] if text.contains("Do not call any more tools")
        ),
        "grace finalization prompt should forbid more tool calls"
    );
}

#[tokio::test]
async fn mixed_valid_and_tool_call_malformed_calls_do_not_trip_breaker() {
    let mixed_turn = || {
        vec![
            LlmEvent::ToolUse {
                id: "bad".to_string(),
                name: "".to_string(),
                input: json!({}),
                extra: None,
            },
            LlmEvent::ToolUse {
                id: "ok".to_string(),
                name: "mock_tool".to_string(),
                input: json!({}),
                extra: None,
            },
            LlmEvent::Done {
                stop_reason: StopReason::ToolUse,
                usage: TokenUsage::default(),
            },
        ]
    };
    let provider = Arc::new(RecordingRequestProvider::new(vec![
        mixed_turn(),
        mixed_turn(),
        vec![
            LlmEvent::TextDelta("done".to_string()),
            LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                usage: TokenUsage::default(),
            },
        ],
    ]));
    let requests = provider.requests();

    let mut config = test_config();
    config.max_tool_call_malformed_turns = Some(1);
    let output = Arc::new(RecordingOutputSink::default());
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("mock_tool", "tool output", false)));

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output.clone(), std::env::temp_dir());
    let result = engine
        .run("mixed tool calls", "")
        .await
        .expect("engine should reach final text");

    assert_eq!(result.text, "done");
    assert_eq!(result.turns, 3);
    assert_eq!(requests.lock().unwrap().len(), 3);
    assert_eq!(
        *output.tool_results.lock().unwrap(),
        vec![
            ("bad".to_string(), "".to_string(), true),
            ("ok".to_string(), "mock_tool".to_string(), false),
            ("bad".to_string(), "".to_string(), true),
            ("ok".to_string(), "mock_tool".to_string(), false),
        ]
    );
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

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output, std::env::temp_dir());
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
