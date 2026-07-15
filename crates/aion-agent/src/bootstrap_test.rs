use super::*;

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use aion_config::config::{CliArgs, McpServerConfig, TransportType};
    use aion_protocol::events::ToolCategory;
    use aion_tools::Tool;
    use aion_types::tool::ToolResult;
    use async_trait::async_trait;
    use serde_json::{Value, json};

    use crate::output::OutputSink;
    use crate::output::null_sink::NullSink;
    use crate::tool_policy::ToolPolicy;

    use super::*;

    struct DeferredTestTool(&'static str);

    #[async_trait]
    impl Tool for DeferredTestTool {
        fn name(&self) -> &str {
            self.0
        }

        fn description(&self) -> &str {
            "deferred test tool"
        }

        fn input_schema(&self) -> Value {
            json!({"type": "object"})
        }

        fn is_concurrency_safe(&self, _input: &Value) -> bool {
            true
        }

        async fn execute(&self, _input: Value) -> ToolResult {
            ToolResult {
                content: "unused".to_string(),
                is_error: false,
            }
        }

        fn category(&self) -> ToolCategory {
            ToolCategory::Info
        }

        fn is_deferred(&self) -> bool {
            true
        }
    }

    fn test_config() -> Config {
        Config::resolve(&CliArgs {
            provider: Some("anthropic".to_string()),
            api_key: Some("sk-test".to_string()),
            base_url: None,
            model: Some("claude-sonnet-4-20250514".to_string()),
            max_tokens: Some(4096),
            thinking: None,
            thinking_budget: None,
            max_turns: None,
            max_tool_call_malformed_turns: None,
            max_tool_call_failure_turns: None,
            system_prompt: None,
            profile: None,
            auto_approve: false,
            project_dir: None,
        })
        .unwrap()
    }

    #[test]
    fn mcp_servers_with_runtime_env_uses_server_env_as_override() {
        let mut config = test_config();
        config.mcp.servers.insert(
            "stdio".to_string(),
            McpServerConfig {
                transport: TransportType::Stdio,
                command: Some("server".to_string()),
                args: None,
                env: Some(HashMap::from([
                    ("OVERRIDE".to_string(), "server".to_string()),
                    ("SERVER_ONLY".to_string(), "1".to_string()),
                ])),
                url: None,
                headers: None,
                deferred: None,
                startup_timeout_ms: None,
            },
        );

        let output: Arc<dyn OutputSink> = Arc::new(NullSink);
        let bootstrap = AgentBootstrap::new(config, "/tmp", output).runtime_env(vec![
            ("OVERRIDE".to_string(), "runtime".to_string()),
            ("RUNTIME_ONLY".to_string(), "1".to_string()),
        ]);

        let servers = bootstrap.mcp_servers_with_runtime_env();
        let env = servers
            .get("stdio")
            .and_then(|server| server.env.as_ref())
            .expect("stdio server env should exist");

        assert_eq!(env.get("OVERRIDE").map(String::as_str), Some("server"));
        assert_eq!(env.get("SERVER_ONLY").map(String::as_str), Some("1"));
        assert_eq!(env.get("RUNTIME_ONLY").map(String::as_str), Some("1"));
    }

    #[tokio::test]
    async fn tool_search_snapshot_excludes_policy_denied_tools() {
        let output: Arc<dyn OutputSink> = Arc::new(NullSink);
        let bootstrap = AgentBootstrap::new(test_config(), "/tmp", output)
            .tool_policy(ToolPolicy::allow_only(["ToolSearch", "AllowedDeferred"]));
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(DeferredTestTool("AllowedDeferred")));
        registry.register(Box::new(DeferredTestTool("DeniedDeferred")));

        bootstrap.register_tool_search(&mut registry);

        let tool_search = registry.get("ToolSearch").expect("ToolSearch should be registered");
        let allowed = tool_search.execute(json!({"query": "AllowedDeferred"})).await;
        let denied = tool_search.execute(json!({"query": "DeniedDeferred"})).await;

        assert!(allowed.content.contains("AllowedDeferred"));
        assert!(denied.content.starts_with("No deferred tools matching"));
        assert!(!denied.content.contains("\"name\": \"DeniedDeferred\""));
    }
}
