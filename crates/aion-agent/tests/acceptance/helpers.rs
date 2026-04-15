// Shared helpers for acceptance tests: provider detection and config builders.

use aion_config::compat::ProviderCompat;
use aion_config::config::{BedrockConfig, Config, ProviderType, SessionConfig, ToolsConfig};
use aion_config::hooks::HooksConfig;
use aion_mcp::config::McpConfig;

// ---------------------------------------------------------------------------
// Provider detection
// ---------------------------------------------------------------------------

/// Returns the OpenAI API key if set and non-empty.
pub fn openai_api_key() -> Option<String> {
    std::env::var("OPENAI_API_KEY")
        .ok()
        .filter(|k| !k.is_empty())
}

/// Returns true when AWS Bedrock is configured for use.
pub fn bedrock_configured() -> bool {
    let has_profile = std::env::var("AWS_PROFILE")
        .ok()
        .filter(|v| !v.is_empty())
        .is_some();
    let bedrock_flag = std::env::var("CLAUDE_CODE_USE_BEDROCK")
        .ok()
        .filter(|v| v == "1")
        .is_some();
    has_profile && bedrock_flag
}

// ---------------------------------------------------------------------------
// Skip macros
// ---------------------------------------------------------------------------

/// Skips the current test if OPENAI_API_KEY is not set.
/// Usage: `skip_if_no_openai!();` at the start of a test function.
macro_rules! skip_if_no_openai {
    () => {
        #[allow(unused_variables)]
        let openai_api_key = match $crate::helpers::openai_api_key() {
            Some(k) => k,
            None => {
                eprintln!("[acceptance] OPENAI_API_KEY not set — skipping");
                return;
            }
        };
    };
}

/// Skips the current test if Bedrock is not configured.
/// Usage: `skip_if_no_bedrock!();` at the start of a test function.
macro_rules! skip_if_no_bedrock {
    () => {
        if !$crate::helpers::bedrock_configured() {
            eprintln!("[acceptance] Bedrock not configured — skipping");
            return;
        }
    };
}

// ---------------------------------------------------------------------------
// Config builders
// ---------------------------------------------------------------------------

/// Build a Config for the OpenAI provider (gpt-4o-mini, cheap for tests).
pub fn openai_config(api_key: &str) -> Config {
    Config {
        provider: ProviderType::OpenAI,
        provider_label: "openai".to_string(),
        api_key: api_key.to_string(),
        base_url: "https://api.openai.com".to_string(),
        model: "gpt-4o-mini".to_string(),
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
            directory: "/tmp/aionrs-acceptance".to_string(),
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

/// Build a Config for the AWS Bedrock provider (Claude Haiku).
pub fn bedrock_config() -> Config {
    Config {
        provider: ProviderType::Bedrock,
        provider_label: "bedrock".to_string(),
        api_key: String::new(), // Bedrock uses AWS credentials, not API key
        base_url: String::new(),
        model: "us.anthropic.claude-haiku-4-20250514-v1:0".to_string(),
        max_tokens: 256,
        max_turns: 3,
        system_prompt: Some("You are a helpful assistant. Be concise.".to_string()),
        thinking: None,
        prompt_caching: false,
        compat: ProviderCompat::anthropic_defaults(),
        tools: ToolsConfig {
            auto_approve: true,
            allow_list: vec![],
            skills: aion_config::config::SkillsPermissionConfig::default(),
        },
        session: SessionConfig {
            enabled: false,
            directory: "/tmp/aionrs-acceptance".to_string(),
            max_sessions: 1,
        },
        compact: aion_config::compact::CompactConfig::default(),
        plan: aion_config::plan::PlanConfig::default(),
        file_cache: aion_config::file_cache::FileCacheConfig::default(),
        hooks: HooksConfig::default(),
        bedrock: Some(BedrockConfig::default()),
        vertex: None,
        mcp: McpConfig::default(),
        debug: aion_config::debug::DebugConfig::default(),
    }
}
