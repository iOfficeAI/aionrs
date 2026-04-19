use std::sync::{Arc, Mutex};

use aion_agent::confirm::ToolConfirmer;
use aion_agent::engine::AgentEngine;
use aion_agent::orchestration::execute_tool_calls;
use aion_agent::output::null_sink::NullSink;
use aion_agent::output::OutputSink;
use aion_compact::CompactionLevel;
use aion_config::compat::ProviderCompat;
use aion_config::config::{Config, ProviderType, SessionConfig, ToolsConfig};
use aion_config::hooks::HooksConfig;
use aion_mcp::config::McpConfig;
use aion_providers::create_provider;
use aion_tools::registry::ToolRegistry;
use aion_types::message::ContentBlock;
use serde_json::json;

const TEST_OUTPUT: &str = "\x1b[32mSTATUS: OK\x1b[0m\n\n\n\n50%\r100%\nCompiling dep-0 v1.0.0\nCompiling dep-1 v1.0.0\nCompiling dep-2 v1.0.0\nCompiling dep-3 v1.0.0\nCompiling dep-4 v1.0.0\n{\n    \"id\": 1,\n    \"name\": \"Alice Wonderland\",\n    \"email\": \"alice@example.com\",\n    \"age\": 30,\n    \"address\": \"123 Main Street, Anytown, USA 12345\",\n    \"phone\": \"+1-555-0123\"\n}";

#[allow(dead_code)]
const TOON_INPUT: &str =
    r#"[{"id":1,"name":"Alice","role":"admin"},{"id":2,"name":"Bob","role":"user"}]"#;

fn openai_api_key() -> Option<String> {
    std::env::var("OPENAI_API_KEY")
        .ok()
        .filter(|k| !k.is_empty())
}

fn openai_config(api_key: &str) -> Config {
    Config {
        provider: ProviderType::OpenAI,
        provider_label: "openai".to_string(),
        api_key: api_key.to_string(),
        base_url: "https://api.openai.com".to_string(),
        model: "gpt-4o-mini".to_string(),
        max_tokens: 256,
        max_turns: 3,
        system_prompt: Some(
            "You are a helpful assistant. Be concise. Answer exactly what is asked.".to_string(),
        ),
        thinking: None,
        prompt_caching: false,
        compat: ProviderCompat::openai_defaults(),
        tools: ToolsConfig {
            auto_approve: true,
            allow_list: vec![],
            skills: aion_config::config::SkillsPermissionConfig::default(),
        },
        session: SessionConfig {
            enabled: false,
            directory: "/tmp".to_string(),
            max_sessions: 1,
        },
        compact: aion_config::compact::CompactConfig::default(),
        plan: aion_config::plan::PlanConfig::default(),
        file_cache: aion_config::file_cache::FileCacheConfig::default(),
        hooks: HooksConfig::default(),
        bedrock: None,
        vertex: None,
        mcp: McpConfig::default(),
        debug: aion_config::debug::DebugConfig::default(),
    }
}

struct FixedOutputTool {
    name: String,
    output: String,
}

impl FixedOutputTool {
    fn new(name: &str, output: &str) -> Self {
        Self {
            name: name.to_string(),
            output: output.to_string(),
        }
    }
}

#[async_trait::async_trait]
impl aion_tools::Tool for FixedOutputTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        "Returns fixed output for testing"
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({"type": "object", "properties": {}, "required": []})
    }

    fn category(&self) -> aion_protocol::events::ToolCategory {
        aion_protocol::events::ToolCategory::Info
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }

    async fn execute(&self, _input: serde_json::Value) -> aion_types::tool::ToolResult {
        aion_types::tool::ToolResult {
            content: self.output.clone(),
            is_error: false,
        }
    }
}

fn extract_tool_result_content(blocks: &[ContentBlock]) -> Option<String> {
    for block in blocks {
        if let ContentBlock::ToolResult { content, .. } = block {
            return Some(content.clone());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// C Layer: Case 9 (Off vs Safe content comparison)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn case_9_off_vs_safe_content() {
    let Some(api_key) = openai_api_key() else {
        eprintln!("[e2e:compaction] OPENAI_API_KEY not set — skipping");
        return;
    };

    eprintln!("[e2e:compaction] === Case 9: Off vs Safe content comparison ===");

    let confirmer = Arc::new(Mutex::new(ToolConfirmer::new(true, vec![])));

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(FixedOutputTool::new("check_tool", TEST_OUTPUT)));
    let tool_calls = vec![ContentBlock::ToolUse {
        id: "t1".to_string(),
        name: "check_tool".to_string(),
        input: json!({}),
    }];

    // Off
    let outcome_off = execute_tool_calls(
        &registry,
        &tool_calls,
        &confirmer,
        None,
        CompactionLevel::Off,
        false,
    )
    .await
    .expect("should succeed");
    let content_off = extract_tool_result_content(&outcome_off).unwrap();

    // Safe
    let outcome_safe = execute_tool_calls(
        &registry,
        &tool_calls,
        &confirmer,
        None,
        CompactionLevel::Safe,
        false,
    )
    .await
    .expect("should succeed");
    let content_safe = extract_tool_result_content(&outcome_safe).unwrap();

    eprintln!(
        "[e2e:compaction] Off content ({} chars)",
        content_off.len()
    );
    eprintln!(
        "[e2e:compaction] Safe content ({} chars)",
        content_safe.len()
    );

    assert!(
        content_off.contains("\x1b"),
        "Off should preserve ANSI escapes"
    );
    assert!(
        !content_safe.contains("\x1b"),
        "Safe should strip ANSI escapes"
    );

    // LLM question (secondary evidence)
    let mut config = openai_config(&api_key);
    config.compact.compaction = CompactionLevel::Safe;

    let provider = create_provider(&config);
    let mut registry2 = ToolRegistry::new();
    registry2.register(Box::new(FixedOutputTool::new("check_tool", TEST_OUTPUT)));
    let output: Arc<dyn OutputSink> = Arc::new(NullSink);
    let mut engine = AgentEngine::new_with_provider(provider, config, registry2, output);

    let prompt = "Call check_tool, then answer: does the tool output contain ANSI color escape codes (sequences starting with \\x1b)? Answer only 'yes' or 'no'.";
    let result = engine
        .run(prompt, "")
        .await
        .expect("engine.run should succeed");

    eprintln!("[e2e:compaction] LLM question: does Safe output contain ANSI?");
    eprintln!("[e2e:compaction] LLM answer: {}", result.text);
    eprintln!(
        "[e2e:compaction] Token usage: {} input / {} output",
        result.usage.input_tokens, result.usage.output_tokens
    );

    let answer = result.text.to_lowercase();
    if answer.contains("no") {
        eprintln!("[e2e:compaction] ✓ LLM confirms no ANSI in Safe output");
    } else {
        eprintln!("[e2e:compaction] ⚠ LLM answer unexpected (non-deterministic, logged for review)");
    }

    eprintln!("[e2e:compaction] ✓ PASS (primary: content assertions passed)");
}
