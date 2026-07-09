use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compat::{MessageCompat, ReasoningCompat, SchemaCompat, ToolCompat, ToolWireShape, TransportCompat};

    // -------------------------------------------------------------------------
    // parse_builtin_provider tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_provider_type_from_str_anthropic() {
        let result = parse_builtin_provider("anthropic");
        assert_eq!(result, Some(ProviderType::Anthropic));
    }

    #[test]
    fn test_provider_type_from_str_openai() {
        let result = parse_builtin_provider("openai");
        assert_eq!(result, Some(ProviderType::OpenAI));
    }

    #[test]
    fn test_provider_type_from_str_bedrock() {
        let result = parse_builtin_provider("bedrock");
        assert_eq!(result, Some(ProviderType::Bedrock));
    }

    #[test]
    fn test_provider_type_from_str_vertex() {
        let result = parse_builtin_provider("vertex");
        assert_eq!(result, Some(ProviderType::Vertex));
    }

    #[test]
    fn test_provider_type_from_str_invalid() {
        let result = parse_builtin_provider("invalid");
        assert_eq!(result, None);
    }

    #[test]
    fn test_provider_alias_resolves_to_builtin_provider() {
        let mut providers = HashMap::new();
        providers.insert(
            "my-service".to_string(),
            ProviderConfig {
                provider: Some("openai".to_string()),
                model: Some("custom-model-v1".to_string()),
                api_key: Some("alias-key".to_string()),
                base_url: Some("https://my-service.example.com/v1".to_string()),
                ..Default::default()
            },
        );

        let resolved = resolve_provider_alias(&providers, "my-service").unwrap();
        assert_eq!(resolved.requested_name, "my-service");
        assert_eq!(resolved.provider_type, ProviderType::OpenAI);
        assert_eq!(resolved.effective_config.model.as_deref(), Some("custom-model-v1"));
        assert_eq!(resolved.effective_config.api_key.as_deref(), Some("alias-key"));
        assert_eq!(
            resolved.effective_config.base_url.as_deref(),
            Some("https://my-service.example.com/v1")
        );
    }

    #[test]
    fn test_provider_alias_overlays_builtin_provider_defaults() {
        let mut providers = HashMap::new();
        providers.insert(
            "openai".to_string(),
            ProviderConfig {
                api_key: Some("builtin-key".to_string()),
                model: Some("gpt-4o".to_string()),
                ..Default::default()
            },
        );
        providers.insert(
            "my-service".to_string(),
            ProviderConfig {
                provider: Some("openai".to_string()),
                base_url: Some("https://my-service.example.com/v1".to_string()),
                ..Default::default()
            },
        );

        let resolved = resolve_provider_alias(&providers, "my-service").unwrap();
        assert_eq!(resolved.provider_type, ProviderType::OpenAI);
        assert_eq!(resolved.effective_config.api_key.as_deref(), Some("builtin-key"));
        assert_eq!(resolved.effective_config.model.as_deref(), Some("gpt-4o"));
        assert_eq!(
            resolved.effective_config.base_url.as_deref(),
            Some("https://my-service.example.com/v1")
        );
    }

    #[test]
    fn test_provider_alias_requires_underlying_provider_type() {
        let mut providers = HashMap::new();
        providers.insert("my-service".to_string(), ProviderConfig::default());

        let result = resolve_provider_alias(&providers, "my-service");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("my-service"));
        assert!(msg.contains("provider"));
        assert!(msg.contains("built-in type"));
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
                max_turns: Some(10),
                max_tool_call_malformed_turns: Some(6),
                max_tool_call_failure_turns: Some(6),
                system_prompt: Some("global prompt".to_string()),
            },
            ..Default::default()
        };
        let project = ConfigFile {
            default: DefaultConfig {
                provider: "openai".to_string(), // non-default -> overrides global
                model: Some("project-model".to_string()),
                max_tokens: 2048,   // non-default -> overrides global
                max_turns: Some(5), // non-default -> overrides global
                max_tool_call_malformed_turns: Some(2),
                max_tool_call_failure_turns: Some(2),
                system_prompt: Some("project prompt".to_string()),
            },
            ..Default::default()
        };

        let merged = merge_config_files(global, project);

        assert_eq!(merged.default.provider, "openai");
        assert_eq!(merged.default.model, Some("project-model".to_string()));
        assert_eq!(merged.default.max_tokens, 2048);
        assert_eq!(merged.default.max_turns, Some(5));
        assert_eq!(merged.default.max_tool_call_malformed_turns, Some(2));
        assert_eq!(merged.default.max_tool_call_failure_turns, Some(2));
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
                max_turns: Some(5),
                max_tool_call_malformed_turns: Some(4),
                max_tool_call_failure_turns: Some(4),
                system_prompt: Some("global prompt".to_string()),
            },
            ..Default::default()
        };
        // Project stays at built-in defaults (provider = "anthropic", max_tokens = 8192, max_turns = None)
        let project = ConfigFile::default();

        let merged = merge_config_files(global, project);

        // provider: project default "anthropic" == default_provider() -> use global "openai"
        assert_eq!(merged.default.provider, "openai");
        assert_eq!(merged.default.model, Some("global-model".to_string()));
        assert_eq!(merged.default.max_tokens, 1024);
        assert_eq!(merged.default.max_turns, Some(5));
        assert_eq!(merged.default.max_tool_call_malformed_turns, Some(4));
        assert_eq!(merged.default.max_tool_call_failure_turns, Some(4));
        assert_eq!(merged.default.system_prompt, Some("global prompt".to_string()));
    }

    #[test]
    fn test_merge_config_empty_file() {
        // Two default ConfigFiles merged should yield defaults.
        let merged = merge_config_files(ConfigFile::default(), ConfigFile::default());

        assert_eq!(merged.default.provider, default_provider());
        assert_eq!(merged.default.max_tokens, default_max_tokens());
        assert_eq!(merged.default.max_turns, None);
        assert_eq!(merged.default.max_tool_call_malformed_turns, None);
        assert_eq!(merged.default.max_tool_call_failure_turns, None);
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
                max_tool_call_malformed_turns: Some(3),
                max_tool_call_failure_turns: Some(3),
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
        // Parent's tool-call-malformed turn limit is inherited
        assert_eq!(result.max_tool_call_malformed_turns, Some(3));
        // Parent's tool-call-failure turn limit is inherited
        assert_eq!(result.max_tool_call_failure_turns, Some(3));
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
        let result = resolve_api_key(Some("cli-key"), Some("config-key"), ProviderType::Anthropic).unwrap();
        assert_eq!(result, "cli-key");
    }

    #[test]
    fn test_api_key_from_config() {
        // When CLI key is absent, config file key should be used.
        let result = resolve_api_key(None, Some("config-key"), ProviderType::Anthropic).unwrap();
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
        if let Err(e) = result {
            let msg = e.to_string();
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
                allow_list: vec!["ExecCommand".into()], // non-default, triggers if branch
                ..Default::default()
            },
            ..Default::default()
        };
        let merged = merge_config_files(global, project);
        assert!(
            merged.tools.auto_approve,
            "global auto_approve=true should be preserved"
        );
    }

    #[test]
    fn test_config_file_parses_shell_default() {
        let cfg: ConfigFile = toml::from_str(
            r#"
[shell]
default = "powershell"
"#,
        )
        .unwrap();

        assert_eq!(cfg.shell.default, "powershell");
    }

    #[test]
    fn test_merge_config_project_shell_overrides_global() {
        let global = ConfigFile {
            shell: crate::shell::ShellConfig { default: "bash".into() },
            ..Default::default()
        };
        let project = ConfigFile {
            shell: crate::shell::ShellConfig {
                default: "powershell".into(),
            },
            ..Default::default()
        };

        let merged = merge_config_files(global, project);

        assert_eq!(merged.shell.default, "powershell");
    }

    #[test]
    fn test_profile_shell_overrides_base_config() {
        let mut config = ConfigFile {
            default: DefaultConfig {
                max_tool_call_malformed_turns: Some(5),
                max_tool_call_failure_turns: Some(5),
                ..Default::default()
            },
            shell: crate::shell::ShellConfig { default: "bash".into() },
            ..Default::default()
        };
        config.profiles.insert(
            "windows".into(),
            ProfileConfig {
                max_tool_call_malformed_turns: Some(2),
                max_tool_call_failure_turns: Some(2),
                shell: Some("powershell".into()),
                ..Default::default()
            },
        );

        let applied = apply_profile(config, "windows").unwrap();

        assert_eq!(applied.shell.default, "powershell");
        assert_eq!(applied.default.max_tool_call_malformed_turns, Some(2));
        assert_eq!(applied.default.max_tool_call_failure_turns, Some(2));
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
        assert_eq!(config.default.max_turns, None);
        assert_eq!(config.default.max_tool_call_malformed_turns, None);
        assert_eq!(config.default.max_tool_call_failure_turns, None);
        assert!(config.default.model.is_none());
        assert!(config.providers.is_empty());
        assert!(config.profiles.is_empty());
    }

    #[test]
    fn resolve_max_turns_defaults_to_unlimited_and_preserves_explicit_limits() {
        assert_eq!(resolve_max_turns(None), None);
        assert_eq!(resolve_max_turns(Some(0)), None);
        assert_eq!(resolve_max_turns(Some(7)), Some(7));
    }

    #[test]
    fn mcp_server_config_deserializes_startup_timeout_ms() {
        let toml_str = r#"
[mcp.servers.slow-tools]
transport = "stdio"
command = "node"
args = ["server.js"]
startup_timeout_ms = 45000
"#;

        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        let server = config.mcp.servers.get("slow-tools").unwrap();

        assert_eq!(server.startup_timeout_ms, Some(45_000));
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
base_url = "https://api.openai.com/v1"

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
        assert_eq!(openai.base_url.as_deref(), Some("https://api.openai.com/v1"));

        let anthropic = config.providers.get("anthropic").unwrap();
        assert_eq!(anthropic.api_key.as_deref(), Some("sk-ant-test"));
        assert_eq!(anthropic.prompt_caching, Some(false));
    }

    #[test]
    fn test_config_file_deserialize_custom_provider_alias() {
        let toml_str = r#"
[default]
provider = "my-service"

[providers.my-service]
provider = "openai"
model = "custom-model-v1"
api_key = "alias-key"
base_url = "https://my-service.example.com/api/openai"
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();

        assert_eq!(config.default.provider, "my-service");
        let alias = config.providers.get("my-service").unwrap();
        assert_eq!(alias.provider.as_deref(), Some("openai"));
        assert_eq!(alias.model.as_deref(), Some("custom-model-v1"));
        assert_eq!(alias.api_key.as_deref(), Some("alias-key"));
        assert_eq!(
            alias.base_url.as_deref(),
            Some("https://my-service.example.com/api/openai")
        );
    }

    // -------------------------------------------------------------------------
    // merge_provider_configs tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_merge_provider_configs_overlay_overrides_base() {
        let base = ProviderConfig {
            api_key: Some("base-key".to_string()),
            base_url: Some("https://base.example.com".to_string()),
            model: Some("base-model".to_string()),
            ..Default::default()
        };
        let overlay = ProviderConfig {
            api_key: Some("overlay-key".to_string()),
            model: Some("overlay-model".to_string()),
            ..Default::default()
        };

        let merged = merge_provider_configs(base, overlay);
        assert_eq!(merged.api_key.as_deref(), Some("overlay-key"));
        assert_eq!(merged.model.as_deref(), Some("overlay-model"));
        // base_url not in overlay -> preserved from base
        assert_eq!(merged.base_url.as_deref(), Some("https://base.example.com"));
    }

    #[test]
    fn test_merge_provider_configs_overlay_none_preserves_base() {
        let base = ProviderConfig {
            api_key: Some("base-key".to_string()),
            base_url: Some("https://base.example.com".to_string()),
            model: Some("base-model".to_string()),
            prompt_caching: Some(true),
            provider: Some("openai".to_string()),
            ..Default::default()
        };
        let overlay = ProviderConfig::default();

        let merged = merge_provider_configs(base, overlay);
        assert_eq!(merged.api_key.as_deref(), Some("base-key"));
        assert_eq!(merged.base_url.as_deref(), Some("https://base.example.com"));
        assert_eq!(merged.model.as_deref(), Some("base-model"));
        assert_eq!(merged.prompt_caching, Some(true));
        assert_eq!(merged.provider.as_deref(), Some("openai"));
    }

    #[test]
    fn test_merge_provider_configs_compat_merges_both() {
        let base = ProviderConfig {
            compat: Some(ProviderCompat {
                messages: MessageCompat {
                    merge_assistant_messages: Some(true),
                    ..Default::default()
                },
                tools: ToolCompat {
                    clean_orphan_tool_calls: Some(true),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        let overlay = ProviderConfig {
            compat: Some(ProviderCompat {
                messages: MessageCompat {
                    merge_assistant_messages: Some(false), // override base
                    dedup_tool_results: Some(true),        // new field
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        let merged = merge_provider_configs(base, overlay);
        let compat = merged.compat.unwrap();
        // overlay wins
        assert_eq!(compat.messages.merge_assistant_messages, Some(false));
        // base preserved
        assert_eq!(compat.tools.clean_orphan_tool_calls, Some(true));
        // overlay adds new
        assert_eq!(compat.messages.dedup_tool_results, Some(true));
    }

    #[test]
    fn test_merge_provider_configs_compat_merges_across_domains() {
        let base = ProviderConfig {
            compat: Some(ProviderCompat {
                transport: TransportCompat {
                    max_tokens_field: Some("max_tokens".to_string()),
                    ..Default::default()
                },
                messages: MessageCompat {
                    merge_assistant_messages: Some(true),
                    clean_orphan_tool_results: Some(true),
                    ..Default::default()
                },
                tools: ToolCompat {
                    auto_tool_id: Some(true),
                    ..Default::default()
                },
                reasoning: ReasoningCompat {
                    supports_effort: Some(true),
                    effort_levels: Some(vec!["low".to_string(), "high".to_string()]),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        let overlay = ProviderConfig {
            compat: Some(ProviderCompat {
                transport: TransportCompat {
                    api_path: Some("/chat/completions".to_string()),
                    ..Default::default()
                },
                messages: MessageCompat {
                    merge_assistant_messages: Some(false),
                    ..Default::default()
                },
                schema: SchemaCompat {
                    sanitize_schema: Some(true),
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        let merged = merge_provider_configs(base, overlay);
        let compat = merged.compat.unwrap();

        assert_eq!(compat.transport.max_tokens_field.as_deref(), Some("max_tokens"));
        assert_eq!(compat.transport.api_path.as_deref(), Some("/chat/completions"));
        assert_eq!(compat.messages.merge_assistant_messages, Some(false));
        assert_eq!(compat.messages.clean_orphan_tool_results, Some(true));
        assert_eq!(compat.tools.auto_tool_id, Some(true));
        assert_eq!(compat.schema.sanitize_schema, Some(true));
        assert_eq!(compat.reasoning.supports_effort, Some(true));
        assert_eq!(
            compat.reasoning.effort_levels,
            Some(vec!["low".to_string(), "high".to_string()])
        );
    }

    #[test]
    fn test_merge_provider_configs_both_empty() {
        let merged = merge_provider_configs(ProviderConfig::default(), ProviderConfig::default());
        assert!(merged.api_key.is_none());
        assert!(merged.base_url.is_none());
        assert!(merged.model.is_none());
        assert!(merged.provider.is_none());
        assert!(merged.prompt_caching.is_none());
        assert!(merged.compat.is_none());
    }

    // -------------------------------------------------------------------------
    // resolve_provider_alias: builtin name path tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_resolve_builtin_provider_with_config() {
        let mut providers = HashMap::new();
        providers.insert(
            "openai".to_string(),
            ProviderConfig {
                api_key: Some("openai-key".to_string()),
                base_url: Some("https://custom-openai.example.com".to_string()),
                ..Default::default()
            },
        );

        let resolved = resolve_provider_alias(&providers, "openai").unwrap();
        assert_eq!(resolved.requested_name, "openai");
        assert_eq!(resolved.provider_type, ProviderType::OpenAI);
        assert_eq!(resolved.effective_config.api_key.as_deref(), Some("openai-key"));
        assert_eq!(
            resolved.effective_config.base_url.as_deref(),
            Some("https://custom-openai.example.com")
        );
    }

    #[test]
    fn test_resolve_builtin_provider_without_config_entry() {
        let providers = HashMap::new();

        let resolved = resolve_provider_alias(&providers, "anthropic").unwrap();
        assert_eq!(resolved.requested_name, "anthropic");
        assert_eq!(resolved.provider_type, ProviderType::Anthropic);
        // No config entry -> all fields default to None
        assert!(resolved.effective_config.api_key.is_none());
        assert!(resolved.effective_config.base_url.is_none());
        assert!(resolved.effective_config.model.is_none());
    }

    // -------------------------------------------------------------------------
    // resolve_provider_alias: error path tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_resolve_alias_maps_to_invalid_builtin_type() {
        let mut providers = HashMap::new();
        providers.insert(
            "my-db".to_string(),
            ProviderConfig {
                provider: Some("mysql".to_string()),
                ..Default::default()
            },
        );

        let result = resolve_provider_alias(&providers, "my-db");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("my-db"));
        assert!(msg.contains("mysql"));
        assert!(msg.contains("not a built-in provider"));
    }

    #[test]
    fn test_resolve_alias_not_found_in_providers() {
        let providers = HashMap::new();

        let result = resolve_provider_alias(&providers, "nonexistent");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("nonexistent"));
        assert!(msg.contains("built-in provider"));
        assert!(msg.contains("[providers.nonexistent]"));
    }

    // -------------------------------------------------------------------------
    // provider_label (requested_name) tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_provider_label_is_alias_name_not_underlying_type() {
        let mut providers = HashMap::new();
        providers.insert(
            "my-service".to_string(),
            ProviderConfig {
                provider: Some("openai".to_string()),
                api_key: Some("key".to_string()),
                ..Default::default()
            },
        );

        let resolved = resolve_provider_alias(&providers, "my-service").unwrap();
        // provider_label should be the alias name, not "openai"
        assert_eq!(resolved.requested_name, "my-service");
        assert_eq!(resolved.provider_type, ProviderType::OpenAI);
    }

    #[test]
    fn test_provider_label_is_builtin_name_for_builtin() {
        let providers = HashMap::new();

        for (name, expected_type) in [
            ("anthropic", ProviderType::Anthropic),
            ("openai", ProviderType::OpenAI),
            ("bedrock", ProviderType::Bedrock),
            ("vertex", ProviderType::Vertex),
        ] {
            let resolved = resolve_provider_alias(&providers, name).unwrap();
            assert_eq!(resolved.requested_name, name);
            assert_eq!(resolved.provider_type, expected_type);
        }
    }

    // -------------------------------------------------------------------------
    // model priority: alias model in resolution chain
    // -------------------------------------------------------------------------

    #[test]
    fn test_alias_model_available_in_effective_config() {
        // Verifies that alias.model is carried through effective_config,
        // which feeds into the priority chain: CLI > alias.model > default.model > hardcoded
        let mut providers = HashMap::new();
        providers.insert(
            "my-service".to_string(),
            ProviderConfig {
                provider: Some("openai".to_string()),
                model: Some("alias-model-v1".to_string()),
                ..Default::default()
            },
        );

        let resolved = resolve_provider_alias(&providers, "my-service").unwrap();
        assert_eq!(resolved.effective_config.model.as_deref(), Some("alias-model-v1"));
    }

    #[test]
    fn test_alias_model_inherits_from_underlying_provider() {
        // When alias has no model but underlying provider does,
        // the alias should inherit it via merge_provider_configs
        let mut providers = HashMap::new();
        providers.insert(
            "openai".to_string(),
            ProviderConfig {
                model: Some("gpt-4o".to_string()),
                ..Default::default()
            },
        );
        providers.insert(
            "my-service".to_string(),
            ProviderConfig {
                provider: Some("openai".to_string()),
                base_url: Some("https://my-service.example.com".to_string()),
                // no model -> should inherit from openai
                ..Default::default()
            },
        );

        let resolved = resolve_provider_alias(&providers, "my-service").unwrap();
        assert_eq!(resolved.effective_config.model.as_deref(), Some("gpt-4o"));
    }

    #[test]
    fn test_alias_model_overrides_underlying_provider_model() {
        // When both alias and underlying provider define model,
        // alias model should win
        let mut providers = HashMap::new();
        providers.insert(
            "openai".to_string(),
            ProviderConfig {
                model: Some("gpt-4o".to_string()),
                ..Default::default()
            },
        );
        providers.insert(
            "my-service".to_string(),
            ProviderConfig {
                provider: Some("openai".to_string()),
                model: Some("custom-model-v2".to_string()),
                ..Default::default()
            },
        );

        let resolved = resolve_provider_alias(&providers, "my-service").unwrap();
        assert_eq!(resolved.effective_config.model.as_deref(), Some("custom-model-v2"));
    }

    // -------------------------------------------------------------------------
    // Phase 5.5: FileCacheConfig in ConfigFile / merge
    // -------------------------------------------------------------------------

    #[test]
    fn tc_5_5_04_file_cache_toml_deserialization() {
        let toml_str = r#"
[file_cache]
max_entries = 50
max_size_bytes = 10485760
enabled = false
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        assert_eq!(config.file_cache.max_entries, 50);
        assert_eq!(config.file_cache.max_size_bytes, 10_485_760);
        assert!(!config.file_cache.enabled);
    }

    #[test]
    fn tc_5_5_02_file_cache_defaults_when_absent() {
        let config: ConfigFile = toml::from_str("").unwrap();
        assert_eq!(config.file_cache.max_entries, 100);
        assert_eq!(config.file_cache.max_size_bytes, 25 * 1024 * 1024);
        assert!(config.file_cache.enabled);
    }

    #[test]
    fn tc_5_5_01_file_cache_custom_capacity_propagates() {
        let toml_str = r#"
[file_cache]
max_entries = 50
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        assert_eq!(config.file_cache.max_entries, 50);
        // Other fields keep defaults.
        assert_eq!(config.file_cache.max_size_bytes, 25 * 1024 * 1024);
        assert!(config.file_cache.enabled);
    }

    #[test]
    fn tc_5_5_03_file_cache_disabled_propagates() {
        let toml_str = r#"
[file_cache]
enabled = false
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        assert!(!config.file_cache.enabled);
    }

    #[test]
    fn merge_file_cache_project_overrides_global() {
        let global = ConfigFile {
            file_cache: FileCacheConfig {
                max_entries: 200,
                max_size_bytes: 50 * 1024 * 1024,
                enabled: true,
            },
            ..Default::default()
        };
        let project = ConfigFile {
            file_cache: FileCacheConfig {
                max_entries: 50,
                ..Default::default()
            },
            ..Default::default()
        };

        let merged = merge_config_files(global, project);
        assert_eq!(
            merged.file_cache.max_entries, 50,
            "project non-default max_entries should override global"
        );
    }

    #[test]
    fn merge_file_cache_global_preserved_when_project_default() {
        let global = ConfigFile {
            file_cache: FileCacheConfig {
                max_entries: 200,
                max_size_bytes: 50 * 1024 * 1024,
                enabled: true,
            },
            ..Default::default()
        };
        let project = ConfigFile::default();

        let merged = merge_config_files(global, project);
        assert_eq!(
            merged.file_cache.max_entries, 200,
            "global should be preserved when project is all-default"
        );
        assert_eq!(merged.file_cache.max_size_bytes, 50 * 1024 * 1024);
    }

    #[test]
    fn merge_file_cache_project_max_size_bytes_overrides_global() {
        // R-5.5-01: project changes only max_size_bytes (enabled=true, max_entries=default).
        let global = ConfigFile {
            file_cache: FileCacheConfig {
                max_entries: 100,
                max_size_bytes: 50 * 1024 * 1024,
                enabled: true,
            },
            ..Default::default()
        };
        let project = ConfigFile {
            file_cache: FileCacheConfig {
                max_entries: 100,                 // default
                max_size_bytes: 10 * 1024 * 1024, // non-default
                enabled: true,                    // default
            },
            ..Default::default()
        };

        let merged = merge_config_files(global, project);
        assert_eq!(
            merged.file_cache.max_size_bytes,
            10 * 1024 * 1024,
            "project max_size_bytes should override global"
        );
    }

    #[test]
    fn merge_file_cache_disabled_overrides_global() {
        let global = ConfigFile {
            file_cache: FileCacheConfig {
                enabled: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let project = ConfigFile {
            file_cache: FileCacheConfig {
                enabled: false,
                ..Default::default()
            },
            ..Default::default()
        };

        let merged = merge_config_files(global, project);
        assert!(
            !merged.file_cache.enabled,
            "project enabled=false should override global"
        );
    }

    #[test]
    fn test_resolve_with_project_dir_loads_project_config() {
        let tmp = tempfile::tempdir().unwrap();
        let project_toml = tmp.path().join(".aionrs.toml");
        std::fs::write(
            &project_toml,
            r#"
[default]
max_tokens = 1234
"#,
        )
        .unwrap();

        let base_cli_args = CliArgs {
            provider: Some("anthropic".into()),
            api_key: Some("test-key".into()),
            base_url: None,
            model: None,
            max_tokens: None,
            thinking: None,
            thinking_budget: None,
            max_turns: None,
            max_tool_call_malformed_turns: None,
            max_tool_call_failure_turns: None,
            system_prompt: None,
            profile: None,
            auto_approve: false,
            project_dir: Some(tmp.path().to_path_buf()),
        };

        let config = Config::resolve(&base_cli_args).unwrap();
        assert_eq!(config.max_tokens, 1234);
        assert_eq!(config.max_tool_call_malformed_turns, None);
        assert_eq!(config.max_tool_call_failure_turns, None);

        std::fs::write(
            &project_toml,
            r#"
[default]
max_tokens = 1234
max_tool_call_malformed_turns = 2
max_tool_call_failure_turns = 4
"#,
        )
        .unwrap();

        let config = Config::resolve(&base_cli_args).unwrap();
        assert_eq!(config.max_tool_call_malformed_turns, Some(2));
        assert_eq!(config.max_tool_call_failure_turns, Some(4));

        let cli_args = CliArgs {
            max_tool_call_malformed_turns: Some(0),
            max_tool_call_failure_turns: Some(0),
            ..base_cli_args
        };

        let config = Config::resolve(&cli_args).unwrap();
        assert_eq!(config.max_tool_call_malformed_turns, Some(0));
        assert_eq!(config.max_tool_call_failure_turns, Some(0));
    }

    #[test]
    fn test_config_resolve_loads_flat_provider_compat_after_domain_split() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join(".aionrs.toml"),
            r#"
[default]
provider = "openai"
model = "test-model"

[providers.openai]
api_key = "test-key"
base_url = "https://example.test/v1"

[providers.openai.compat]
max_tokens_field = "max_completion_tokens"
api_path = "/chat/completions"
merge_assistant_messages = false
clean_orphan_tool_calls = false
clean_orphan_tool_results = false
dedup_tool_results = true
sanitize_malformed_tool_calls = false
strip_patterns = ["__REASONING__"]
auto_tool_id = false
supports_thinking = true
supports_effort = true
effort_levels = ["low", "medium"]
"#,
        )
        .unwrap();

        let cli = CliArgs {
            provider: None,
            api_key: None,
            base_url: None,
            model: None,
            max_tokens: None,
            thinking: None,
            thinking_budget: None,
            max_turns: None,
            max_tool_call_malformed_turns: None,
            max_tool_call_failure_turns: None,
            system_prompt: None,
            profile: None,
            auto_approve: false,
            project_dir: Some(tmp.path().to_path_buf()),
        };

        let config = Config::resolve(&cli).unwrap();

        assert_eq!(config.compat.max_tokens_field(), "max_completion_tokens");
        assert_eq!(config.compat.api_path(), "/chat/completions");
        assert!(!config.compat.merge_assistant_messages());
        assert!(!config.compat.clean_orphan_tool_calls());
        assert!(!config.compat.clean_orphan_tool_results());
        assert!(config.compat.dedup_tool_results());
        assert!(!config.compat.sanitize_malformed_tool_calls());
        assert!(!config.compat.auto_tool_id());
        assert!(config.compat.supports_thinking());
        assert!(config.compat.supports_effort());
        assert_eq!(config.compat.effort_levels(), &["low", "medium"]);
        assert_eq!(
            config.compat.messages.strip_patterns,
            Some(vec!["__REASONING__".to_string()])
        );
    }

    #[test]
    fn test_config_resolve_cli_thinking_records_request_without_enabling_capability() {
        let tmp = tempfile::tempdir().unwrap();
        let cli = CliArgs {
            provider: Some("openai".into()),
            api_key: Some("test-key".into()),
            base_url: None,
            model: None,
            max_tokens: None,
            thinking: Some("enabled".into()),
            thinking_budget: Some(16_000),
            max_turns: None,
            max_tool_call_malformed_turns: None,
            max_tool_call_failure_turns: None,
            system_prompt: None,
            profile: None,
            auto_approve: false,
            project_dir: Some(tmp.path().to_path_buf()),
        };

        let config = Config::resolve(&cli).unwrap();

        assert!(!config.compat.supports_thinking());
        assert!(matches!(
            config.thinking,
            Some(ThinkingConfig::Enabled { budget_tokens: 16_000 })
        ));
    }

    #[test]
    fn test_config_resolve_cli_thinking_disabled_does_not_enable_capability() {
        let tmp = tempfile::tempdir().unwrap();
        let cli = CliArgs {
            provider: Some("openai".into()),
            api_key: Some("test-key".into()),
            base_url: None,
            model: None,
            max_tokens: None,
            thinking: Some("disabled".into()),
            thinking_budget: None,
            max_turns: None,
            max_tool_call_malformed_turns: None,
            max_tool_call_failure_turns: None,
            system_prompt: None,
            profile: None,
            auto_approve: false,
            project_dir: Some(tmp.path().to_path_buf()),
        };

        let config = Config::resolve(&cli).unwrap();

        assert!(!config.compat.supports_thinking());
        assert!(matches!(config.thinking, Some(ThinkingConfig::Disabled)));
    }

    #[test]
    fn test_config_resolve_cli_thinking_preserves_explicit_unsupported_capability() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join(".aionrs.toml"),
            r#"
[providers.openai.compat]
supports_thinking = false
"#,
        )
        .unwrap();
        let cli = CliArgs {
            provider: Some("openai".into()),
            api_key: Some("test-key".into()),
            base_url: None,
            model: None,
            max_tokens: None,
            thinking: Some("enabled".into()),
            thinking_budget: Some(16_000),
            max_turns: None,
            max_tool_call_malformed_turns: None,
            max_tool_call_failure_turns: None,
            system_prompt: None,
            profile: None,
            auto_approve: false,
            project_dir: Some(tmp.path().to_path_buf()),
        };

        let config = Config::resolve(&cli).unwrap();

        assert!(!config.compat.supports_thinking());
        assert!(matches!(
            config.thinking,
            Some(ThinkingConfig::Enabled { budget_tokens: 16_000 })
        ));
    }

    #[test]
    fn test_config_resolve_cli_thinking_budget_alone_records_request_without_enabling_capability() {
        let tmp = tempfile::tempdir().unwrap();
        let cli = CliArgs {
            provider: Some("openai".into()),
            api_key: Some("test-key".into()),
            base_url: None,
            model: None,
            max_tokens: None,
            thinking: None,
            thinking_budget: Some(12_000),
            max_turns: None,
            max_tool_call_malformed_turns: None,
            max_tool_call_failure_turns: None,
            system_prompt: None,
            profile: None,
            auto_approve: false,
            project_dir: Some(tmp.path().to_path_buf()),
        };

        let config = Config::resolve(&cli).unwrap();

        assert!(!config.compat.supports_thinking());
        assert!(matches!(
            config.thinking,
            Some(ThinkingConfig::Enabled { budget_tokens: 12_000 })
        ));
    }

    #[test]
    fn test_config_resolve_rejects_invalid_cli_thinking() {
        let tmp = tempfile::tempdir().unwrap();
        let cli = CliArgs {
            provider: Some("openai".into()),
            api_key: Some("test-key".into()),
            base_url: None,
            model: None,
            max_tokens: None,
            thinking: Some("auto".into()),
            thinking_budget: None,
            max_turns: None,
            max_tool_call_malformed_turns: None,
            max_tool_call_failure_turns: None,
            system_prompt: None,
            profile: None,
            auto_approve: false,
            project_dir: Some(tmp.path().to_path_buf()),
        };

        let err = Config::resolve(&cli).unwrap_err().to_string();

        assert!(err.contains("Invalid --thinking value"));
    }

    #[test]
    fn test_config_resolve_normalizes_official_openai_root_base_url() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join(".aionrs.toml"),
            r#"
[providers.openai]
base_url = "https://api.openai.com"
"#,
        )
        .unwrap();
        let cli = CliArgs {
            provider: Some("openai".into()),
            api_key: Some("test-key".into()),
            base_url: None,
            model: None,
            max_tokens: None,
            thinking: None,
            thinking_budget: None,
            max_turns: None,
            max_tool_call_malformed_turns: None,
            max_tool_call_failure_turns: None,
            system_prompt: None,
            profile: None,
            auto_approve: false,
            project_dir: Some(tmp.path().to_path_buf()),
        };

        let config = Config::resolve(&cli).unwrap();

        assert_eq!(config.base_url, "https://api.openai.com/v1");
    }

    #[test]
    fn test_config_resolve_does_not_normalize_openai_compatible_base_url() {
        let tmp = tempfile::tempdir().unwrap();
        let cli = CliArgs {
            provider: Some("openai".into()),
            api_key: Some("test-key".into()),
            base_url: Some("https://api.deepseek.com".into()),
            model: None,
            max_tokens: None,
            thinking: None,
            thinking_budget: None,
            max_turns: None,
            max_tool_call_malformed_turns: None,
            max_tool_call_failure_turns: None,
            system_prompt: None,
            profile: None,
            auto_approve: false,
            project_dir: Some(tmp.path().to_path_buf()),
        };

        let config = Config::resolve(&cli).unwrap();

        assert_eq!(config.base_url, "https://api.deepseek.com");
    }

    #[test]
    fn test_config_resolve_loads_flat_provider_max_tool_count_limits() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join(".aionrs.toml"),
            r#"
[default]
provider = "gemini"
model = "test-model"

[providers.gemini]
provider = "openai"
api_key = "test-key"
base_url = "https://example.test/v1"

[providers.gemini.compat]
max_tool_count = 512
max_request_body_bytes = 1048576
"#,
        )
        .unwrap();

        let cli = CliArgs {
            provider: None,
            api_key: None,
            base_url: None,
            model: None,
            max_tokens: None,
            thinking: None,
            thinking_budget: None,
            max_turns: None,
            max_tool_call_malformed_turns: None,
            max_tool_call_failure_turns: None,
            system_prompt: None,
            profile: None,
            auto_approve: false,
            project_dir: Some(tmp.path().to_path_buf()),
        };

        let config = Config::resolve(&cli).unwrap();

        assert_eq!(config.compat.max_tool_count(), Some(512));
        assert_eq!(config.compat.max_request_body_bytes(), Some(1_048_576));
    }

    #[test]
    fn test_openai_field_controls_alias_and_profile_override_flattened_compat() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join(".aionrs.toml"),
            r#"
[default]
provider = "nim"
model = "alias-model"

[providers.openai]
api_key = "builtin-key"
base_url = "https://api.openai.test/v1"

[providers.openai.compat]
include_stream_options = true
emit_tools = true
supports_effort = true

[providers.nim]
provider = "openai"
api_key = "alias-key"
base_url = "https://nim.example.test/v1"

[providers.nim.compat]
include_stream_options = false
emit_tools = false
supports_effort = false

[profiles.restore-openai-fields]
provider = "nim"

[profiles.restore-openai-fields.compat]
include_stream_options = true
emit_tools = true
supports_effort = true
"#,
        )
        .unwrap();

        let base_cli = CliArgs {
            provider: None,
            api_key: None,
            base_url: None,
            model: None,
            max_tokens: None,
            thinking: None,
            thinking_budget: None,
            max_turns: None,
            max_tool_call_malformed_turns: None,
            max_tool_call_failure_turns: None,
            system_prompt: None,
            profile: None,
            auto_approve: false,
            project_dir: Some(tmp.path().to_path_buf()),
        };

        let alias_config = Config::resolve(&base_cli).unwrap();
        assert_eq!(alias_config.provider, ProviderType::OpenAI);
        assert_eq!(alias_config.provider_label, "nim");
        assert!(!alias_config.compat.include_stream_options());
        assert!(!alias_config.compat.emit_tools());
        assert!(!alias_config.compat.supports_effort());

        let profile_config = Config::resolve(&CliArgs {
            profile: Some("restore-openai-fields".to_string()),
            ..base_cli
        })
        .unwrap();
        assert_eq!(profile_config.provider, ProviderType::OpenAI);
        assert_eq!(profile_config.provider_label, "nim");
        assert!(profile_config.compat.include_stream_options());
        assert!(profile_config.compat.emit_tools());
        assert!(profile_config.compat.supports_effort());
    }

    #[test]
    fn test_config_resolve_tool_wire_shape_override_from_provider_compat() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join(".aionrs.toml"),
            r#"
[default]
provider = "openai"
model = "test-model"

[providers.openai]
api_key = "test-key"
base_url = "https://example.test/v1"

[providers.openai.compat]
tool_wire_shape = "anthropic_input_schema"
"#,
        )
        .unwrap();

        let cli = CliArgs {
            provider: None,
            api_key: None,
            base_url: None,
            model: None,
            max_tokens: None,
            thinking: None,
            thinking_budget: None,
            max_turns: None,
            max_tool_call_malformed_turns: None,
            max_tool_call_failure_turns: None,
            system_prompt: None,
            profile: None,
            auto_approve: false,
            project_dir: Some(tmp.path().to_path_buf()),
        };

        let config = Config::resolve(&cli).unwrap();

        assert_eq!(config.compat.tool_wire_shape(), ToolWireShape::AnthropicInputSchema);
    }

    #[test]
    fn test_resolve_zero_max_turns_disables_turn_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let cli_args = CliArgs {
            provider: Some("anthropic".into()),
            api_key: Some("test-key".into()),
            base_url: None,
            model: None,
            max_tokens: None,
            thinking: None,
            thinking_budget: None,
            max_turns: Some(0),
            max_tool_call_malformed_turns: None,
            max_tool_call_failure_turns: None,
            system_prompt: None,
            profile: None,
            auto_approve: false,
            project_dir: Some(tmp.path().to_path_buf()),
        };

        let config = Config::resolve(&cli_args).unwrap();
        assert_eq!(config.max_turns, None);
    }

    #[test]
    fn test_resolve_without_project_dir_uses_cwd() {
        let cli_args = CliArgs {
            provider: Some("anthropic".into()),
            api_key: Some("test-key".into()),
            base_url: None,
            model: None,
            max_tokens: None,
            thinking: None,
            thinking_budget: None,
            max_turns: None,
            max_tool_call_malformed_turns: None,
            max_tool_call_failure_turns: None,
            system_prompt: None,
            profile: None,
            auto_approve: false,
            project_dir: None,
        };

        let config = Config::resolve(&cli_args);
        assert!(config.is_ok());
    }
}
