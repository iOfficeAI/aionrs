use std::path::Path;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::protocol::events::ToolCategory;
use crate::types::tool::{JsonSchema, ToolResult};

use super::Tool;

pub struct WriteTool;

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &str {
        "Write"
    }

    fn description(&self) -> &str {
        "Writes content to a file, creating parent directories if needed."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The absolute path to the file to write"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file"
                }
            },
            "required": ["file_path", "content"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        false
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let Some(file_path) = input["file_path"].as_str() else {
            return ToolResult {
                content: "Missing required parameter: file_path".to_string(),
                is_error: true,
            };
        };
        let Some(content) = input["content"].as_str() else {
            return ToolResult {
                content: "Missing required parameter: content".to_string(),
                is_error: true,
            };
        };

        let path = Path::new(file_path);
        let existed = path.exists();

        // Create parent directories
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    return ToolResult {
                        content: format!("Failed to create directories: {}", e),
                        is_error: true,
                    };
                }
            }
        }

        // Write atomically: write to temp file, then rename
        let tmp_path = format!("{}.tmp.{}", file_path, std::process::id());
        if let Err(e) = std::fs::write(&tmp_path, content) {
            return ToolResult {
                content: format!("Failed to write file: {}", e),
                is_error: true,
            };
        }

        if let Err(e) = std::fs::rename(&tmp_path, file_path) {
            // Fallback: direct write if rename fails (cross-device)
            let _ = std::fs::remove_file(&tmp_path);
            if let Err(e) = std::fs::write(file_path, content) {
                return ToolResult {
                    content: format!("Failed to write file: {}", e),
                    is_error: true,
                };
            }
            return ToolResult {
                content: format!("Updated {} (rename failed: {}, used direct write)", file_path, e),
                is_error: false,
            };
        }

        let line_count = content.lines().count();
        let action = if existed { "Updated" } else { "Created" };
        ToolResult {
            content: format!("{} {} ({} lines)", action, file_path, line_count),
            is_error: false,
        }
    }

    fn max_result_size(&self) -> usize {
        10_000
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Edit
    }

    fn describe(&self, input: &Value) -> String {
        let path = input.get("file_path").and_then(|v| v.as_str()).unwrap_or("unknown");
        format!("Write to {}", path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    use crate::tools::Tool;

    #[tokio::test]
    async fn test_write_new_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("hello.txt");

        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "content": "hello world"
        });

        let tool = WriteTool;
        let result = tool.execute(input).await;

        assert!(!result.is_error, "expected success, got: {}", result.content);
        assert!(file_path.exists(), "file should exist after write");
        assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "hello world");
    }

    #[tokio::test]
    async fn test_write_creates_parent_dirs() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("subdir/nested/file.txt");

        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "content": "nested content"
        });

        let tool = WriteTool;
        let result = tool.execute(input).await;

        assert!(!result.is_error, "expected success, got: {}", result.content);
        assert!(file_path.parent().unwrap().exists(), "parent dirs should be created");
        assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "nested content");
    }

    #[tokio::test]
    async fn test_write_overwrite_existing() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("overwrite.txt");

        let tool = WriteTool;

        // Write initial content
        let input1 = json!({
            "file_path": file_path.to_str().unwrap(),
            "content": "original"
        });
        let result1 = tool.execute(input1).await;
        assert!(!result1.is_error);
        assert!(result1.content.contains("Created"));

        // Overwrite with new content
        let input2 = json!({
            "file_path": file_path.to_str().unwrap(),
            "content": "replaced"
        });
        let result2 = tool.execute(input2).await;
        assert!(!result2.is_error);
        assert!(result2.content.contains("Updated"));

        assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "replaced");
    }

    #[tokio::test]
    async fn test_write_file_content_matches() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("exact.txt");

        let content = "line 1\nline 2\nline 3\n";
        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "content": content
        });

        let tool = WriteTool;
        let result = tool.execute(input).await;

        assert!(!result.is_error, "expected success, got: {}", result.content);

        let read_back = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(read_back, content, "read-back content must exactly match written content");
    }
}
