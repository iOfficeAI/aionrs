use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::auth::{AuthConfig, OAuthManager};
use crate::compact::CompactConfig;
use crate::compat::ProviderCompat;
use crate::file_cache::FileCacheConfig;
use crate::hooks::HooksConfig;
use crate::logging::LoggingConfig;
use crate::plan::PlanConfig;
use crate::shell::ShellConfig;
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
    pub service_account_json: Option<String>,
}

/// Transport type for MCP server connections
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TransportType {
    #[default]
    Stdio,
    Sse,
    StreamableHttp,
}

/// A single MCP server configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpServerConfig {
    pub transport: TransportType,
    /// For stdio transport: the command to run
    pub command: Option<String>,
    /// For stdio transport: arguments to the command
    pub args: Option<Vec<String>>,
    /// Environment variables to set for this server (stdio)
    pub env: Option<HashMap<String, String>>,
    /// For SSE/HTTP transport: the URL
    pub url: Option<String>,
    /// HTTP headers for SSE/HTTP transports
    pub headers: Option<HashMap<String, String>>,
    /// Whether tools from this server should be deferred (name-only stub sent to LLM).
    /// Defaults to true when omitted — MCP tools are deferred by default to reduce
    /// input token usage. Set to `false` to send full schemas eagerly.
    pub deferred: Option<bool>,
    /// Startup timeout in milliseconds for connecting, initializing, and listing tools.
    /// Defaults to 30000ms when omitted.
    pub startup_timeout_ms: Option<u64>,
}

/// Collection of MCP server configurations
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct McpConfig {
    #[serde(default)]
    pub servers: HashMap<String, McpServerConfig>,
}

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
    pub compact: CompactConfig,

    #[serde(default)]
    pub plan: PlanConfig,

    #[serde(default)]
    pub shell: ShellConfig,

    #[serde(default)]
    pub file_cache: FileCacheConfig,

    #[serde(default)]
    pub hooks: HooksConfig,

    pub bedrock: Option<BedrockConfig>,
    pub vertex: Option<VertexConfig>,
    pub auth: Option<AuthConfig>,

    #[serde(default)]
    pub mcp: McpConfig,

    #[serde(default)]
    pub logging: LoggingConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DefaultConfig {
    #[serde(default = "default_provider")]
    pub provider: String,
    pub model: Option<String>,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default)]
    pub max_turns: Option<usize>,
    #[serde(default)]
    pub max_tool_call_malformed_turns: Option<usize>,
    #[serde(default)]
    pub max_tool_call_failure_turns: Option<usize>,
    pub system_prompt: Option<String>,
}

impl Default for DefaultConfig {
    fn default() -> Self {
        Self {
            provider: default_provider(),
            model: None,
            max_tokens: default_max_tokens(),
            max_turns: None,
            max_tool_call_malformed_turns: None,
            max_tool_call_failure_turns: None,
            system_prompt: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ProviderConfig {
    /// Underlying built-in provider type for a custom provider alias.
    pub provider: Option<String>,
    /// Optional default model for this provider entry.
    pub model: Option<String>,
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
    pub max_tool_call_malformed_turns: Option<usize>,
    pub max_tool_call_failure_turns: Option<usize>,
    /// Inherit settings from another profile
    pub extends: Option<String>,
    /// MCP server names to enable for this profile (references [mcp.servers.*])
    pub mcp_servers: Option<Vec<String>>,
    /// Default shell override for this profile.
    pub shell: Option<String>,
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

fn resolve_max_turns(configured: Option<usize>) -> Option<usize> {
    match configured {
        Some(0) => None,
        Some(limit) => Some(limit),
        None => None,
    }
}

// --- Resolved runtime config ---

#[derive(Debug, Clone)]
pub struct Config {
    pub provider_label: String,
    pub provider: ProviderType,
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub max_tokens: u32,
    pub max_turns: Option<usize>,
    pub max_tool_call_malformed_turns: Option<usize>,
    pub max_tool_call_failure_turns: Option<usize>,
    pub system_prompt: Option<String>,
    pub thinking: Option<ThinkingConfig>,
    pub prompt_caching: bool,
    pub compat: ProviderCompat,
    pub tools: ToolsConfig,
    pub session: SessionConfig,
    pub compact: CompactConfig,
    pub plan: PlanConfig,
    pub shell: ShellConfig,
    pub file_cache: FileCacheConfig,
    pub hooks: HooksConfig,
    pub bedrock: Option<BedrockConfig>,
    pub vertex: Option<VertexConfig>,
    pub mcp: McpConfig,
    pub logging: LoggingConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderType {
    Anthropic,
    OpenAI,
    Bedrock,
    Vertex,
}

#[derive(Debug, Clone)]
struct ResolvedProviderConfig {
    requested_name: String,
    provider_type: ProviderType,
    effective_config: ProviderConfig,
}

/// CLI arguments needed for config resolution
pub struct CliArgs {
    pub provider: Option<String>,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub max_tokens: Option<u32>,
    pub thinking: Option<String>,
    pub thinking_budget: Option<u32>,
    pub max_turns: Option<usize>,
    pub max_tool_call_malformed_turns: Option<usize>,
    pub max_tool_call_failure_turns: Option<usize>,
    pub system_prompt: Option<String>,
    pub profile: Option<String>,
    pub auto_approve: bool,
    pub project_dir: Option<PathBuf>,
}

impl Config {
    /// Load and merge config from all sources
    pub fn resolve(cli: &CliArgs) -> anyhow::Result<Self> {
        // 1. Load global config
        let global = load_config_file(&global_config_path());

        // 2. Load project config (from project_dir if specified, else CWD)
        let project_path = cli
            .project_dir
            .as_ref()
            .map(|d| d.join(".aionrs.toml"))
            .unwrap_or_else(project_config_path);
        let project = load_config_file(&project_path);

        // 3. Merge: global <- project
        let mut merged = merge_config_files(global, project);

        // 4. If --profile specified, overlay profile settings
        if let Some(profile_name) = &cli.profile {
            merged = apply_profile(merged, profile_name)?;
        }

        // 5. Apply CLI overrides and resolve final config
        let provider_str = cli.provider.as_deref().unwrap_or(&merged.default.provider);

        let resolved_provider = resolve_provider_alias(&merged.providers, provider_str)?;
        let provider_label = resolved_provider.requested_name.clone();
        let provider = resolved_provider.provider_type;
        let provider_config = resolved_provider.effective_config;

        let base_url = cli
            .base_url
            .clone()
            .or_else(|| provider_config.base_url.clone())
            .unwrap_or_else(|| match provider {
                ProviderType::Anthropic => "https://api.anthropic.com".into(),
                ProviderType::OpenAI => "https://api.openai.com/v1".into(),
                // Bedrock/Vertex URLs are constructed from region/project, not base_url
                ProviderType::Bedrock | ProviderType::Vertex => String::new(),
            });
        let base_url = normalize_base_url(provider, base_url);

        let model = cli
            .model
            .clone()
            .or(provider_config.model.clone())
            .or(merged.default.model.clone())
            .unwrap_or_else(|| match provider {
                ProviderType::Anthropic => "claude-sonnet-4-20250514".into(),
                ProviderType::OpenAI => "gpt-4o".into(),
                ProviderType::Bedrock => "anthropic.claude-sonnet-4-20250514-v1:0".into(),
                ProviderType::Vertex => "claude-sonnet-4@20250514".into(),
            });

        let max_tokens = cli.max_tokens.unwrap_or(merged.default.max_tokens);
        let max_turns = resolve_max_turns(cli.max_turns.or(merged.default.max_turns));
        let max_tool_call_malformed_turns = cli
            .max_tool_call_malformed_turns
            .or(merged.default.max_tool_call_malformed_turns);
        let max_tool_call_failure_turns = cli
            .max_tool_call_failure_turns
            .or(merged.default.max_tool_call_failure_turns);
        let system_prompt = cli.system_prompt.clone().or(merged.default.system_prompt.clone());

        // 6. Resolve API key: CLI > config file > env var
        let api_key = resolve_api_key(cli.api_key.as_deref(), provider_config.api_key.as_deref(), provider)?;

        // 7. Apply auto_approve from CLI
        let mut tools = merged.tools;
        if cli.auto_approve {
            tools.auto_approve = true;
        }

        // Resolve prompt_caching: default true for Anthropic
        let prompt_caching = provider_config
            .prompt_caching
            .unwrap_or(matches!(provider, ProviderType::Anthropic));

        // Resolve compat: provider-type defaults + user overrides
        let compat_defaults = match provider {
            ProviderType::Anthropic => ProviderCompat::anthropic_defaults(),
            ProviderType::OpenAI => ProviderCompat::openai_defaults(),
            ProviderType::Bedrock => ProviderCompat::bedrock_defaults(),
            ProviderType::Vertex => ProviderCompat::anthropic_defaults(),
        };

        let user_compat = provider_config.compat.clone().unwrap_or_default();

        let compat = ProviderCompat::merge(compat_defaults, user_compat);
        let thinking = resolve_cli_thinking(cli.thinking.as_deref(), cli.thinking_budget)?;

        Ok(Config {
            provider_label,
            provider,
            api_key,
            base_url,
            model,
            max_tokens,
            max_turns,
            max_tool_call_malformed_turns,
            max_tool_call_failure_turns,
            system_prompt,
            thinking,
            prompt_caching,
            compat,
            tools,
            session: merged.session,
            compact: merged.compact,
            plan: merged.plan,
            shell: merged.shell,
            file_cache: merged.file_cache,
            hooks: merged.hooks,
            bedrock: merged.bedrock,
            vertex: merged.vertex,
            mcp: merged.mcp,
            logging: merged.logging,
        })
    }
}

const DEFAULT_THINKING_BUDGET: u32 = 10_000;

fn resolve_cli_thinking(
    thinking: Option<&str>,
    thinking_budget: Option<u32>,
) -> anyhow::Result<Option<ThinkingConfig>> {
    match thinking {
        Some("enabled") => Ok(Some(ThinkingConfig::Enabled {
            budget_tokens: thinking_budget.unwrap_or(DEFAULT_THINKING_BUDGET),
        })),
        Some("disabled") => Ok(Some(ThinkingConfig::Disabled)),
        Some(other) => anyhow::bail!("Invalid --thinking value: {other}. Expected 'enabled' or 'disabled'."),
        None => Ok(None),
    }
}

fn normalize_base_url(provider: ProviderType, base_url: String) -> String {
    if provider != ProviderType::OpenAI {
        return base_url;
    }

    let trimmed = base_url.trim_end_matches('/');
    if trimmed.eq_ignore_ascii_case("https://api.openai.com") || trimmed.eq_ignore_ascii_case("http://api.openai.com") {
        return format!("{trimmed}/v1");
    }

    base_url
}

fn parse_builtin_provider(s: &str) -> Option<ProviderType> {
    match s {
        "anthropic" => Some(ProviderType::Anthropic),
        "openai" => Some(ProviderType::OpenAI),
        "bedrock" => Some(ProviderType::Bedrock),
        "vertex" => Some(ProviderType::Vertex),
        _ => None,
    }
}

fn merge_provider_configs(base: ProviderConfig, overlay: ProviderConfig) -> ProviderConfig {
    ProviderConfig {
        provider: overlay.provider.or(base.provider),
        model: overlay.model.or(base.model),
        api_key: overlay.api_key.or(base.api_key),
        base_url: overlay.base_url.or(base.base_url),
        prompt_caching: overlay.prompt_caching.or(base.prompt_caching),
        compat: match (base.compat, overlay.compat) {
            (Some(base), Some(overlay)) => Some(ProviderCompat::merge(base, overlay)),
            (Some(base), None) => Some(base),
            (None, Some(overlay)) => Some(overlay),
            (None, None) => None,
        },
    }
}

fn resolve_provider_alias(
    providers: &HashMap<String, ProviderConfig>,
    requested: &str,
) -> anyhow::Result<ResolvedProviderConfig> {
    if let Some(provider_type) = parse_builtin_provider(requested) {
        return Ok(ResolvedProviderConfig {
            requested_name: requested.to_string(),
            provider_type,
            effective_config: providers.get(requested).cloned().unwrap_or_default(),
        });
    }

    let alias_config = providers.get(requested).cloned().ok_or_else(|| {
        anyhow::anyhow!(
            "Unknown provider: '{}'. Expected a built-in provider (anthropic, openai, bedrock, vertex) \
             or a custom alias defined in [providers.{}].",
            requested,
            requested
        )
    })?;

    let underlying = alias_config.provider.clone().ok_or_else(|| {
        anyhow::anyhow!(
            "Provider alias '{}' requires a 'provider' field in [providers.{}] \
             that maps to a built-in type (anthropic, openai, bedrock, vertex).",
            requested,
            requested
        )
    })?;

    let provider_type = parse_builtin_provider(&underlying).ok_or_else(|| {
        anyhow::anyhow!(
            "Provider alias '{}' maps to '{}', which is not a built-in provider. \
             Use one of: anthropic, openai, bedrock, vertex.",
            requested,
            underlying
        )
    })?;

    Ok(ResolvedProviderConfig {
        requested_name: requested.to_string(),
        provider_type,
        effective_config: merge_provider_configs(providers.get(&underlying).cloned().unwrap_or_default(), alias_config),
    })
}

fn resolve_api_key(cli_key: Option<&str>, config_key: Option<&str>, provider: ProviderType) -> anyhow::Result<String> {
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
         (API_KEY, ANTHROPIC_API_KEY, or OPENAI_API_KEY), or run 'aionrs auth login'."
    )
}

// --- App directories ---

/// Platform-aware app config root.
///
/// - Linux:   `~/.config/aionrs`
/// - macOS:   `~/Library/Application Support/aionrs`
/// - Windows: `%APPDATA%\aionrs`
pub fn app_config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("aionrs"))
}

// --- Config file loading and merging ---

pub fn global_config_path() -> PathBuf {
    app_config_dir()
        .unwrap_or_else(|| PathBuf::from("aionrs"))
        .join("config.toml")
}

fn project_config_path() -> PathBuf {
    PathBuf::from(".aionrs.toml")
}

fn load_config_file(path: &Path) -> ConfigFile {
    match std::fs::read_to_string(path) {
        Ok(content) => toml::from_str(&content).unwrap_or_else(|e| {
            tracing::warn!(target: "aion_config", path = %path.display(), error = %e, "failed to parse config file");
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
        max_turns: project.default.max_turns.or(global.default.max_turns),
        max_tool_call_malformed_turns: project
            .default
            .max_tool_call_malformed_turns
            .or(global.default.max_tool_call_malformed_turns),
        max_tool_call_failure_turns: project
            .default
            .max_tool_call_failure_turns
            .or(global.default.max_tool_call_failure_turns),
        system_prompt: project.default.system_prompt.or(global.default.system_prompt),
    };

    // Merge providers: global as base, project overrides
    let mut providers = global.providers;
    for (k, v) in project.providers {
        let base = providers.remove(&k).unwrap_or_default();
        providers.insert(k, merge_provider_configs(base, v));
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
    let mcp = McpConfig { servers: mcp_servers };

    // Plan: project overrides global if any field differs from default
    let plan = if !project.plan.enabled || project.plan.plan_directory != PlanConfig::default().plan_directory {
        project.plan
    } else {
        global.plan
    };

    // File cache: project overrides global if any field differs from default.
    let file_cache = if !project.file_cache.enabled
        || project.file_cache.max_entries != FileCacheConfig::default().max_entries
        || project.file_cache.max_size_bytes != FileCacheConfig::default().max_size_bytes
    {
        project.file_cache
    } else {
        global.file_cache
    };

    // Bedrock/Vertex/Auth: project overrides global
    let bedrock = project.bedrock.or(global.bedrock);
    let vertex = project.vertex.or(global.vertex);
    let auth = project.auth.or(global.auth);

    // Compact: project overrides global for any non-default field.
    // Since CompactConfig uses serde defaults, a fully-default project config
    // is indistinguishable from "absent". We use project if its context_window
    // differs from the default, otherwise fall back to global.
    let compact =
        if project.compact.context_window != CompactConfig::default().context_window || !project.compact.enabled {
            project.compact
        } else {
            global.compact
        };

    let logging = LoggingConfig::merge(global.logging, project.logging);

    let shell = if project.shell.default != ShellConfig::default().default {
        project.shell
    } else {
        global.shell
    };

    ConfigFile {
        default,
        providers,
        profiles,
        tools,
        session,
        compact,
        plan,
        shell,
        file_cache,
        hooks,
        bedrock,
        vertex,
        auth,
        mcp,
        logging,
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
        max_tool_call_malformed_turns: overlay
            .max_tool_call_malformed_turns
            .or(base.max_tool_call_malformed_turns),
        max_tool_call_failure_turns: overlay.max_tool_call_failure_turns.or(base.max_tool_call_failure_turns),
        extends: None, // already resolved
        mcp_servers: overlay.mcp_servers.or(base.mcp_servers),
        shell: overlay.shell.or(base.shell),
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
        config.default.max_turns = Some(max_turns);
    }
    if let Some(max_tool_call_malformed_turns) = profile.max_tool_call_malformed_turns {
        config.default.max_tool_call_malformed_turns = Some(max_tool_call_malformed_turns);
    }
    if let Some(max_tool_call_failure_turns) = profile.max_tool_call_failure_turns {
        config.default.max_tool_call_failure_turns = Some(max_tool_call_failure_turns);
    }
    if let Some(shell) = profile.shell {
        config.shell.default = shell;
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
        tracing::info!(target: "aion_config", path = %path.display(), "config file already exists");
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, DEFAULT_CONFIG_TEMPLATE)?;
    tracing::info!(target: "aion_config", path = %path.display(), "config file created");
    Ok(())
}

const DEFAULT_CONFIG_TEMPLATE: &str = r#"# aionrs configuration

# Default provider settings
[default]
provider = "anthropic"            # built-in provider or custom alias from [providers.<name>]
# model = "claude-sonnet-4-20250514"
max_tokens = 8192
# max_turns = 20                  # optional max model turns per run; omit or set 0 to disable
# max_tool_call_malformed_turns = 3  # 0 disables the tool-call-malformed round breaker
# max_tool_call_failure_turns = 3    # 0 disables the tool-call-failure round breaker
# system_prompt = "..."          # optional custom system prompt

# Shell execution settings
[shell]
default = "auto"                 # auto, powershell, pwsh, cmd, bash, zsh, sh, or executable path

# Provider-specific API settings
[providers.anthropic]
# api_key = "sk-ant-xxx"         # can also use env: API_KEY or ANTHROPIC_API_KEY
# base_url = "https://api.anthropic.com"

[providers.openai]
# api_key = "sk-xxx"             # can also use env: OPENAI_API_KEY
# base_url = "https://api.openai.com/v1"

# Custom provider alias (maps to a built-in provider type)
# [providers.my-service]
# provider = "openai"
# model = "custom-model-v1"
# api_key = "sk-xxx"
# base_url = "https://my-service.example.com/api/openai"

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

# OAuth settings (for `aionrs auth login` with Claude.ai account)
# [auth]
# auth_url = "https://claude.ai/oauth"
# token_url = "https://claude.ai/oauth/token"
# client_id = "aionrs"

# Named profiles for quick switching (--profile <name>)
# [profiles.deepseek]
# provider = "openai"
# model = "deepseek-chat"
# api_key = "sk-xxx"
# base_url = "https://api.deepseek.com/v1"

# [profiles.deepseek-v4-pro]
# provider = "openai"
# model = "deepseek-v4-pro"
# api_key = "sk-xxx"
# base_url = "https://api.deepseek.com/v1"
# max_tokens = 16384
#
# [profiles.deepseek-v4-pro.compat]
# supports_thinking = true

# [profiles.ollama]
# provider = "openai"
# model = "qwen2.5:32b"
# api_key = "ollama"
# base_url = "http://localhost:11434"

# [profiles.my-service]
# provider = "my-service"

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

# Context compaction settings
# [compact]
# context_window = 200000        # context window size in tokens
# output_reserve = 20000         # tokens reserved for output
# autocompact_buffer = 13000     # buffer below effective window for autocompact trigger
# emergency_buffer = 3000        # tokens from limit for emergency block
# max_failures = 3               # consecutive failures before circuit-breaker trips
# micro_keep_recent = 5          # keep N most recent tool results
# micro_gap_seconds = 3600       # gap threshold for time-based microcompact
# compactable_tools = ["Read", "ExecCommand", "Grep", "Glob", "Write", "Edit"]
# enabled = true

# File state cache (dedup repeated reads, staleness detection)
# [file_cache]
# max_entries = 100            # max cached file entries
# max_size_bytes = 26214400    # 25 MB total cache size
# enabled = true

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

# Logging configuration
# [logging]
# enabled = true                   # enable file logging (default: false)
# level = "info"                   # log level filter (default: "info")
# dir = "~/Library/Logs/aionrs"    # log directory (default: platform-specific)

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
# startup_timeout_ms = 30000

# [mcp.servers.remote]
# transport = "sse"
# url = "http://localhost:3001/sse"

# [mcp.servers.api]
# transport = "streamable-http"
# url = "https://tools.example.com/mcp"
# headers = { Authorization = "Bearer xxx" }
"#;

#[cfg(test)]
#[path = "config_test.rs"]
mod config_test;
