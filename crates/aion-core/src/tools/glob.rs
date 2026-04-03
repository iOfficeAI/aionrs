use std::path::Path;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::protocol::events::ToolCategory;
use crate::types::tool::{JsonSchema, ToolResult};

use super::Tool;

const MAX_RESULTS: usize = 100;

pub struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "Glob"
    }

    fn description(&self) -> &str {
        "Finds files matching a glob pattern."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern, e.g. \"**/*.rs\""
                },
                "path": {
                    "type": "string",
                    "description": "Root directory (default: cwd)"
                }
            },
            "required": ["pattern"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let Some(pattern) = input["pattern"].as_str() else {
            return ToolResult {
                content: "Missing required parameter: pattern".to_string(),
                is_error: true,
            };
        };

        let root = input["path"].as_str().unwrap_or(".");
        let root_path = Path::new(root);

        // Build full glob pattern
        let full_pattern = if pattern.starts_with('/') {
            pattern.to_string()
        } else {
            format!("{}/{}", root_path.display(), pattern)
        };

        let entries = match glob::glob(&full_pattern) {
            Ok(paths) => paths,
            Err(e) => {
                return ToolResult {
                    content: format!("Invalid glob pattern: {}", e),
                    is_error: true,
                };
            }
        };

        let mut files: Vec<(std::time::SystemTime, String)> = Vec::new();

        for entry in entries {
            if files.len() >= MAX_RESULTS {
                break;
            }
            if let Ok(path) = entry {
                if path.is_file() {
                    let mtime = path
                        .metadata()
                        .and_then(|m| m.modified())
                        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

                    // Make path relative to root
                    let display_path = path
                        .strip_prefix(root_path)
                        .unwrap_or(&path)
                        .display()
                        .to_string();

                    files.push((mtime, display_path));
                }
            }
        }

        // Sort by modification time, newest first
        files.sort_by(|a, b| b.0.cmp(&a.0));

        if files.is_empty() {
            return ToolResult {
                content: "No files matched the pattern".to_string(),
                is_error: false,
            };
        }

        let result: Vec<String> = files.into_iter().map(|(_, path)| path).collect();
        ToolResult {
            content: result.join("\n"),
            is_error: false,
        }
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    fn describe(&self, input: &Value) -> String {
        let pattern = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("*");
        format!("Search for {}", pattern)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use tempfile::tempdir;

    use crate::types::tool::ToolResult;

    async fn run_glob(pattern: &str, path: &str) -> ToolResult {
        let tool = GlobTool;
        let input = json!({ "pattern": pattern, "path": path });
        tool.execute(input).await
    }

    #[tokio::test]
    async fn test_glob_matches_pattern() {
        let dir = tempdir().unwrap();
        let base = dir.path();

        fs::write(base.join("main.rs"), "fn main() {}").unwrap();
        fs::write(base.join("lib.rs"), "pub mod lib;").unwrap();
        fs::write(base.join("notes.txt"), "some notes").unwrap();
        fs::write(base.join("readme.md"), "# Readme").unwrap();

        let result = run_glob("*.rs", base.to_str().unwrap()).await;

        assert!(!result.is_error, "glob should succeed");
        let lines: Vec<&str> = result.content.lines().collect();
        assert_eq!(lines.len(), 2, "should match exactly 2 .rs files");
        for line in &lines {
            assert!(line.ends_with(".rs"), "each match should be a .rs file, got: {}", line);
        }
        assert!(!result.content.contains("notes.txt"), "should not include .txt files");
        assert!(!result.content.contains("readme.md"), "should not include .md files");
    }

    #[tokio::test]
    async fn test_glob_no_matches() {
        let dir = tempdir().unwrap();
        let base = dir.path();

        fs::write(base.join("main.rs"), "fn main() {}").unwrap();
        fs::write(base.join("lib.rs"), "pub mod lib;").unwrap();

        let result = run_glob("*.xyz", base.to_str().unwrap()).await;

        assert!(!result.is_error, "no-match glob should not be an error");
        assert_eq!(result.content, "No files matched the pattern");
    }

    #[tokio::test]
    async fn test_glob_with_limit() {
        let dir = tempdir().unwrap();
        let base = dir.path();

        for i in 0..5 {
            fs::write(base.join(format!("file_{}.txt", i)), format!("content {}", i)).unwrap();
        }

        let result = run_glob("*.txt", base.to_str().unwrap()).await;

        assert!(!result.is_error, "glob should succeed");
        let lines: Vec<&str> = result.content.lines().collect();
        assert_eq!(lines.len(), 5, "all 5 files should be returned");
    }

    #[tokio::test]
    async fn test_glob_recursive() {
        let dir = tempdir().unwrap();
        let base = dir.path();

        // Create nested directory structure
        let sub_a = base.join("a");
        let sub_b = base.join("a").join("b");
        fs::create_dir_all(&sub_b).unwrap();

        fs::write(base.join("root.txt"), "root level").unwrap();
        fs::write(sub_a.join("mid.txt"), "middle level").unwrap();
        fs::write(sub_b.join("deep.txt"), "deep level").unwrap();
        // Non-matching file
        fs::write(sub_a.join("skip.rs"), "not a txt").unwrap();

        let result = run_glob("**/*.txt", base.to_str().unwrap()).await;

        assert!(!result.is_error, "recursive glob should succeed");
        let lines: Vec<&str> = result.content.lines().collect();
        assert_eq!(lines.len(), 3, "should find 3 .txt files across all levels");
        assert!(
            result.content.contains("root.txt"),
            "should include root-level file"
        );
        assert!(
            result.content.contains("mid.txt"),
            "should include mid-level file"
        );
        assert!(
            result.content.contains("deep.txt"),
            "should include deep-level file"
        );
        assert!(
            !result.content.contains("skip.rs"),
            "should not include .rs files"
        );
    }
}
