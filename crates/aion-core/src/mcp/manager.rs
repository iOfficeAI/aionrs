use std::collections::HashMap;

use serde_json::json;

use super::config::{McpServerConfig, TransportType};
use super::protocol::{
    ClientCapabilities, ClientInfo, InitializeParams, InitializeResult, JsonRpcRequest,
    McpToolDef, McpToolResult, ToolsListResult,
};
use super::transport::stdio::StdioTransport;
use super::transport::sse::SseTransport;
use super::transport::streamable_http::StreamableHttpTransport;
use super::transport::{McpError, McpTransport};

/// A connected MCP server with its discovered tools
struct McpServer {
    #[allow(dead_code)]
    name: String,
    transport: Box<dyn McpTransport>,
    tools: Vec<McpToolDef>,
}

/// Manages connections to multiple MCP servers
pub struct McpManager {
    servers: HashMap<String, McpServer>,
}

impl McpManager {
    /// Connect to all configured MCP servers
    pub async fn connect_all(
        configs: &HashMap<String, McpServerConfig>,
    ) -> Result<Self, McpError> {
        let mut servers = HashMap::new();

        for (name, config) in configs {
            match Self::connect_server(name, config).await {
                Ok(server) => {
                    eprintln!(
                        "[mcp] Connected to '{}': {} tools",
                        name,
                        server.tools.len()
                    );
                    servers.insert(name.clone(), server);
                }
                Err(e) => {
                    // Non-fatal: continue with other servers
                    eprintln!("[mcp] Failed to connect to '{}': {}", name, e);
                }
            }
        }

        Ok(Self { servers })
    }

    /// Connect to a single MCP server: create transport, initialize, discover tools
    async fn connect_server(
        name: &str,
        config: &McpServerConfig,
    ) -> Result<McpServer, McpError> {
        let empty_map = HashMap::new();

        // 1. Create transport
        let transport: Box<dyn McpTransport> = match config.transport {
            TransportType::Stdio => {
                let command = config
                    .command
                    .as_deref()
                    .ok_or_else(|| McpError::InitFailed("stdio transport requires 'command'".into()))?;
                let args = config.args.as_deref().unwrap_or(&[]);
                let env = config.env.as_ref().unwrap_or(&empty_map);
                Box::new(StdioTransport::spawn(command, args, env).await?)
            }
            TransportType::Sse => {
                let url = config
                    .url
                    .as_deref()
                    .ok_or_else(|| McpError::InitFailed("SSE transport requires 'url'".into()))?;
                let headers = config.headers.as_ref().unwrap_or(&empty_map);
                Box::new(SseTransport::connect(url, headers).await?)
            }
            TransportType::StreamableHttp => {
                let url = config
                    .url
                    .as_deref()
                    .ok_or_else(|| McpError::InitFailed("streamable-http transport requires 'url'".into()))?;
                let headers = config.headers.as_ref().unwrap_or(&empty_map);
                Box::new(StreamableHttpTransport::connect(url, headers).await?)
            }
        };

        // 2. Initialize handshake
        let init_params = InitializeParams {
            protocol_version: "2025-03-26".to_string(),
            capabilities: ClientCapabilities {
                tools: Some(json!({})),
            },
            client_info: ClientInfo {
                name: "aionrs".to_string(),
                version: "0.3.0".to_string(),
            },
        };

        let init_req = JsonRpcRequest::new(
            1,
            "initialize",
            Some(serde_json::to_value(&init_params).map_err(|e| {
                McpError::InitFailed(format!("Failed to serialize init params: {}", e))
            })?),
        );

        let init_response = transport.request(&init_req).await?;
        let _init_result: InitializeResult = serde_json::from_value(
            init_response
                .result
                .ok_or_else(|| McpError::InitFailed("No result in initialize response".into()))?,
        )
        .map_err(|e| McpError::InitFailed(format!("Failed to parse init result: {}", e)))?;

        // 3. Send initialized notification
        let initialized_notification =
            JsonRpcRequest::notification("notifications/initialized", None);
        transport.notify(&initialized_notification).await?;

        // 4. List tools
        let list_req = JsonRpcRequest::new(2, "tools/list", None);
        let list_response = transport.request(&list_req).await?;
        let tools_result: ToolsListResult = serde_json::from_value(
            list_response
                .result
                .ok_or_else(|| McpError::InitFailed("No result in tools/list response".into()))?,
        )
        .map_err(|e| McpError::InitFailed(format!("Failed to parse tools list: {}", e)))?;

        Ok(McpServer {
            name: name.to_string(),
            transport,
            tools: tools_result.tools,
        })
    }

    /// Get all discovered tools with their server names
    pub fn all_tools(&self) -> Vec<(&str, &McpToolDef)> {
        let mut result = Vec::new();
        for (server_name, server) in &self.servers {
            for tool in &server.tools {
                result.push((server_name.as_str(), tool));
            }
        }
        result
    }

    /// Check if a tool name exists across any server
    pub fn has_tool_name(&self, name: &str) -> bool {
        self.servers
            .values()
            .any(|s| s.tools.iter().any(|t| t.name == name))
    }

    /// Count how many servers have a tool with the given name
    pub fn tool_name_count(&self, name: &str) -> usize {
        self.servers
            .values()
            .filter(|s| s.tools.iter().any(|t| t.name == name))
            .count()
    }

    /// Execute a tool on a specific server
    pub async fn call_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<String, McpError> {
        let server = self
            .servers
            .get(server_name)
            .ok_or_else(|| McpError::ServerNotFound(server_name.to_string()))?;

        let request = JsonRpcRequest::new(
            0, // id doesn't matter for stdio, will be used for SSE/HTTP
            "tools/call",
            Some(json!({
                "name": tool_name,
                "arguments": arguments
            })),
        );

        let response = server.transport.request(&request).await?;

        let result_value = response
            .result
            .ok_or_else(|| McpError::Transport("No result in tool call response".into()))?;

        // Parse result and concatenate text content
        let tool_result: McpToolResult = serde_json::from_value(result_value)
            .map_err(|e| McpError::Transport(format!("Failed to parse tool result: {}", e)))?;

        let mut text_parts = Vec::new();
        for content in &tool_result.content {
            match content {
                super::protocol::McpContent::Text { text } => text_parts.push(text.clone()),
                super::protocol::McpContent::Image { mime_type, .. } => {
                    text_parts.push(format!("[image: {}]", mime_type));
                }
                super::protocol::McpContent::Resource { .. } => {
                    text_parts.push("[resource]".to_string());
                }
            }
        }

        Ok(text_parts.join("\n"))
    }

    /// Gracefully shutdown all servers
    pub async fn shutdown(&self) {
        for (name, server) in &self.servers {
            if let Err(e) = server.transport.close().await {
                eprintln!("[mcp] Error closing '{}': {}", name, e);
            }
        }
    }
}
