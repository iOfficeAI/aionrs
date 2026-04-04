use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::auth::{AuthConfig, OAuthManager};
use crate::compat::ProviderCompat;
use crate::hooks::HooksConfig;
use aion_types::llm::ThinkingConfig;

// ---------------------------------------------------------------------------
// Provider-specific sub-configurations (defined here to avoid circular deps)
// ---------------------------------------------------------------------------

/// AWS Bedrock credentials configuration
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct BedrockConfig {
    pub region: Option<String>,
    pub access_key_id: Option<String>,
    pub secret_access_key: Option<String>,
    pub session_token: Option<String>,
    pub profile: Option<String>,
}

/// Google Vertex AI authentication configuration
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct VertexConfig {
    pub project_id: Option<String>,
    pub region: Option<String>,
    pub credentials_file: Option<String>,
}

/// MCP server configuration
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct McpConfig {
    #[serde(default)]
    pub servers: HashMap<String, McpServerConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpServerConfig {
    pub transport: TransportType,
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub env: Option<HashMap<String, String>>,
    pub url: Option<String>,
    pub headers: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TransportType {
    Stdio,
    Sse,
    StreamableHttp,
}

// ---------------------------------------------------------------------------
// Config file (TOML) structures
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ConfigFile {
    #[serde(default)]
    pub default: DefaultConfig,
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,
    #[serde(default)]
    pub profiles: HashMap<String, ProfileConfig>,
    #[serde(default)]
    pub tools: ToolsConfig,
    #[serde(default)]
    pub session: SessionConfig,
    #[serde(default)]
    pub hooks: HooksConfig,
    pub bedrock: Option<BedrockConfig>,
    pub vertex: Option<VertexConfig>,
    pub auth: Option<AuthConfig>,
    #[serde(default)]
    pub mcp: McpConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DefaultConfig {
    #[serde(default = "default_provider")]
    pub provider: String,
    pub model: Option<String>,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_max_turns")]
    pub max_turns: usize,
    pub system_prompt: Option<String>,
}

impl Default for DefaultConfig {
    fn default() -> Self {
        Self {
            provider: default_provider(),
            model: None,
            max_tokens: default_max_tokens(),
            max_turns: default_max_turns(),
            system_prompt: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ProviderConfig {
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub prompt_caching: Option<bool>,
    pub compat: Option<ProviderCompat>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ProfileConfig {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub max_tokens: Option<u32>,
    pub max_turns: Option<usize>,
    pub extends: Option<String>,
    pub mcp_servers: Option<Vec<String>>,
    pub compat: Option<ProviderCompat>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolsConfig {
    #[serde(default)]
    pub auto_approve: bool,
    #[serde(default = "default_allow_list")]
    pub allow_list: Vec<String>,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            auto_approve: false,
            allow_list: default_allow_list(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SessionConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_session_dir")]
    pub directory: String,
    #[serde(default = "default_max_sessions")]
    pub max_sessions: usize,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            directory: default_session_dir(),
            max_sessions: default_max_sessions(),
        }
    }
}

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

fn default_provider() -> String {
    "anthropic".to_string()
}
fn default_max_tokens() -> u32 {
    8192
}
fn default_max_turns() -> usize {
    30
}
fn default_allow_list() -> Vec<String> {
    vec!["Read".into(), "Grep".into(), "Glob".into()]
}
fn default_true() -> bool {
    true
}
fn default_session_dir() -> String {
    ".aionrs/sessions".to_string()
}
fn default_max_sessions() -> usize {
    20
}

// ---------------------------------------------------------------------------
// Resolved runtime Config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderType {
    Anthropic,
    OpenAI,
    Bedrock,
    Vertex,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub provider: ProviderType,
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub max_tokens: u32,
    pub max_turns: usize,
    pub system_prompt: Option<String>,
    pub thinking: Option<ThinkingConfig>,
    pub prompt_caching: bool,
    pub compat: ProviderCompat,
    pub tools: ToolsConfig,
    pub session: SessionConfig,
    pub hooks: HooksConfig,
    pub bedrock: Option<BedrockConfig>,
    pub vertex: Option<VertexConfig>,
    pub mcp: McpConfig,
}

/// CLI arguments needed for config resolution
pub struct CliArgs {
    pub provider: Option<String>,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub max_tokens: Option<u32>,
    pub max_turns: Option<usize>,
    pub system_prompt: Option<String>,
    pub profile: Option<String>,
    pub auto_approve: bool,
}

impl Config {
    pub fn resolve(cli: &CliArgs) -> anyhow::Result<Self> {
        let global = load_config_file(&global_config_path());
        let project = load_config_file(&project_config_path());
        let mut merged = merge_config_files(global, project);

        if let Some(profile_name) = &cli.profile {
            merged = apply_profile(merged, profile_name)?;
        }

        let provider_str = cli.provider.as_deref().unwrap_or(&merged.default.provider);
        let provider = parse_provider(provider_str)?;

        let base_url = cli
            .base_url
            .clone()
            .or_else(|| {
                merged
                    .providers
                    .get(provider_str)
                    .and_then(|p| p.base_url.clone())
            })
            .unwrap_or_else(|| match provider {
                ProviderType::Anthropic => "https://api.anthropic.com".into(),
                ProviderType::OpenAI => "https://api.openai.com".into(),
                ProviderType::Bedrock | ProviderType::Vertex => String::new(),
            });

        let model = cli
            .model
            .clone()
            .or(merged.default.model.clone())
            .unwrap_or_else(|| match provider {
                ProviderType::Anthropic => "claude-sonnet-4-20250514".into(),
                ProviderType::OpenAI => "gpt-4o".into(),
                ProviderType::Bedrock => "anthropic.claude-sonnet-4-20250514-v1:0".into(),
                ProviderType::Vertex => "claude-sonnet-4@20250514".into(),
            });

        let max_tokens = cli.max_tokens.unwrap_or(merged.default.max_tokens);
        let max_turns = cli.max_turns.unwrap_or(merged.default.max_turns);
        let system_prompt = cli
            .system_prompt
            .clone()
            .or(merged.default.system_prompt.clone());

        let api_key = resolve_api_key(
            cli.api_key.as_deref(),
            merged
                .providers
                .get(provider_str)
                .and_then(|p| p.api_key.as_deref()),
            provider,
        )?;

        let mut tools = merged.tools;
        if cli.auto_approve {
            tools.auto_approve = true;
        }

        let prompt_caching = merged
            .providers
            .get(provider_str)
            .and_then(|p| p.prompt_caching)
            .unwrap_or(matches!(provider, ProviderType::Anthropic));

        let compat_defaults = match provider {
            ProviderType::Anthropic => ProviderCompat::anthropic_defaults(),
            ProviderType::OpenAI => ProviderCompat::openai_defaults(),
            ProviderType::Bedrock => ProviderCompat::bedrock_defaults(),
            ProviderType::Vertex => ProviderCompat::anthropic_defaults(),
        };
        let user_compat = merged
            .providers
            .get(provider_str)
            .and_then(|p| p.compat.clone())
            .unwrap_or_default();
        let compat = ProviderCompat::merge(compat_defaults, user_compat);

        Ok(Config {
            provider,
            api_key,
            base_url,
            model,
            max_tokens,
            max_turns,
            system_prompt,
            thinking: None,
            prompt_caching,
            compat,
            tools,
            session: merged.session,
            hooks: merged.hooks,
            bedrock: merged.bedrock,
            vertex: merged.vertex,
            mcp: merged.mcp,
        })
    }
}

fn parse_provider(s: &str) -> anyhow::Result<ProviderType> {
    match s {
        "anthropic" => Ok(ProviderType::Anthropic),
        "openai" => Ok(ProviderType::OpenAI),
        "bedrock" => Ok(ProviderType::Bedrock),
        "vertex" => Ok(ProviderType::Vertex),
        other => anyhow::bail!(
            "Unknown provider: '{}'. Use 'anthropic', 'openai', 'bedrock', or 'vertex'.",
            other
        ),
    }
}

fn resolve_api_key(
    cli_key: Option<&str>,
    config_key: Option<&str>,
    provider: ProviderType,
) -> anyhow::Result<String> {
    if let Some(key) = cli_key {
        return Ok(key.to_string());
    }
    if let Some(key) = config_key {
        return Ok(key.to_string());
    }
    if let Ok(key) = std::env::var("API_KEY") {
        return Ok(key);
    }
    match provider {
        ProviderType::Anthropic => {
            if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
                return Ok(key);
            }
        }
        ProviderType::OpenAI => {
            if let Ok(key) = std::env::var("OPENAI_API_KEY") {
                return Ok(key);
            }
        }
        ProviderType::Bedrock | ProviderType::Vertex => {
            return Ok(String::new());
        }
    }
    let oauth = OAuthManager::new(AuthConfig::default());
    if oauth.has_credentials() {
        return Ok(String::new());
    }
    anyhow::bail!(
        "No API key found. Provide via --api-key, config file, \
         environment variable (API_KEY, ANTHROPIC_API_KEY, OPENAI_API_KEY), \
         or run 'aionrs --login'."
    )
}

pub fn global_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("aionrs")
        .join("config.toml")
}

fn project_config_path() -> PathBuf {
    PathBuf::from(".aionrs.toml")
}

fn load_config_file(path: &Path) -> ConfigFile {
    match std::fs::read_to_string(path) {
        Ok(content) => toml::from_str(&content).unwrap_or_else(|e| {
            eprintln!("Warning: failed to parse {}: {}", path.display(), e);
            ConfigFile::default()
        }),
        Err(_) => ConfigFile::default(),
    }
}

pub fn init_config() -> anyhow::Result<()> {
    let path = global_config_path();
    if path.exists() {
        eprintln!("Config already exists: {}", path.display());
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, DEFAULT_CONFIG_TEMPLATE)?;
    eprintln!("Config created: {}", path.display());
    Ok(())
}

fn merge_config_files(global: ConfigFile, project: ConfigFile) -> ConfigFile {
    let default = DefaultConfig {
        provider: if project.default.provider != default_provider() {
            project.default.provider
        } else {
            global.default.provider
        },
        model: project.default.model.or(global.default.model),
        max_tokens: if project.default.max_tokens != default_max_tokens() {
            project.default.max_tokens
        } else {
            global.default.max_tokens
        },
        max_turns: if project.default.max_turns != default_max_turns() {
            project.default.max_turns
        } else {
            global.default.max_turns
        },
        system_prompt: project
            .default
            .system_prompt
            .or(global.default.system_prompt),
    };

    let mut providers = global.providers;
    for (k, v) in project.providers {
        let entry = providers.entry(k).or_default();
        if v.api_key.is_some() {
            entry.api_key = v.api_key;
        }
        if v.base_url.is_some() {
            entry.base_url = v.base_url;
        }
        if v.prompt_caching.is_some() {
            entry.prompt_caching = v.prompt_caching;
        }
        if v.compat.is_some() {
            entry.compat = v.compat;
        }
    }

    let mut profiles = global.profiles;
    profiles.extend(project.profiles);

    let tools = if project.tools.allow_list != default_allow_list() || project.tools.auto_approve {
        project.tools
    } else {
        ToolsConfig {
            auto_approve: global.tools.auto_approve || project.tools.auto_approve,
            allow_list: if project.tools.allow_list != default_allow_list() {
                project.tools.allow_list
            } else {
                global.tools.allow_list
            },
        }
    };

    let session = if project.session.directory != default_session_dir() {
        project.session
    } else {
        SessionConfig {
            enabled: global.session.enabled && project.session.enabled,
            directory: if project.session.directory != default_session_dir() {
                project.session.directory
            } else {
                global.session.directory
            },
            max_sessions: if project.session.max_sessions != default_max_sessions() {
                project.session.max_sessions
            } else {
                global.session.max_sessions
            },
        }
    };

    let hooks = HooksConfig {
        pre_tool_use: [global.hooks.pre_tool_use, project.hooks.pre_tool_use].concat(),
        post_tool_use: [global.hooks.post_tool_use, project.hooks.post_tool_use].concat(),
        stop: [global.hooks.stop, project.hooks.stop].concat(),
    };

    let mut mcp_servers = global.mcp.servers;
    mcp_servers.extend(project.mcp.servers);

    ConfigFile {
        default,
        providers,
        profiles,
        tools,
        session,
        hooks,
        bedrock: project.bedrock.or(global.bedrock),
        vertex: project.vertex.or(global.vertex),
        auth: project.auth.or(global.auth),
        mcp: McpConfig {
            servers: mcp_servers,
        },
    }
}

fn resolve_profile(
    profiles: &HashMap<String, ProfileConfig>,
    name: &str,
    visited: &mut Vec<String>,
) -> anyhow::Result<ProfileConfig> {
    if visited.contains(&name.to_string()) {
        anyhow::bail!(
            "Circular profile inheritance detected: {} -> {}",
            visited.join(" -> "),
            name
        );
    }
    visited.push(name.to_string());
    let profile = profiles
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("Profile '{}' not found in config", name))?
        .clone();
    if let Some(parent_name) = &profile.extends {
        let parent = resolve_profile(profiles, parent_name, visited)?;
        Ok(merge_profiles(parent, profile))
    } else {
        Ok(profile)
    }
}

fn merge_profiles(base: ProfileConfig, overlay: ProfileConfig) -> ProfileConfig {
    ProfileConfig {
        provider: overlay.provider.or(base.provider),
        model: overlay.model.or(base.model),
        api_key: overlay.api_key.or(base.api_key),
        base_url: overlay.base_url.or(base.base_url),
        max_tokens: overlay.max_tokens.or(base.max_tokens),
        max_turns: overlay.max_turns.or(base.max_turns),
        extends: None,
        mcp_servers: overlay.mcp_servers.or(base.mcp_servers),
        compat: overlay.compat.or(base.compat),
    }
}

fn apply_profile(mut config: ConfigFile, profile_name: &str) -> anyhow::Result<ConfigFile> {
    let mut visited = Vec::new();
    let profile = resolve_profile(&config.profiles, profile_name, &mut visited)?;

    if let Some(p) = profile.provider {
        config.default.provider = p;
    }
    if let Some(m) = profile.model {
        config.default.model = Some(m);
    }
    if let Some(t) = profile.max_tokens {
        config.default.max_tokens = t;
    }
    if let Some(t) = profile.max_turns {
        config.default.max_turns = t;
    }

    let provider_name = config.default.provider.clone();
    let entry = config.providers.entry(provider_name).or_default();
    if let Some(k) = profile.api_key {
        entry.api_key = Some(k);
    }
    if let Some(u) = profile.base_url {
        entry.base_url = Some(u);
    }
    if let Some(c) = profile.compat {
        entry.compat = Some(match entry.compat.take() {
            Some(existing) => ProviderCompat::merge(existing, c),
            None => c,
        });
    }

    if let Some(server_names) = profile.mcp_servers {
        config
            .mcp
            .servers
            .retain(|name, _| server_names.contains(name));
    }

    Ok(config)
}

const DEFAULT_CONFIG_TEMPLATE: &str = r#"# aionrs configuration

[default]
provider = "anthropic"
max_tokens = 8192
max_turns = 30

[providers.anthropic]
# api_key = "sk-ant-xxx"

[providers.openai]
# api_key = "sk-xxx"

[tools]
auto_approve = false
allow_list = ["Read", "Grep", "Glob"]

[session]
enabled = true
directory = ".aionrs/sessions"
max_sessions = 20
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_type_from_str_all() {
        assert_eq!(
            parse_provider("anthropic").unwrap(),
            ProviderType::Anthropic
        );
        assert_eq!(parse_provider("openai").unwrap(), ProviderType::OpenAI);
        assert_eq!(parse_provider("bedrock").unwrap(), ProviderType::Bedrock);
        assert_eq!(parse_provider("vertex").unwrap(), ProviderType::Vertex);
        assert!(parse_provider("invalid").is_err());
    }

    #[test]
    fn test_merge_config_empty_files() {
        let merged = merge_config_files(ConfigFile::default(), ConfigFile::default());
        assert_eq!(merged.default.provider, "anthropic");
        assert_eq!(merged.default.max_tokens, 8192);
    }

    #[test]
    fn test_api_key_bedrock_returns_empty() {
        let result = resolve_api_key(None, None, ProviderType::Bedrock).unwrap();
        assert_eq!(result, "");
    }
}
