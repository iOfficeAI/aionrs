mod common;

use std::sync::{Arc, Mutex};

use aion_agent::engine::AgentEngine;
use aion_agent::error::AgentError;
use aion_agent::output::OutputSink;
use aion_agent::output::terminal::TerminalSink;
use aion_agent::session::SessionManager;
use aion_providers::{
    LlmProvider, ProviderFailure, ProviderFailurePhase, ProviderRawSignalSource,
    ProviderRawSignals, ProviderStreamReceiver, classify_provider_failure,
};
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

fn malformed_tool_turn(id: &str, name: &str, input: Value) -> Vec<LlmEvent> {
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
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<ProviderStreamReceiver, ProviderFailure> {
        self.requests.lock().unwrap().push(request.messages.clone());
        let events = self.responses.lock().unwrap().remove(0);
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            for event in events {
                let _ = tx.send(Ok(event)).await;
            }
        });
        Ok(rx)
    }
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

    let mut engine =
        AgentEngine::new_with_provider(provider, config, registry, output, std::env::temp_dir());
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

    let mut engine =
        AgentEngine::new_with_provider(provider, config, registry, output, std::env::temp_dir());
    let result = engine
        .run("Use the tool", "")
        .await
        .expect("engine should succeed");

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

    let mut engine = AgentEngine::new_with_provider(
        provider,
        test_config(),
        registry,
        silent_output(),
        std::env::temp_dir(),
    );

    let result = engine
        .run("use tool", "")
        .await
        .expect("engine should succeed");

    assert_eq!(result.text, "done");
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);

    let followup_messages = &requests[1];
    let assistant_message = followup_messages
        .iter()
        .find(|message| message.role == Role::Assistant)
        .expect("assistant message should be present");

    match &assistant_message.content[0] {
        ContentBlock::Thinking {
            thinking,
            signature,
        } => {
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

    let mut engine = AgentEngine::new_with_provider(
        provider,
        config,
        registry,
        output.clone(),
        std::env::temp_dir(),
    );
    let result = engine
        .run("Use Glob twice", "")
        .await
        .expect("engine should succeed");

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

    let mut engine =
        AgentEngine::new_with_provider(provider, config, registry, output, std::env::temp_dir());
    let result = engine
        .run("Give me a long answer", "")
        .await
        .expect("engine should succeed");

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

    let mut engine = AgentEngine::new_with_provider(
        provider,
        config.clone(),
        registry,
        output,
        std::env::temp_dir(),
    );

    // Initialize session so save_session() has a session to persist
    engine
        .init_session("test-provider", "/tmp", None)
        .expect("init_session should succeed");

    engine
        .run("First message", "")
        .await
        .expect("first run should succeed");
    engine
        .run("Second message", "")
        .await
        .expect("second run should succeed");

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

    let mut engine =
        AgentEngine::new_with_provider(provider, config, registry, output, std::env::temp_dir());
    let result = engine
        .run("Do work", "")
        .await
        .expect("engine should succeed");

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
    };

    let provider = Arc::new(MockLlmProvider::with_turns(vec![
        tool_use_turn(),
        tool_use_turn(),
    ]));

    let mut config = test_config();
    config.max_turns = Some(1);

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("mock_tool", "result", false)));
    let output = silent_output();

    let mut engine =
        AgentEngine::new_with_provider(provider, config, registry, output, std::env::temp_dir());
    let result = engine
        .run("Keep calling tools", "")
        .await
        .expect("should return Ok, not Err");

    assert_eq!(result.stop_reason, StopReason::MaxTurns);
    assert_eq!(result.turns, 1);
}

#[tokio::test]
async fn repeated_malformed_tool_call_stops_on_default_third_turn() {
    let dir = tempdir().expect("tempdir should be created");
    let provider = Arc::new(RecordingRequestProvider::new(vec![
        malformed_tool_turn("bad", "", json!({})),
        malformed_tool_turn("bad", "", json!({})),
        malformed_tool_turn("bad", "", json!({})),
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
    config.max_malformed_tool_call_turns = None;
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
        .expect_err("engine should surface repeated malformed tool-call loop");

    assert!(matches!(
        err,
        AgentError::RepeatedMalformedToolCall { count: 3, limit: 3 }
    ));
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
            id.as_str() == "bad"
                && **is_error
                && content.contains("Malformed tool call: empty function name")
        }),
        "malformed tool uses should have paired synthetic error results"
    );
}

#[tokio::test]
async fn repeated_malformed_tool_call_threshold_one_stops_immediately() {
    let provider = Arc::new(RecordingRequestProvider::new(vec![
        malformed_tool_turn("bad", "", json!({})),
        malformed_tool_turn("bad", "", json!({})),
    ]));
    let requests = provider.requests();

    let mut config = test_config();
    config.max_malformed_tool_call_turns = Some(1);

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
        .expect_err("engine should surface repeated malformed tool-call loop");

    assert!(matches!(
        err,
        AgentError::RepeatedMalformedToolCall { count: 1, limit: 1 }
    ));
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn repeated_malformed_tool_call_disabled_falls_back_to_max_turns() {
    let provider = Arc::new(RecordingRequestProvider::new(vec![
        malformed_tool_turn("bad", "", json!({})),
        malformed_tool_turn("bad", "", json!({})),
        malformed_tool_turn("bad", "", json!({})),
    ]));
    let requests = provider.requests();

    let mut config = test_config();
    config.max_malformed_tool_call_turns = Some(0);
    config.max_turns = Some(2);

    let mut engine = AgentEngine::new_with_provider(
        provider,
        config,
        ToolRegistry::new(),
        silent_output(),
        std::env::temp_dir(),
    );
    let result = engine
        .run("repeat malformed", "")
        .await
        .expect("engine should stop cleanly");

    assert_eq!(result.stop_reason, StopReason::MaxTurns);
    assert_eq!(result.turns, 2);
    assert_eq!(requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn mixed_valid_and_malformed_tool_calls_do_not_trip_breaker() {
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
    config.max_malformed_tool_call_turns = Some(1);
    let output = Arc::new(RecordingOutputSink::default());
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("mock_tool", "tool output", false)));

    let mut engine = AgentEngine::new_with_provider(
        provider,
        config,
        registry,
        output.clone(),
        std::env::temp_dir(),
    );
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

#[tokio::test]
async fn provider_stream_failure_maps_to_agent_provider_failure() {
    struct StreamFailureProvider {
        failure: ProviderFailure,
    }

    #[async_trait]
    impl LlmProvider for StreamFailureProvider {
        async fn stream(
            &self,
            _request: &LlmRequest,
        ) -> Result<mpsc::Receiver<Result<LlmEvent, ProviderFailure>>, ProviderFailure> {
            let (tx, rx) = mpsc::channel(1);
            let failure = self.failure.clone();
            tokio::spawn(async move {
                let _ = tx.send(Err(failure)).await;
            });
            Ok(rx)
        }
    }

    let failure = classify_provider_failure(ProviderRawSignals {
        provider: "test-provider".to_string(),
        model: Some("test-model".to_string()),
        status: None,
        raw_code: None,
        raw_type: None,
        message: Some("connection reset".to_string()),
        request_id: None,
        retry_after_ms: None,
        source: ProviderRawSignalSource::Transport,
        phase: ProviderFailurePhase::AfterFirstDelta,
    });

    let provider = Arc::new(StreamFailureProvider {
        failure: failure.clone(),
    });
    let config = test_config();
    let registry = ToolRegistry::new();
    let output = silent_output();

    let mut engine =
        AgentEngine::new_with_provider(provider, config, registry, output, std::env::temp_dir());
    let err = engine
        .run("Hello", "")
        .await
        .map(|_| panic!("expected error, got Ok"))
        .unwrap_err();

    assert!(matches!(err, AgentError::Provider(actual) if actual == failure));
}
