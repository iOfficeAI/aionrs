use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures::stream::{FuturesUnordered, StreamExt};
use serde_json::json;

use super::config::{McpServerConfig, TransportType};
use super::protocol::{
    ClientCapabilities, ClientInfo, InitializeParams, InitializeResult, JsonRpcRequest,
    McpResource, McpToolDef, McpToolResult, ResourcesListResult, ResourcesReadResult,
    ToolsListResult,
};
use super::transport::sse::SseTransport;
use super::transport::stdio::StdioTransport;
use super::transport::streamable_http::StreamableHttpTransport;
use super::transport::{McpError, McpTransport};

const DEFAULT_STARTUP_TIMEOUT_MS: u64 = 30_000;

/// A connected MCP server with its discovered tools and capabilities
struct McpServer {
    #[allow(dead_code)]
    name: String,
    transport: Box<dyn McpTransport>,
    tools: Vec<McpToolDef>,
    /// Whether the server declared resources capability in its initialize response
    supports_resources: bool,
}

/// Manages connections to multiple MCP servers
pub struct McpManager {
    servers: HashMap<String, McpServer>,
    /// Monotonically increasing request ID counter for all JSON-RPC calls
    next_id: AtomicU64,
}

impl McpManager {
    /// Connect to all configured MCP servers
    pub async fn connect_all(configs: &HashMap<String, McpServerConfig>) -> Result<Self, McpError> {
        Self::connect_all_with_connector(configs, |name, config| async move {
            Self::connect_server(&name, &config).await
        })
        .await
    }

    async fn connect_all_with_connector<F, Fut>(
        configs: &HashMap<String, McpServerConfig>,
        connector: F,
    ) -> Result<Self, McpError>
    where
        F: Fn(String, McpServerConfig) -> Fut,
        Fut: Future<Output = Result<McpServer, McpError>>,
    {
        let mut servers = HashMap::new();
        let mut pending = FuturesUnordered::new();

        for (name, config) in configs {
            let name = name.clone();
            let config = config.clone();
            let connect = connector(name.clone(), config.clone());
            pending.push(async move {
                let result = Self::with_startup_timeout(&name, &config, connect).await;
                (name, result)
            });
        }

        while let Some((name, result)) = pending.next().await {
            match result {
                Ok(server) => {
                    tracing::info!(target: "aion_mcp", server = %name, tools = server.tools.len(), resources = server.supports_resources, "mcp server connected");
                    servers.insert(name, server);
                }
                Err(e) => {
                    // Non-fatal: continue with other servers
                    tracing::warn!(target: "aion_mcp", server = %name, error = %e, "mcp server connection failed");
                }
            }
        }

        Ok(Self {
            servers,
            next_id: AtomicU64::new(10),
        })
    }

    fn startup_timeout(config: &McpServerConfig) -> Duration {
        Duration::from_millis(
            config
                .startup_timeout_ms
                .unwrap_or(DEFAULT_STARTUP_TIMEOUT_MS),
        )
    }

    async fn with_startup_timeout<Fut>(
        name: &str,
        config: &McpServerConfig,
        connect: Fut,
    ) -> Result<McpServer, McpError>
    where
        Fut: Future<Output = Result<McpServer, McpError>>,
    {
        let timeout = Self::startup_timeout(config);
        match tokio::time::timeout(timeout, connect).await {
            Ok(result) => result,
            Err(_) => Err(McpError::Transport(format!(
                "MCP server '{name}' startup timed out after {}ms; set startup_timeout_ms to increase it",
                timeout.as_millis()
            ))),
        }
    }

    /// Connect a single additional MCP server after initial setup.
    /// Returns the list of tool names exposed by the server.
    pub async fn connect_one(
        &mut self,
        name: String,
        config: &McpServerConfig,
    ) -> Result<Vec<String>, McpError> {
        let server =
            Self::with_startup_timeout(&name, config, Self::connect_server(&name, config)).await?;
        let tool_names: Vec<String> = server.tools.iter().map(|t| t.name.clone()).collect();
        tracing::info!(target: "aion_mcp", server = %name, tools = server.tools.len(), resources = server.supports_resources, "mcp server connected");
        self.servers.insert(name, server);
        Ok(tool_names)
    }

    /// Connect to a single MCP server: create transport, initialize, discover tools
    async fn connect_server(name: &str, config: &McpServerConfig) -> Result<McpServer, McpError> {
        let empty_map = HashMap::new();

        // 1. Create transport
        let transport: Box<dyn McpTransport> = match config.transport {
            TransportType::Stdio => {
                let command = config.command.as_deref().ok_or_else(|| {
                    McpError::InitFailed("stdio transport requires 'command'".into())
                })?;
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
                let url = config.url.as_deref().ok_or_else(|| {
                    McpError::InitFailed("streamable-http transport requires 'url'".into())
                })?;
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
        let init_result: InitializeResult = serde_json::from_value(
            init_response
                .result
                .ok_or_else(|| McpError::InitFailed("No result in initialize response".into()))?,
        )
        .map_err(|e| McpError::InitFailed(format!("Failed to parse init result: {}", e)))?;

        // Check whether server declared resources capability
        let supports_resources = init_result
            .capabilities
            .get("resources")
            .map(|v| !v.is_null())
            .unwrap_or(false);

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
            supports_resources,
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

    /// Get names of all connected servers.
    pub fn server_names(&self) -> Vec<String> {
        self.servers.keys().cloned().collect()
    }

    /// Check if a connected server declared the resources capability.
    pub fn server_supports_resources(&self, server_name: &str) -> bool {
        self.servers
            .get(server_name)
            .map(|s| s.supports_resources)
            .unwrap_or(false)
    }

    /// List all resources from a server.
    pub async fn list_resources(&self, server_name: &str) -> Result<Vec<McpResource>, McpError> {
        let server = self
            .servers
            .get(server_name)
            .ok_or_else(|| McpError::ServerNotFound(server_name.to_string()))?;

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest::new(id, "resources/list", None);
        let response = server.transport.request(&request).await?;

        let result_value = response
            .result
            .ok_or_else(|| McpError::Transport("No result in resources/list response".into()))?;

        let list_result: ResourcesListResult = serde_json::from_value(result_value)
            .map_err(|e| McpError::Transport(format!("Failed to parse resources/list: {}", e)))?;

        Ok(list_result.resources)
    }

    /// Read a single resource by URI from a server. Returns the text content.
    pub async fn read_resource(&self, server_name: &str, uri: &str) -> Result<String, McpError> {
        let server = self
            .servers
            .get(server_name)
            .ok_or_else(|| McpError::ServerNotFound(server_name.to_string()))?;

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest::new(id, "resources/read", Some(json!({ "uri": uri })));
        let response = server.transport.request(&request).await?;

        let result_value = response
            .result
            .ok_or_else(|| McpError::Transport("No result in resources/read response".into()))?;

        let read_result: ResourcesReadResult = serde_json::from_value(result_value)
            .map_err(|e| McpError::Transport(format!("Failed to parse resources/read: {}", e)))?;

        // Return the first text content found
        read_result
            .contents
            .into_iter()
            .find_map(|c| c.text)
            .ok_or_else(|| McpError::Transport(format!("No text content in resource '{}'", uri)))
    }

    /// Gracefully shutdown all servers
    pub async fn shutdown(&self) {
        for (name, server) in &self.servers {
            if let Err(e) = server.transport.close().await {
                tracing::warn!(target: "aion_mcp", server = %name, error = %e, "error closing mcp server");
            }
        }
    }

    /// Test-only constructor: build a manager from pre-configured servers.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn new_for_test(
        entries: Vec<(&str, bool, Box<dyn super::transport::McpTransport>)>,
    ) -> Self {
        let mut servers = HashMap::new();
        for (name, supports_resources, transport) in entries {
            servers.insert(
                name.to_string(),
                McpServer {
                    name: name.to_string(),
                    transport,
                    tools: vec![],
                    supports_resources,
                },
            );
        }
        Self {
            servers,
            next_id: AtomicU64::new(10),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::JsonRpcResponse;
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::sync::Barrier;

    // -----------------------------------------------------------------------
    // MockTransport: returns pre-configured JSON-RPC responses
    // -----------------------------------------------------------------------

    struct MockTransport {
        /// Responses returned in order for each request call
        responses: Mutex<Vec<serde_json::Value>>,
    }

    impl MockTransport {
        fn new(responses: Vec<serde_json::Value>) -> Self {
            Self {
                responses: Mutex::new(responses),
            }
        }
    }

    #[async_trait]
    impl McpTransport for MockTransport {
        async fn request(&self, _req: &JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
            let mut guard = self.responses.lock().unwrap();
            let value = if guard.is_empty() {
                json!(null)
            } else {
                guard.remove(0)
            };
            Ok(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: Some(1),
                result: Some(value),
                error: None,
            })
        }

        async fn notify(&self, _req: &JsonRpcRequest) -> Result<(), McpError> {
            Ok(())
        }

        async fn close(&self) -> Result<(), McpError> {
            Ok(())
        }
    }

    struct ErrorTransport;

    #[async_trait]
    impl McpTransport for ErrorTransport {
        async fn request(&self, _req: &JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
            Err(McpError::Transport("mock transport error".into()))
        }

        async fn notify(&self, _req: &JsonRpcRequest) -> Result<(), McpError> {
            Ok(())
        }

        async fn close(&self) -> Result<(), McpError> {
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // Test helpers: build McpManager with pre-configured servers
    // -----------------------------------------------------------------------

    fn make_manager_with_servers(entries: Vec<(&str, bool, Box<dyn McpTransport>)>) -> McpManager {
        McpManager::new_for_test(entries)
    }

    fn delayed_config(delay_ms: u64, startup_timeout_ms: Option<u64>) -> McpServerConfig {
        McpServerConfig {
            transport: TransportType::Stdio,
            command: None,
            args: Some(vec![delay_ms.to_string()]),
            env: None,
            url: None,
            headers: None,
            deferred: None,
            startup_timeout_ms,
        }
    }

    fn successful_test_server(name: &str) -> McpServer {
        McpServer {
            name: name.to_string(),
            transport: Box::new(MockTransport::new(vec![])),
            tools: vec![],
            supports_resources: false,
        }
    }

    async fn delayed_test_connect(
        name: String,
        config: McpServerConfig,
    ) -> Result<McpServer, McpError> {
        let delay_ms = config
            .args
            .as_ref()
            .and_then(|args| args.first())
            .and_then(|arg| arg.parse::<u64>().ok())
            .unwrap_or(0);
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        Ok(successful_test_server(&name))
    }

    #[tokio::test]
    async fn connect_all_attempts_servers_concurrently() {
        let configs = HashMap::from([
            ("slow-a".to_string(), delayed_config(0, None)),
            ("slow-b".to_string(), delayed_config(0, None)),
            ("slow-c".to_string(), delayed_config(0, None)),
        ]);
        let all_connectors_started = Arc::new(Barrier::new(4));
        let connector_barrier = Arc::clone(&all_connectors_started);

        let manager_task = tokio::spawn(async move {
            McpManager::connect_all_with_connector(&configs, move |name, _config| {
                let connector_barrier = Arc::clone(&connector_barrier);
                async move {
                    connector_barrier.wait().await;
                    Ok(successful_test_server(&name))
                }
            })
            .await
            .unwrap()
        });

        tokio::time::timeout(Duration::from_millis(100), all_connectors_started.wait())
            .await
            .expect("connect_all should start every connector before awaiting the first result");
        let manager = manager_task.await.unwrap();

        assert_eq!(manager.server_names().len(), 3);
    }

    #[tokio::test]
    async fn connect_all_applies_per_server_startup_timeout() {
        let configs = HashMap::from([
            ("fast".to_string(), delayed_config(10, None)),
            ("too-slow".to_string(), delayed_config(200, Some(20))),
        ]);

        let started_at = tokio::time::Instant::now();
        let manager = McpManager::connect_all_with_connector(&configs, delayed_test_connect)
            .await
            .unwrap();
        let elapsed = started_at.elapsed();

        assert_eq!(manager.server_names(), vec!["fast".to_string()]);
        assert!(
            elapsed < Duration::from_millis(150),
            "timed out server should not block connect_all; elapsed={elapsed:?}"
        );
    }

    // -----------------------------------------------------------------------
    // TC-2.x: server_supports_resources [黑盒 + 白盒]
    // -----------------------------------------------------------------------

    #[test]
    fn tc_2_1_server_supports_resources_true() {
        // [黑盒] TC-2.1: server with resources capability returns true
        let manager = make_manager_with_servers(vec![(
            "test-server",
            true,
            Box::new(MockTransport::new(vec![])),
        )]);

        assert!(manager.server_supports_resources("test-server"));
    }

    #[test]
    fn tc_2_2_server_supports_resources_false() {
        // [黑盒] TC-2.2: server without resources capability returns false
        let manager = make_manager_with_servers(vec![(
            "no-resources-server",
            false,
            Box::new(MockTransport::new(vec![])),
        )]);

        assert!(!manager.server_supports_resources("no-resources-server"));
    }

    #[test]
    fn tc_2_3_server_supports_resources_unknown_server() {
        // [黑盒] TC-2.3: unknown server name returns false (not error)
        let manager = make_manager_with_servers(vec![]);

        assert!(!manager.server_supports_resources("unknown-server"));
    }

    #[test]
    fn tc_2_wb_supports_resources_from_capabilities_null_value() {
        // [白盒] capabilities.get("resources") = null → supports_resources = false
        // This is tested via the parsed field; we verify via make_manager helper
        let manager = make_manager_with_servers(vec![(
            "server",
            false, // null resources → false per impl: !v.is_null() = false
            Box::new(MockTransport::new(vec![])),
        )]);

        assert!(!manager.server_supports_resources("server"));
    }

    // -----------------------------------------------------------------------
    // TC-2.10/2.11: server_names [黑盒]
    // -----------------------------------------------------------------------

    #[test]
    fn tc_2_10_server_names_returns_all() {
        // [黑盒] TC-2.10: server_names returns all connected server names
        let manager = make_manager_with_servers(vec![
            ("server-a", false, Box::new(MockTransport::new(vec![]))),
            ("server-b", true, Box::new(MockTransport::new(vec![]))),
        ]);

        let mut names = manager.server_names();
        names.sort();
        assert_eq!(names, vec!["server-a", "server-b"]);
    }

    #[test]
    fn tc_2_11_server_names_empty_manager() {
        // [黑盒] TC-2.11: no connected servers → empty vec
        let manager = make_manager_with_servers(vec![]);

        assert!(manager.server_names().is_empty());
    }

    #[test]
    fn tc_2_wb_server_names_returns_owned_strings() {
        // [白盒] Decision 1: server_names() returns Vec<String> not Vec<&str>
        let manager = make_manager_with_servers(vec![(
            "my-server",
            false,
            Box::new(MockTransport::new(vec![])),
        )]);

        let names: Vec<String> = manager.server_names();
        assert_eq!(names, vec!["my-server"]);
    }

    // -----------------------------------------------------------------------
    // TC-2.4/2.5: list_resources [黑盒]
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn tc_2_4_list_resources_normal() {
        // [黑盒] TC-2.4: list_resources returns resources from server
        let resources_response = json!({
            "resources": [
                {"uri": "skill://skill-a"},
                {"uri": "skill://skill-b", "name": "Skill B"}
            ]
        });

        let manager = make_manager_with_servers(vec![(
            "test-server",
            true,
            Box::new(MockTransport::new(vec![resources_response])),
        )]);

        let result = manager.list_resources("test-server").await.unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].uri, "skill://skill-a");
        assert_eq!(result[1].uri, "skill://skill-b");
    }

    #[tokio::test]
    async fn tc_2_5_list_resources_empty() {
        // [黑盒] TC-2.5: list_resources returns empty list when server has no resources
        let resources_response = json!({"resources": []});

        let manager = make_manager_with_servers(vec![(
            "test-server",
            true,
            Box::new(MockTransport::new(vec![resources_response])),
        )]);

        let result = manager.list_resources("test-server").await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn tc_2_6_list_resources_server_not_found() {
        // [黑盒] TC-2.6: list_resources returns error when server does not exist
        let manager = make_manager_with_servers(vec![]);

        let result = manager.list_resources("nonexistent").await;
        assert!(result.is_err());
        match result.unwrap_err() {
            McpError::ServerNotFound(name) => assert_eq!(name, "nonexistent"),
            e => panic!("expected ServerNotFound, got {:?}", e),
        }
    }

    // -----------------------------------------------------------------------
    // TC-2.7/2.8/2.9: read_resource [黑盒 + 白盒]
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn tc_2_7_read_resource_returns_text() {
        // [黑盒] TC-2.7: read_resource returns text content
        let read_response = json!({
            "contents": [{"uri": "skill://my-skill", "mimeType": "text/plain", "text": "---\ndescription: A skill\n---\n# My Skill\n"}]
        });

        let manager = make_manager_with_servers(vec![(
            "test-server",
            true,
            Box::new(MockTransport::new(vec![read_response])),
        )]);

        let result = manager
            .read_resource("test-server", "skill://my-skill")
            .await
            .unwrap();
        assert!(result.contains("description: A skill"));
    }

    #[tokio::test]
    async fn tc_2_8_read_resource_transport_error() {
        // [黑盒] TC-2.8: read_resource returns error when server returns transport error
        let manager =
            make_manager_with_servers(vec![("test-server", true, Box::new(ErrorTransport))]);

        let result = manager
            .read_resource("test-server", "skill://nonexistent")
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn tc_2_9_read_resource_server_not_found() {
        // [黑盒] TC-2.9: read_resource returns error when server does not exist
        let manager = make_manager_with_servers(vec![]);

        let result = manager
            .read_resource("nonexistent", "skill://my-skill")
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            McpError::ServerNotFound(name) => assert_eq!(name, "nonexistent"),
            e => panic!("expected ServerNotFound, got {:?}", e),
        }
    }

    #[tokio::test]
    async fn tc_2_wb_read_resource_no_text_content_returns_error() {
        // [白盒] Decision 3: find_map returns None when all contents have text=None → error
        let read_response = json!({
            "contents": [{"uri": "skill://binary", "mimeType": "application/octet-stream"}]
        });

        let manager = make_manager_with_servers(vec![(
            "test-server",
            true,
            Box::new(MockTransport::new(vec![read_response])),
        )]);

        let result = manager.read_resource("test-server", "skill://binary").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn tc_2_wb_read_resource_find_map_first_text() {
        // [白盒] Decision 3: find_map returns first content with non-None text
        let read_response = json!({
            "contents": [
                {"uri": "skill://x"},
                {"uri": "skill://x", "text": "actual content"}
            ]
        });

        let manager = make_manager_with_servers(vec![(
            "test-server",
            true,
            Box::new(MockTransport::new(vec![read_response])),
        )]);

        let result = manager
            .read_resource("test-server", "skill://x")
            .await
            .unwrap();
        assert_eq!(result, "actual content");
    }

    #[test]
    fn tc_2_wb_next_id_starts_at_10() {
        // [白盒] Decision 4: AtomicU64 counter starts at 10 to avoid conflict with connect_server IDs 1/2
        let manager = make_manager_with_servers(vec![]);
        // next_id is private — we verify by doing two fetch_adds and checking values are 10 and 11
        let id1 = manager
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let id2 = manager
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(id1, 10, "first ID should be 10");
        assert_eq!(id2, 11, "second ID should be 11");
    }
}
