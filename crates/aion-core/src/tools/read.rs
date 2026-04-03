use async_trait::async_trait;
use serde_json::{Value, json};

use crate::protocol::events::ToolCategory;
use crate::types::tool::{JsonSchema, ToolResult};

use super::Tool;

pub struct ReadTool;

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str {
        "Read"
    }

    fn description(&self) -> &str {
        "Reads a file from the local filesystem. Returns content with line numbers."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The absolute path to the file to read"
                },
                "offset": {
                    "type": "integer",
                    "description": "Line number to start reading from (0-based)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to read"
                }
            },
            "required": ["file_path"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let Some(file_path) = input["file_path"].as_str() else {
            return ToolResult {
                content: "Missing required parameter: file_path".to_string(),
                is_error: true,
            };
        };

        let content = match std::fs::read(file_path) {
            Ok(bytes) => bytes,
            Err(e) => {
                return ToolResult {
                    content: format!("Failed to read file {}: {}", file_path, e),
                    is_error: true,
                };
            }
        };

        // Check if binary
        if content.iter().take(8192).any(|&b| b == 0) {
            return ToolResult {
                content: format!("(binary file, {} bytes)", content.len()),
                is_error: false,
            };
        }

        let text = String::from_utf8_lossy(&content);
        let lines: Vec<&str> = text.lines().collect();

        let offset = input["offset"].as_u64().unwrap_or(0) as usize;
        let limit = input["limit"]
            .as_u64()
            .map(|l| l as usize)
            .unwrap_or(lines.len());

        let end = (offset + limit).min(lines.len());
        let slice = &lines[offset.min(lines.len())..end];

        let numbered: Vec<String> = slice
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{:>6}\t{}", offset + i + 1, line))
            .collect();

        ToolResult {
            content: numbered.join("\n"),
            is_error: false,
        }
    }

    fn max_result_size(&self) -> usize {
        100_000
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    fn describe(&self, input: &Value) -> String {
        let path = input.get("file_path").and_then(|v| v.as_str()).unwrap_or("unknown");
        format!("Read {}", path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_read_file_full() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        let mut file = std::fs::File::create(&file_path).unwrap();
        writeln!(file, "line one").unwrap();
        writeln!(file, "line two").unwrap();
        writeln!(file, "line three").unwrap();
        drop(file);

        let input = json!({ "file_path": file_path.to_str().unwrap() });
        let result = ReadTool.execute(input).await;

        assert!(!result.is_error);
        assert!(result.content.contains("1\tline one"));
        assert!(result.content.contains("2\tline two"));
        assert!(result.content.contains("3\tline three"));
    }

    #[tokio::test]
    async fn test_read_file_with_offset_and_limit() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("lines.txt");
        let mut file = std::fs::File::create(&file_path).unwrap();
        for i in 1..=10 {
            writeln!(file, "line {}", i).unwrap();
        }
        drop(file);

        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "offset": 2,
            "limit": 3
        });
        let result = ReadTool.execute(input).await;

        assert!(!result.is_error);
        let lines: Vec<&str> = result.content.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("3\tline 3"));
        assert!(lines[1].contains("4\tline 4"));
        assert!(lines[2].contains("5\tline 5"));
    }

    #[tokio::test]
    async fn test_read_nonexistent_file() {
        let input = json!({ "file_path": "/tmp/nonexistent_file_abc123.txt" });
        let result = ReadTool.execute(input).await;

        assert!(result.is_error);
        assert!(result.content.contains("Failed to read file"));
    }

    #[tokio::test]
    async fn test_read_empty_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("empty.txt");
        std::fs::File::create(&file_path).unwrap();

        let input = json!({ "file_path": file_path.to_str().unwrap() });
        let result = ReadTool.execute(input).await;

        assert!(!result.is_error);
        assert!(result.content.is_empty());
    }

    #[tokio::test]
    async fn test_read_large_file_truncation() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("large.txt");
        let mut file = std::fs::File::create(&file_path).unwrap();
        for i in 1..=200 {
            writeln!(file, "line number {}", i).unwrap();
        }
        drop(file);

        let input = json!({ "file_path": file_path.to_str().unwrap() });
        let result = ReadTool.execute(input).await;

        assert!(!result.is_error);
        let lines: Vec<&str> = result.content.lines().collect();
        assert_eq!(lines.len(), 200);
        assert!(lines[0].contains("1\tline number 1"));
        assert!(lines[199].contains("200\tline number 200"));
    }
}
