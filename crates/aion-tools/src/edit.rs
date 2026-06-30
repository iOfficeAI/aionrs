use std::path::Path;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde_json::{Value, json};

use aion_protocol::events::ToolCategory;
use aion_types::tool::{JsonSchema, ToolResult};

use crate::Tool;
use crate::file_cache::{FileStateCache, file_mtime_ms, update_cache_after_write};

pub struct EditTool {
    file_cache: Option<Arc<RwLock<FileStateCache>>>,
}

impl EditTool {
    /// Create an EditTool with optional file state cache.
    ///
    /// When cache is `Some`, the tool enforces:
    /// - "Must Read first" guard (file must be in cache before editing)
    /// - Staleness detection (disk mtime must match cached mtime)
    /// - Post-write cache update (mtime + content refreshed after edit)
    ///
    /// Pass `None` to disable all cache-related guards (legacy behavior).
    pub fn new(file_cache: Option<Arc<RwLock<FileStateCache>>>) -> Self {
        Self { file_cache }
    }
}

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "Edit"
    }

    fn description(&self) -> &str {
        "Performs exact string replacements in files.\n\n\
         Usage:\n\
         - You must use the Read tool first before editing a file.\n\
         - The old_string must be unique in the file. If multiple matches exist, \
         the edit will fail. Provide more surrounding context to make it unique, \
         or use replace_all to change every occurrence.\n\
         - Use replace_all for renaming variables or replacing all instances of a string.\n\
         - Prefer Edit over Write for modifying existing files — Edit only sends the diff.\n\
         - When matching text from Read output, preserve the exact indentation (tabs/spaces)."
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

        let path = Path::new(file_path);

        // Cache guard: "must Read first" + staleness detection.
        if let Some(cache_arc) = &self.file_cache
            && let Ok(mut cache) = cache_arc.write()
        {
            let cached = cache.get(path);
            if cached.is_none() {
                return ToolResult {
                    content: format!(
                        "You must Read {} before editing. Use the Read tool first \
                         so the file content is loaded into context.",
                        file_path
                    ),
                    is_error: true,
                };
            }
            // Staleness check: compare cached mtime with current disk mtime.
            let cached_mtime = cached.map(|s| s.mtime_ms);
            let disk_mtime = file_mtime_ms(path);
            if let (Some(cached_mt), Some(disk_mt)) = (cached_mtime, disk_mtime)
                && cached_mt != disk_mt
            {
                return ToolResult {
                    content: format!(
                        "File {} has been modified externally since last read. \
                         Read the file again to see the current content before editing.",
                        file_path
                    ),
                    is_error: true,
                };
            }
        }

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

        // Post-write cache update: refresh mtime and content.
        if let Some(cache_arc) = &self.file_cache {
            update_cache_after_write(cache_arc, path, &new_content);
        }

        ToolResult {
            content: format!("Edited {}: replaced {} occurrence(s)", file_path, match_count),
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
#[path = "edit_test.rs"]
mod edit_test;
