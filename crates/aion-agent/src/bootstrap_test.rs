use super::*;

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use aion_config::config::{CliArgs, McpServerConfig, TransportType};

    use crate::output::OutputSink;
    use crate::output::null_sink::NullSink;

    use super::*;

    #[test]
    fn mcp_servers_with_runtime_env_uses_server_env_as_override() {
        let mut config = Config::resolve(&CliArgs {
            provider: Some("anthropic".to_string()),
            api_key: Some("sk-test".to_string()),
            base_url: None,
            model: Some("claude-sonnet-4-20250514".to_string()),
            max_tokens: Some(4096),
            thinking: None,
            max_turns: None,
            max_tool_call_malformed_turns: None,
            max_tool_call_failure_turns: None,
            system_prompt: None,
            profile: None,
            auto_approve: false,
            project_dir: None,
        })
        .unwrap();
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
}
