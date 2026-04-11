use std::sync::Arc;

use aion_agent::engine::AgentEngine;
use aion_agent::output::OutputSink;
use aion_agent::output::terminal::TerminalSink;
use aion_config::compat::ProviderCompat;
use aion_config::config::{Config, ProviderType, SessionConfig, ToolsConfig};
use aion_config::hooks::HooksConfig;
use aion_mcp::config::McpConfig;
use aion_providers::create_provider;
use aion_tools::read::ReadTool;
use aion_tools::registry::ToolRegistry;

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
        model: "gpt-4o-mini".to_string(), // cheapest for e2e
        max_tokens: 256,
        max_turns: 3,
        system_prompt: Some("You are a helpful assistant. Be concise.".to_string()),
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
    }
}

/// Smoke test: single-turn text completion.
#[tokio::test]
async fn test_openai_single_turn_completion() {
    let Some(api_key) = openai_api_key() else {
        eprintln!("[e2e] OPENAI_API_KEY not set — skipping");
        return;
    };

    let config = openai_config(&api_key);
    let provider = create_provider(&config);
    let output: Arc<dyn OutputSink> = Arc::new(TerminalSink::new(true));
    let registry = ToolRegistry::new();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    let result = engine
        .run("Say 'hello world' and nothing else.", "")
        .await
        .expect("engine.run should not fail");

    assert!(!result.text.is_empty(), "response text should not be empty");
    assert!(result.usage.output_tokens > 0);

    eprintln!(
        "[e2e] openai single-turn: {} tokens in / {} out",
        result.usage.input_tokens, result.usage.output_tokens
    );
}

/// Tool-use smoke test: agent calls Read tool when asked to read a file.
#[tokio::test]
async fn test_openai_tool_use() {
    let Some(api_key) = openai_api_key() else {
        eprintln!("[e2e] OPENAI_API_KEY not set — skipping");
        return;
    };

    let tmp = tempfile::NamedTempFile::new().expect("tempfile");
    std::fs::write(tmp.path(), "e2e-openai-content-99").expect("write tempfile");
    let path = tmp.path().to_string_lossy().to_string();

    let config = openai_config(&api_key);
    let provider = create_provider(&config);
    let output: Arc<dyn OutputSink> = Arc::new(TerminalSink::new(true));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ReadTool::new(None)));

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    let prompt = format!(
        "Read the file at '{}' and tell me what it contains. Be brief.",
        path
    );
    let result = engine
        .run(&prompt, "")
        .await
        .expect("engine.run should not fail");

    assert!(!result.text.is_empty());
    assert!(
        result.text.contains("e2e-openai-content-99") || result.turns > 1,
        "model should echo the content or use multiple turns: {}",
        result.text
    );

    eprintln!(
        "[e2e] openai tool-use: {} turns, {} tokens out",
        result.turns, result.usage.output_tokens
    );
}
