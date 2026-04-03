use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC 2.0 request
#[derive(Debug, Serialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    pub fn new(id: u64, method: &str, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0",
            id: Some(id),
            method: method.to_string(),
            params,
        }
    }

    pub fn notification(method: &str, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0",
            id: None,
            method: method.to_string(),
            params,
        }
    }
}

/// JSON-RPC 2.0 response
#[derive(Debug, Deserialize)]
pub struct JsonRpcResponse {
    #[allow(dead_code)]
    pub jsonrpc: String,
    pub id: Option<u64>,
    pub result: Option<Value>,
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[allow(dead_code)]
    pub data: Option<Value>,
}

/// MCP tool definition returned by tools/list
#[derive(Debug, Clone, Deserialize)]
pub struct McpToolDef {
    pub name: String,
    pub description: Option<String>,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

/// MCP tool call result
#[derive(Debug, Deserialize)]
pub struct McpToolResult {
    pub content: Vec<McpContent>,
}

/// Content types returned by MCP tool calls
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum McpContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    #[serde(rename = "resource")]
    Resource {
        #[allow(dead_code)]
        resource: Value,
    },
}

/// Initialize request params
#[derive(Debug, Serialize)]
pub struct InitializeParams {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    pub capabilities: ClientCapabilities,
    #[serde(rename = "clientInfo")]
    pub client_info: ClientInfo,
}

#[derive(Debug, Serialize)]
pub struct ClientCapabilities {
    pub tools: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

/// Initialize response result
#[derive(Debug, Deserialize)]
pub struct InitializeResult {
    #[serde(rename = "protocolVersion")]
    #[allow(dead_code)]
    pub protocol_version: String,
    #[allow(dead_code)]
    pub capabilities: Value,
    #[serde(rename = "serverInfo")]
    #[allow(dead_code)]
    pub server_info: Option<Value>,
}

/// Tools list response
#[derive(Debug, Deserialize)]
pub struct ToolsListResult {
    pub tools: Vec<McpToolDef>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_jsonrpc_request_serialization() {
        // Verify that a regular request serializes with jsonrpc, id, method and params
        let req = JsonRpcRequest::new(1, "tools/list", Some(json!({"cursor": null})));
        let value = serde_json::to_value(&req).unwrap();

        assert_eq!(value["jsonrpc"], "2.0");
        assert_eq!(value["id"], 1u64);
        assert_eq!(value["method"], "tools/list");
        assert!(value.get("params").is_some());
    }

    #[test]
    fn test_jsonrpc_request_notification() {
        // Notifications must not include the "id" field when serialized
        let req = JsonRpcRequest::notification("notifications/initialized", None);
        let value = serde_json::to_value(&req).unwrap();

        assert_eq!(value["jsonrpc"], "2.0");
        assert_eq!(value["method"], "notifications/initialized");
        // id should be absent because it is None and marked skip_serializing_if
        assert!(value.get("id").is_none() || value["id"].is_null());
        // When skip_serializing_if fires the key is absent entirely
        assert!(!value.as_object().unwrap().contains_key("id"));
    }

    #[test]
    fn test_jsonrpc_response_deserialization_success() {
        // Deserialize a successful JSON-RPC response and check result field
        let json_str = r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json_str).unwrap();

        assert_eq!(resp.id, Some(1));
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn test_jsonrpc_response_deserialization_error() {
        // Deserialize an error JSON-RPC response and check error fields
        let json_str = r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32601,"message":"Method not found"}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json_str).unwrap();

        assert_eq!(resp.id, Some(2));
        assert!(resp.result.is_none());
        let err = resp.error.expect("error field should be present");
        assert_eq!(err.code, -32601);
        assert_eq!(err.message, "Method not found");
    }

    #[test]
    fn test_mcp_tool_def_deserialization() {
        // Deserialize a McpToolDef including the camelCase inputSchema rename
        let json_str = r#"{
            "name": "read_file",
            "description": "Read a file from disk",
            "inputSchema": {"type": "object", "properties": {}}
        }"#;
        let tool: McpToolDef = serde_json::from_str(json_str).unwrap();

        assert_eq!(tool.name, "read_file");
        assert_eq!(tool.description.as_deref(), Some("Read a file from disk"));
        assert_eq!(tool.input_schema["type"], "object");
    }

    #[test]
    fn test_mcp_content_text() {
        // Deserialize McpContent::Text using the internally-tagged "type" field
        let json_str = r#"{"type":"text","text":"hello world"}"#;
        let content: McpContent = serde_json::from_str(json_str).unwrap();

        match content {
            McpContent::Text { text } => assert_eq!(text, "hello world"),
            other => panic!("expected McpContent::Text, got {:?}", other),
        }
    }

    #[test]
    fn test_mcp_content_image() {
        // Deserialize McpContent::Image including the camelCase mimeType rename
        let json_str = r#"{"type":"image","data":"base64data==","mimeType":"image/png"}"#;
        let content: McpContent = serde_json::from_str(json_str).unwrap();

        match content {
            McpContent::Image { data, mime_type } => {
                assert_eq!(data, "base64data==");
                assert_eq!(mime_type, "image/png");
            }
            other => panic!("expected McpContent::Image, got {:?}", other),
        }
    }
}
