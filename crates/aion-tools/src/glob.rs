use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{Value, json};

use aion_protocol::events::ToolCategory;
use aion_types::tool::{JsonSchema, ToolResult};

use crate::Tool;

const MAX_RESULTS: usize = 100;

pub struct GlobTool {
    cwd: PathBuf,
}

impl GlobTool {
    pub fn new(cwd: PathBuf) -> Self {
        Self { cwd }
    }
}

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "Glob"
    }

    fn description(&self) -> &str {
        "Fast file pattern matching tool that works with any codebase size.\n\n\
         - Supports glob patterns like \"**/*.rs\" or \"src/**/*.ts\".\n\
         - Returns matching file paths sorted by modification time (newest first).\n\
         - Returns at most 100 results. Only returns files, not directories.\n\
         - The path parameter defaults to the current working directory.\n\
         - Use this tool when you need to find files by name or extension patterns."
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
        let root_path = if Path::new(root).is_relative() {
            self.cwd.join(root)
        } else {
            PathBuf::from(root)
        };

        tracing::debug!(cwd = %self.cwd.display(), resolved_root = %root_path.display(), pattern = %pattern, "GlobTool scanning");

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

            let Ok(path) = entry else {
                continue;
            };
            if !path.is_file() {
                continue;
            }

            let mtime = path
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

            // Make path relative to root
            let display_path = path.strip_prefix(&root_path).unwrap_or(&path).display().to_string();

            files.push((mtime, display_path));
        }

        // Sort by modification time, newest first
        files.sort_by_key(|f| std::cmp::Reverse(f.0));

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
#[path = "glob_test.rs"]
mod glob_test;
