use async_trait::async_trait;
use serde_json::{Value, json};

use crate::protocol::events::ToolCategory;
use crate::types::tool::{JsonSchema, ToolResult};

use super::Tool;

pub struct EditTool;

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "Edit"
    }

    fn description(&self) -> &str {
        "Performs exact string replacements in files."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The absolute path to the file to modify"
                },
                "old_string": {
                    "type": "string",
                    "description": "The text to replace"
                },
                "new_string": {
                    "type": "string",
                    "description": "The replacement text"
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Replace all occurrences (default false)"
                }
            },
            "required": ["file_path", "old_string", "new_string"]
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
        let Some(old_string) = input["old_string"].as_str() else {
            return ToolResult {
                content: "Missing required parameter: old_string".to_string(),
                is_error: true,
            };
        };
        let Some(new_string) = input["new_string"].as_str() else {
            return ToolResult {
                content: "Missing required parameter: new_string".to_string(),
                is_error: true,
            };
        };
        let replace_all = input["replace_all"].as_bool().unwrap_or(false);

        let content = match std::fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(e) => {
                return ToolResult {
                    content: format!("Failed to read file {}: {}", file_path, e),
                    is_error: true,
                };
            }
        };

        let match_count = content.matches(old_string).count();

        if match_count == 0 {
            return ToolResult {
                content: "old_string not found in file".to_string(),
                is_error: true,
            };
        }

        if match_count > 1 && !replace_all {
            return ToolResult {
                content: format!(
                    "Multiple matches found ({}). Use replace_all or provide more context.",
                    match_count
                ),
                is_error: true,
            };
        }

        let new_content = if replace_all {
            content.replace(old_string, new_string)
        } else {
            content.replacen(old_string, new_string, 1)
        };

        if let Err(e) = std::fs::write(file_path, &new_content) {
            return ToolResult {
                content: format!("Failed to write file: {}", e),
                is_error: true,
            };
        }

        ToolResult {
            content: format!(
                "Edited {}: replaced {} occurrence(s)",
                file_path, match_count
            ),
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
        format!("Edit {}", path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_edit_replace_block() {
        // arrange
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "hello world").unwrap();

        let tool = EditTool;
        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "old_string": "hello",
            "new_string": "goodbye"
        });

        // act
        let result = tool.execute(input).await;

        // assert
        assert!(!result.is_error, "unexpected error: {}", result.content);
        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "goodbye world");
    }

    #[tokio::test]
    async fn test_edit_old_string_not_found() {
        // arrange
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "hello world").unwrap();

        let tool = EditTool;
        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "old_string": "nonexistent",
            "new_string": "replacement"
        });

        // act
        let result = tool.execute(input).await;

        // assert
        assert!(result.is_error);
        assert!(
            result.content.contains("not found"),
            "expected 'not found' in error message, got: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn test_edit_preserves_surrounding() {
        // arrange
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "aaa\nbbb\nccc\n").unwrap();

        let tool = EditTool;
        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "old_string": "bbb",
            "new_string": "XXX"
        });

        // act
        let result = tool.execute(input).await;

        // assert
        assert!(!result.is_error, "unexpected error: {}", result.content);
        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "aaa\nXXX\nccc\n");
    }

    #[tokio::test]
    async fn test_edit_nonexistent_file() {
        // arrange
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("does_not_exist.txt");

        let tool = EditTool;
        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "old_string": "anything",
            "new_string": "replacement"
        });

        // act
        let result = tool.execute(input).await;

        // assert
        assert!(result.is_error);
        assert!(
            result.content.contains("Failed to read file"),
            "expected read failure message, got: {}",
            result.content
        );
    }
}
