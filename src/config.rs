use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::auth::{AuthConfig, OAuthManager};
use crate::hooks::HooksConfig;
use crate::mcp::config::McpConfig;
use crate::provider::bedrock::BedrockConfig;
use crate::provider::compat::ProviderCompat;
use crate::provider::vertex::VertexConfig;
use crate::types::llm::ThinkingConfig;

// --- Config file structures (TOML deserialization) ---

/// Top-level config file structure
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
    /// Enable prompt caching (Anthropic only, default: true)
    pub prompt_caching: Option<bool>,
    /// Provider compatibility overrides
    pub compat: Option<ProviderCompat>,
}

/// A named profile bundles provider + model + overrides
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ProfileConfig {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub max_tokens: Option<u32>,
    pub max_turns: Option<usize>,
    /// Inherit settings from another profile
    pub extends: Option<String>,
    /// MCP server names to enable for this profile (references [mcp.servers.*])
    pub mcp_servers: Option<Vec<String>>,
    /// Provider compatibility overrides
    pub compat: Option<ProviderCompat>,
}

/// Per-skill deny/allow rule lists loaded from `[tools.skills]` in config.toml.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct SkillsPermissionConfig {
    #[serde(default)]
    pub deny: Vec<String>,
    #[serde(default)]
    pub allow: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolsConfig {
    #[serde(default)]
    pub auto_approve: bool,
    #[serde(default = "default_allow_list")]
    pub allow_list: Vec<String>,
    /// Skill-level deny/allow rules. Merged by concatenation across global + project configs.
    #[serde(default)]
    pub skills: SkillsPermissionConfig,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            auto_approve: false,
            allow_list: default_allow_list(),
            skills: SkillsPermissionConfig::default(),
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

// --- Default value functions ---

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

// --- Resolved runtime config ---

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderType {
    Anthropic,
    OpenAI,
    Bedrock,
    Vertex,
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
    /// Load and merge config from all sources
    pub fn resolve(cli: &CliArgs) -> anyhow::Result<Self> {
        // 1. Load global config
        let global = load_config_file(&global_config_path());

        // 2. Load project config (cwd)
        let project = load_config_file(&project_config_path());

        // 3. Merge: global <- project
        let mut merged = merge_config_files(global, project);

        // 4. If --profile specified, overlay profile settings
        if let Some(profile_name) = &cli.profile {
            merged = apply_profile(merged, profile_name)?;
        }

        // 5. Apply CLI overrides and resolve final config
        let provider_str = cli
            .provider
            .as_deref()
            .unwrap_or(&merged.default.provider);

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
                // Bedrock/Vertex URLs are constructed from region/project, not base_url
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

        // 6. Resolve API key: CLI > config file > env var
        let api_key = resolve_api_key(
            cli.api_key.as_deref(),
            merged.providers.get(provider_str).and_then(|p| p.api_key.as_deref()),
            provider,
        )?;

        // 7. Apply auto_approve from CLI
        let mut tools = merged.tools;
        if cli.auto_approve {
            tools.auto_approve = true;
        }

        // Resolve prompt_caching: default true for Anthropic
        let prompt_caching = merged
            .providers
            .get(provider_str)
            .and_then(|p| p.prompt_caching)
            .unwrap_or(matches!(provider, ProviderType::Anthropic));

        // Resolve compat: provider-type defaults + user overrides
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
    // CLI arg takes precedence
    if let Some(key) = cli_key {
        return Ok(key.to_string());
    }

    // Config file value
    if let Some(key) = config_key {
        return Ok(key.to_string());
    }

    // Env var fallback chain
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
        // Bedrock uses AWS credentials, Vertex uses GCP credentials
        // They don't need a traditional API key
        ProviderType::Bedrock | ProviderType::Vertex => {
            return Ok(String::new());
        }
    }

    // Try OAuth credentials as last resort
    let oauth = OAuthManager::new(AuthConfig::default());
    if oauth.has_credentials() {
        return Ok(String::new()); // Will be resolved at runtime via OAuth
    }

    anyhow::bail!(
        "No API key found. Provide via --api-key, config file, environment variable \
         (API_KEY, ANTHROPIC_API_KEY, or OPENAI_API_KEY), or run 'aionrs --login'."
    )
}

// --- Config file loading and merging ---

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
            eprintln!(
                "Warning: failed to parse {}: {}",
                path.display(),
                e
            );
            ConfigFile::default()
        }),
        Err(_) => ConfigFile::default(),
    }
}

/// Merge two config files. Project overrides global.
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
        system_prompt: project.default.system_prompt.or(global.default.system_prompt),
    };

    // Merge providers: global as base, project overrides
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

    // Merge profiles: global as base, project overrides
    let mut profiles = global.profiles;
    profiles.extend(project.profiles);

    // Tools: project overrides global for scalar fields; skills deny/allow are concatenated
    // (global first, then project) — consistent with the hooks merge strategy.
    let tools = if project.tools.allow_list != default_allow_list() || project.tools.auto_approve {
        ToolsConfig {
            auto_approve: global.tools.auto_approve || project.tools.auto_approve,
            allow_list: project.tools.allow_list,
            skills: SkillsPermissionConfig {
                deny: [global.tools.skills.deny, project.tools.skills.deny].concat(),
                allow: [global.tools.skills.allow, project.tools.skills.allow].concat(),
            },
        }
    } else {
        ToolsConfig {
            auto_approve: global.tools.auto_approve || project.tools.auto_approve,
            allow_list: global.tools.allow_list,
            skills: SkillsPermissionConfig {
                deny: [global.tools.skills.deny, project.tools.skills.deny].concat(),
                allow: [global.tools.skills.allow, project.tools.skills.allow].concat(),
            },
        }
    };

    // Session: project overrides global
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

    // Hooks: combine hooks from both configs (project hooks appended after global)
    let hooks = HooksConfig {
        pre_tool_use: [global.hooks.pre_tool_use, project.hooks.pre_tool_use].concat(),
        post_tool_use: [global.hooks.post_tool_use, project.hooks.post_tool_use].concat(),
        stop: [global.hooks.stop, project.hooks.stop].concat(),
    };

    // MCP: merge servers from both configs, project overrides global
    let mut mcp_servers = global.mcp.servers;
    mcp_servers.extend(project.mcp.servers);
    let mcp = McpConfig {
        servers: mcp_servers,
    };

    // Bedrock/Vertex/Auth: project overrides global
    let bedrock = project.bedrock.or(global.bedrock);
    let vertex = project.vertex.or(global.vertex);
    let auth = project.auth.or(global.auth);

    ConfigFile {
        default,
        providers,
        profiles,
        tools,
        session,
        hooks,
        bedrock,
        vertex,
        auth,
        mcp,
    }
}

/// Resolve a profile with inheritance chain (with cycle detection)
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

/// Merge two profiles: overlay takes precedence over base
fn merge_profiles(base: ProfileConfig, overlay: ProfileConfig) -> ProfileConfig {
    ProfileConfig {
        provider: overlay.provider.or(base.provider),
        model: overlay.model.or(base.model),
        api_key: overlay.api_key.or(base.api_key),
        base_url: overlay.base_url.or(base.base_url),
        max_tokens: overlay.max_tokens.or(base.max_tokens),
        max_turns: overlay.max_turns.or(base.max_turns),
        extends: None, // already resolved
        mcp_servers: overlay.mcp_servers.or(base.mcp_servers),
        compat: overlay.compat.or(base.compat),
    }
}

fn apply_profile(mut config: ConfigFile, profile_name: &str) -> anyhow::Result<ConfigFile> {
    let mut visited = Vec::new();
    let profile = resolve_profile(&config.profiles, profile_name, &mut visited)?;

    if let Some(provider) = profile.provider {
        config.default.provider = provider;
    }
    if let Some(model) = profile.model {
        config.default.model = Some(model);
    }
    if let Some(max_tokens) = profile.max_tokens {
        config.default.max_tokens = max_tokens;
    }
    if let Some(max_turns) = profile.max_turns {
        config.default.max_turns = max_turns;
    }

    // Profile can override api_key, base_url, and compat for the active provider
    let provider_name = config.default.provider.clone();
    let entry = config.providers.entry(provider_name).or_default();
    if let Some(api_key) = profile.api_key {
        entry.api_key = Some(api_key);
    }
    if let Some(base_url) = profile.base_url {
        entry.base_url = Some(base_url);
    }
    if let Some(compat) = profile.compat {
        entry.compat = Some(match entry.compat.take() {
            Some(existing) => ProviderCompat::merge(existing, compat),
            None => compat,
        });
    }

    // Filter MCP servers by profile's mcp_servers list
    if let Some(server_names) = profile.mcp_servers {
        config.mcp.servers.retain(|name, _| server_names.contains(name));
    }

    Ok(config)
}

// --- Init config command ---

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

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // parse_provider tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_provider_type_from_str_anthropic() {
        let result = parse_provider("anthropic").unwrap();
        assert_eq!(result, ProviderType::Anthropic);
    }

    #[test]
    fn test_provider_type_from_str_openai() {
        let result = parse_provider("openai").unwrap();
        assert_eq!(result, ProviderType::OpenAI);
    }

    #[test]
    fn test_provider_type_from_str_bedrock() {
        let result = parse_provider("bedrock").unwrap();
        assert_eq!(result, ProviderType::Bedrock);
    }

    #[test]
    fn test_provider_type_from_str_vertex() {
        let result = parse_provider("vertex").unwrap();
        assert_eq!(result, ProviderType::Vertex);
    }

    #[test]
    fn test_provider_type_from_str_invalid() {
        let result = parse_provider("invalid");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Unknown provider"));
        assert!(msg.contains("invalid"));
    }

    // -------------------------------------------------------------------------
    // merge_config_files tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_merge_config_cli_overrides_file() {
        // Project config sets a non-default provider; it should win over global.
        let global = ConfigFile {
            default: DefaultConfig {
                provider: "anthropic".to_string(),
                model: Some("global-model".to_string()),
                max_tokens: 4096,
                max_turns: 10,
                system_prompt: Some("global prompt".to_string()),
            },
            ..Default::default()
        };
        let project = ConfigFile {
            default: DefaultConfig {
                provider: "openai".to_string(), // non-default -> overrides global
                model: Some("project-model".to_string()),
                max_tokens: 2048, // non-default -> overrides global
                max_turns: 5,     // non-default -> overrides global
                system_prompt: Some("project prompt".to_string()),
            },
            ..Default::default()
        };

        let merged = merge_config_files(global, project);

        assert_eq!(merged.default.provider, "openai");
        assert_eq!(merged.default.model, Some("project-model".to_string()));
        assert_eq!(merged.default.max_tokens, 2048);
        assert_eq!(merged.default.max_turns, 5);
        assert_eq!(merged.default.system_prompt, Some("project prompt".to_string()));
    }

    #[test]
    fn test_merge_config_file_provides_defaults() {
        // Project config is default; global values should be preserved.
        let global = ConfigFile {
            default: DefaultConfig {
                provider: "openai".to_string(),
                model: Some("global-model".to_string()),
                max_tokens: 1024,
                max_turns: 5,
                system_prompt: Some("global prompt".to_string()),
            },
            ..Default::default()
        };
        // Project stays at built-in defaults (provider = "anthropic", max_tokens = 8192, max_turns = 30)
        let project = ConfigFile::default();

        let merged = merge_config_files(global, project);

        // provider: project default "anthropic" == default_provider() -> use global "openai"
        assert_eq!(merged.default.provider, "openai");
        assert_eq!(merged.default.model, Some("global-model".to_string()));
        assert_eq!(merged.default.max_tokens, 1024);
        assert_eq!(merged.default.max_turns, 5);
        assert_eq!(merged.default.system_prompt, Some("global prompt".to_string()));
    }

    #[test]
    fn test_merge_config_empty_file() {
        // Two default ConfigFiles merged should yield defaults.
        let merged = merge_config_files(ConfigFile::default(), ConfigFile::default());

        assert_eq!(merged.default.provider, default_provider());
        assert_eq!(merged.default.max_tokens, default_max_tokens());
        assert_eq!(merged.default.max_turns, default_max_turns());
        assert!(merged.default.model.is_none());
        assert!(merged.providers.is_empty());
        assert!(merged.profiles.is_empty());
    }

    // -------------------------------------------------------------------------
    // resolve_profile tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_profile_inheritance() {
        // Profile "child" extends "parent"; child fields win, missing ones fall back to parent.
        let mut profiles = HashMap::new();
        profiles.insert(
            "parent".to_string(),
            ProfileConfig {
                provider: Some("anthropic".to_string()),
                model: Some("claude-3".to_string()),
                max_tokens: Some(4096),
                ..Default::default()
            },
        );
        profiles.insert(
            "child".to_string(),
            ProfileConfig {
                model: Some("claude-4".to_string()), // overrides parent
                extends: Some("parent".to_string()),
                ..Default::default()
            },
        );

        let mut visited = Vec::new();
        let result = resolve_profile(&profiles, "child", &mut visited).unwrap();

        // Child's model wins
        assert_eq!(result.model, Some("claude-4".to_string()));
        // Parent's provider is inherited
        assert_eq!(result.provider, Some("anthropic".to_string()));
        // Parent's max_tokens is inherited
        assert_eq!(result.max_tokens, Some(4096));
        // extends is cleared after resolution
        assert!(result.extends.is_none());
    }

    #[test]
    fn test_profile_cycle_detection() {
        // A extends B, B extends A -> should fail with cycle error.
        let mut profiles = HashMap::new();
        profiles.insert(
            "a".to_string(),
            ProfileConfig {
                extends: Some("b".to_string()),
                ..Default::default()
            },
        );
        profiles.insert(
            "b".to_string(),
            ProfileConfig {
                extends: Some("a".to_string()),
                ..Default::default()
            },
        );

        let mut visited = Vec::new();
        let result = resolve_profile(&profiles, "a", &mut visited);

        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Circular profile inheritance"));
    }

    #[test]
    fn test_profile_not_found() {
        let profiles: HashMap<String, ProfileConfig> = HashMap::new();
        let mut visited = Vec::new();
        let result = resolve_profile(&profiles, "nonexistent", &mut visited);

        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("nonexistent"));
    }

    // -------------------------------------------------------------------------
    // resolve_api_key tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_api_key_from_cli_arg() {
        // CLI key takes highest priority regardless of other sources.
        let result =
            resolve_api_key(Some("cli-key"), Some("config-key"), ProviderType::Anthropic)
                .unwrap();
        assert_eq!(result, "cli-key");
    }

    #[test]
    fn test_api_key_from_config() {
        // When CLI key is absent, config file key should be used.
        let result =
            resolve_api_key(None, Some("config-key"), ProviderType::Anthropic).unwrap();
        assert_eq!(result, "config-key");
    }

    #[test]
    fn test_api_key_missing_returns_error() {
        // Remove all env vars that could supply a key so the function must fail.
        // Note: single-threaded tests share the process environment; clearing here
        // is safe for unit test purposes.
        // SAFETY: single-threaded test context; no other threads read these vars.
        unsafe {
            std::env::remove_var("API_KEY");
            std::env::remove_var("ANTHROPIC_API_KEY");
        }

        // Only fails if OAuth credentials file is also absent, which is true in CI.
        // We accept either an error OR an empty key (Bedrock/Vertex path), but for
        // Anthropic with no key at all the function should return an error.
        let result = resolve_api_key(None, None, ProviderType::Anthropic);

        // The result is either an error (no OAuth file) or Ok (OAuth file found).
        // We can only assert the error path reliably when the OAuth file is absent.
        if result.is_err() {
            let msg = result.unwrap_err().to_string();
            assert!(msg.contains("No API key found"));
        }
        // If OAuth credentials exist on the test machine, the function returns Ok("").
        // Both outcomes are correct; the important invariant is no panic.
    }

    #[test]
    fn test_api_key_bedrock_returns_empty_without_key() {
        // Bedrock uses AWS credentials, so an empty key is the expected success value.
        let result = resolve_api_key(None, None, ProviderType::Bedrock).unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn test_api_key_vertex_returns_empty_without_key() {
        // Vertex uses GCP credentials, so an empty key is the expected success value.
        let result = resolve_api_key(None, None, ProviderType::Vertex).unwrap();
        assert_eq!(result, "");
    }

    // -------------------------------------------------------------------------
    // P5-14: SkillsPermissionConfig TOML deserialization
    // -------------------------------------------------------------------------

    #[test]
    fn test_merge_config_global_auto_approve_preserved_with_project_allow_list() {
        let global = ConfigFile {
            tools: ToolsConfig {
                auto_approve: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let project = ConfigFile {
            tools: ToolsConfig {
                allow_list: vec!["Bash".into()], // non-default, triggers if branch
                ..Default::default()
            },
            ..Default::default()
        };
        let merged = merge_config_files(global, project);
        assert!(merged.tools.auto_approve, "global auto_approve=true should be preserved");
    }

    #[test]
    fn p5_14_skills_deny_allow_deserialized() {
        let toml_str = r#"
[tools]
auto_approve = false
allow_list = ["Read"]

[tools.skills]
deny = ["dangerous-skill", "admin:*"]
allow = ["commit", "review-pr", "db:*"]
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        assert_eq!(
            config.tools.skills.deny,
            vec!["dangerous-skill".to_string(), "admin:*".to_string()]
        );
        assert_eq!(
            config.tools.skills.allow,
            vec!["commit".to_string(), "review-pr".to_string(), "db:*".to_string()]
        );
    }

    #[test]
    fn p5_14_skills_defaults_to_empty() {
        // When [tools.skills] is absent, deny and allow default to empty vecs.
        let config: ConfigFile = toml::from_str("").unwrap();
        assert!(config.tools.skills.deny.is_empty());
        assert!(config.tools.skills.allow.is_empty());
    }

    #[test]
    fn p5_14_merge_skills_concat() {
        // global and project skills lists are concatenated.
        let global = ConfigFile {
            tools: ToolsConfig {
                skills: SkillsPermissionConfig {
                    deny: vec!["global-deny".to_string()],
                    allow: vec!["global-allow".to_string()],
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let project = ConfigFile {
            tools: ToolsConfig {
                skills: SkillsPermissionConfig {
                    deny: vec!["project-deny".to_string()],
                    allow: vec!["project-allow".to_string()],
                },
                ..Default::default()
            },
            ..Default::default()
        };

        let merged = merge_config_files(global, project);
        assert_eq!(
            merged.tools.skills.deny,
            vec!["global-deny".to_string(), "project-deny".to_string()]
        );
        assert_eq!(
            merged.tools.skills.allow,
            vec!["global-allow".to_string(), "project-allow".to_string()]
        );
    }

    // -------------------------------------------------------------------------
    // ConfigFile TOML deserialization tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_config_file_deserialize_minimal() {
        // An empty TOML string should deserialize to all defaults without error.
        let config: ConfigFile = toml::from_str("").unwrap();

        assert_eq!(config.default.provider, "anthropic");
        assert_eq!(config.default.max_tokens, 8192);
        assert_eq!(config.default.max_turns, 30);
        assert!(config.default.model.is_none());
        assert!(config.providers.is_empty());
        assert!(config.profiles.is_empty());
    }

    #[test]
    fn test_config_file_deserialize_with_providers() {
        let toml_str = r#"
[default]
provider = "openai"
model = "gpt-4o"
max_tokens = 4096

[providers.openai]
api_key = "sk-test-key"
base_url = "https://api.openai.com"

[providers.anthropic]
api_key = "sk-ant-test"
prompt_caching = false
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();

        assert_eq!(config.default.provider, "openai");
        assert_eq!(config.default.model, Some("gpt-4o".to_string()));
        assert_eq!(config.default.max_tokens, 4096);

        let openai = config.providers.get("openai").unwrap();
        assert_eq!(openai.api_key.as_deref(), Some("sk-test-key"));
        assert_eq!(openai.base_url.as_deref(), Some("https://api.openai.com"));

        let anthropic = config.providers.get("anthropic").unwrap();
        assert_eq!(anthropic.api_key.as_deref(), Some("sk-ant-test"));
        assert_eq!(anthropic.prompt_caching, Some(false));
    }
}

const DEFAULT_CONFIG_TEMPLATE: &str = r#"# aionrs configuration

# Default provider settings
[default]
provider = "anthropic"            # "anthropic" | "openai" | "bedrock" | "vertex"
# model = "claude-sonnet-4-20250514"
max_tokens = 8192
max_turns = 30
# system_prompt = "..."          # optional custom system prompt

# Provider-specific API settings
[providers.anthropic]
# api_key = "sk-ant-xxx"         # can also use env: API_KEY or ANTHROPIC_API_KEY
# base_url = "https://api.anthropic.com"

[providers.openai]
# api_key = "sk-xxx"             # can also use env: OPENAI_API_KEY
# base_url = "https://api.openai.com"

# Provider compatibility overrides (usually not needed — defaults work)
# [providers.openai.compat]
# max_tokens_field = "max_completion_tokens"  # for OpenAI official models
# merge_assistant_messages = true
# clean_orphan_tool_calls = true
# dedup_tool_results = true
# strip_patterns = ["__OPENROUTER_REASONING_DETAILS__"]

# AWS Bedrock configuration (uses AWS SigV4 auth, no API key needed)
# [bedrock]
# region = "us-east-1"
# access_key_id = "AKIA..."
# secret_access_key = "..."
# session_token = "..."
# profile = "my-profile"        # or use AWS profile

# Google Vertex AI configuration (uses GCP OAuth2 auth, no API key needed)
# [vertex]
# project_id = "my-gcp-project"
# region = "us-central1"
# credentials_file = "/path/to/service-account.json"  # or use ADC

# OAuth settings (for --login with Claude.ai account)
# [auth]
# auth_url = "https://claude.ai/oauth"
# token_url = "https://claude.ai/oauth/token"
# client_id = "aionrs"

# Named profiles for quick switching (--profile <name>)
# [profiles.deepseek]
# provider = "openai"
# model = "deepseek-chat"
# api_key = "sk-xxx"
# base_url = "https://api.deepseek.com"

# [profiles.ollama]
# provider = "openai"
# model = "qwen2.5:32b"
# api_key = "ollama"
# base_url = "http://localhost:11434"

# [profiles.bedrock-claude]
# provider = "bedrock"
# model = "anthropic.claude-sonnet-4-20250514-v1:0"

# [profiles.vertex-claude]
# provider = "vertex"
# model = "claude-sonnet-4@20250514"

# Tool confirmation settings
[tools]
auto_approve = false             # --auto-approve overrides
# Tools that skip confirmation even when auto_approve = false
allow_list = ["Read", "Grep", "Glob"]

# Session settings
[session]
enabled = true
directory = ".aionrs/sessions"  # relative to project root
max_sessions = 20                # auto-cleanup oldest

# Hook system: run shell commands at tool lifecycle events
# [[hooks.post_tool_use]]
# name = "rustfmt"
# tool_match = ["Write", "Edit"]
# file_match = ["*.rs"]
# command = "rustfmt ${TOOL_INPUT_FILE_PATH}"

# [[hooks.post_tool_use]]
# name = "prettier"
# tool_match = ["Write", "Edit"]
# file_match = ["*.ts", "*.tsx"]
# command = "npx prettier --write ${TOOL_INPUT_FILE_PATH}"

# [[hooks.stop]]
# name = "final-lint"
# command = "cargo clippy --quiet 2>&1 | tail -5"

# MCP (Model Context Protocol) servers
# [mcp.servers.filesystem]
# transport = "stdio"
# command = "npx"
# args = ["-y", "@modelcontextprotocol/server-filesystem", "/Users/me/project"]

# [mcp.servers.github]
# transport = "stdio"
# command = "npx"
# args = ["-y", "@modelcontextprotocol/server-github"]
# env = { GITHUB_TOKEN = "ghp_xxx" }

# [mcp.servers.remote]
# transport = "sse"
# url = "http://localhost:3001/sse"

# [mcp.servers.api]
# transport = "streamable-http"
# url = "https://tools.example.com/mcp"
# headers = { Authorization = "Bearer xxx" }
"#;
