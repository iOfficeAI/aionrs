use std::sync::Arc;

use aion_agent::bootstrap::AgentBootstrap;
use aion_agent::output::null_sink::NullSink;
use aion_config::compat::ProviderCompat;
use aion_config::config::{Config, ProviderType};

fn minimal_config() -> Config {
    Config {
        provider_label: "openai".into(),
        provider: ProviderType::OpenAI,
        api_key: "sk-test".into(),
        base_url: "http://localhost:0".into(),
        model: "gpt-test-model".into(),
        max_tokens: 1024,
        max_turns: Some(5),
        system_prompt: None,
        thinking: None,
        prompt_caching: false,
        compat: ProviderCompat::openai_defaults(),
        tools: Default::default(),
        session: Default::default(),
        compact: Default::default(),
        plan: Default::default(),
        file_cache: Default::default(),
        hooks: Default::default(),
        bedrock: None,
        vertex: None,
        mcp: Default::default(),
        debug: Default::default(),
    }
}

#[tokio::test]
async fn bootstrap_builds_engine_with_model_in_prompt() {
    let config = minimal_config();
    let output: Arc<dyn aion_agent::output::OutputSink> = Arc::new(NullSink);

    let result = AgentBootstrap::new(config, "/tmp/test-workspace", output)
        .build()
        .await
        .expect("bootstrap should succeed");

    assert!(!result.engine.tool_names().is_empty());
    assert!(!result.has_mcp);
    assert!(result.mcp_managers.is_empty());
}
