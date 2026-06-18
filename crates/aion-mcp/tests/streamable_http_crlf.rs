//! End-to-end test: a stateful Streamable-HTTP MCP server that emits SSE with
//! CRLF (`\r\n`) line endings, exactly like Python `fastmcp` / the MCP Python
//! SDK do via `sse-starlette` (whose default separator is `\r\n`).
//!
//! Regression guard for the issue where such servers connect successfully but
//! expose no tools to the model: the runtime SSE parser searched for `\n\n`
//! event boundaries, which never appear in a `\r\n\r\n`-delimited stream.

use std::collections::HashMap;

use aion_mcp::config::{McpServerConfig, TransportType};
use aion_mcp::manager::McpManager;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Build an HTTP response whose body is a single SSE `message` event using
/// CRLF separators — byte-for-byte what `sse-starlette` produces.
fn sse_response(json_rpc: &str, session_id: Option<&str>) -> Vec<u8> {
    let body = format!("event: message\r\ndata: {json_rpc}\r\n\r\n");
    let mut head = String::from("HTTP/1.1 200 OK\r\n");
    head.push_str("Content-Type: text/event-stream\r\n");
    if let Some(sid) = session_id {
        head.push_str(&format!("mcp-session-id: {sid}\r\n"));
    }
    head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    head.push_str("Connection: close\r\n\r\n");
    head.push_str(&body);
    head.into_bytes()
}

fn accepted_204() -> Vec<u8> {
    b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
}

/// Read one full HTTP request (headers + Content-Length body) from the socket.
async fn read_http_request(socket: &mut tokio::net::TcpStream) -> String {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    // Read headers.
    let header_end = loop {
        let n = socket.read(&mut tmp).await.unwrap();
        if n == 0 {
            return String::from_utf8_lossy(&buf).into_owned();
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let headers = String::from_utf8_lossy(&buf[..header_end]).to_lowercase();
    let content_length = headers
        .lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while buf.len() < header_end + content_length {
        let n = socket.read(&mut tmp).await.unwrap();
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    String::from_utf8_lossy(&buf).into_owned()
}

#[tokio::test]
async fn stateful_fastmcp_style_server_exposes_tools() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{addr}/mcp");

    // Faithful stateful server: returns Mcp-Session-Id on initialize and serves
    // every JSON-RPC response as CRLF-delimited SSE.
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_http_request(&mut socket).await;

            let response = if request.contains("\"initialize\"") {
                sse_response(
                    r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"fastmcp-test","version":"1.0"}}}"#,
                    Some("session-abc-123"),
                )
            } else if request.contains("\"tools/list\"") {
                sse_response(
                    r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"meta_ads_query","description":"Query Meta ads","inputSchema":{"type":"object"}}]}}"#,
                    Some("session-abc-123"),
                )
            } else {
                // notifications/initialized and anything else
                accepted_204()
            };

            socket.write_all(&response).await.unwrap();
            let _ = socket.flush().await;
        }
    });

    let config = McpServerConfig {
        transport: TransportType::StreamableHttp,
        command: None,
        args: None,
        env: None,
        url: Some(url),
        headers: None,
        deferred: None,
        startup_timeout_ms: Some(5_000),
    };
    let configs = HashMap::from([("meta-ads".to_string(), config)]);

    let manager = McpManager::connect_all(&configs).await.unwrap();

    assert!(
        manager.has_tool_name("meta_ads_query"),
        "stateful CRLF-SSE server tools must be discovered and exposed to the model"
    );
    assert_eq!(manager.tool_name_count("meta_ads_query"), 1);
}
