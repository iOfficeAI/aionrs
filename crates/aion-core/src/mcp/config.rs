use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// MCP configuration section in config file
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct McpConfig {
    #[serde(default)]
    pub servers: HashMap<String, McpServerConfig>,
}

/// Configuration for a single MCP server
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpServerConfig {
    pub transport: TransportType,

    // stdio transport fields
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub env: Option<HashMap<String, String>>,

    // SSE / Streamable HTTP transport fields
    pub url: Option<String>,
    pub headers: Option<HashMap<String, String>>,
}

/// Supported MCP transport types
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TransportType {
    Stdio,
    Sse,
    StreamableHttp,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_config_deserialize() {
        // Deserialize a McpConfig containing one stdio server from TOML
        let toml_str = r#"
[servers.test]
transport = "stdio"
command = "echo"
args = ["hello"]
"#;
        let config: McpConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.servers.len(), 1);

        let server = config.servers.get("test").expect("server 'test' should exist");
        assert_eq!(server.transport, TransportType::Stdio);
        assert_eq!(server.command.as_deref(), Some("echo"));
        assert_eq!(
            server.args.as_deref(),
            Some(["hello".to_string()].as_slice())
        );
        assert!(server.env.is_none());
        assert!(server.url.is_none());
    }

    #[test]
    fn test_transport_type_variants() {
        // Verify serde deserialization of all TransportType variants
        #[derive(Deserialize)]
        struct Wrapper {
            transport: TransportType,
        }

        let stdio: Wrapper = toml::from_str(r#"transport = "stdio""#).unwrap();
        assert_eq!(stdio.transport, TransportType::Stdio);

        let sse: Wrapper = toml::from_str(r#"transport = "sse""#).unwrap();
        assert_eq!(sse.transport, TransportType::Sse);

        let http: Wrapper = toml::from_str(r#"transport = "streamable-http""#).unwrap();
        assert_eq!(http.transport, TransportType::StreamableHttp);
    }
}
