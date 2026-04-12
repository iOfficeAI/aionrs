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
        let path = input
            .get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        format!("Edit {}", path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    use crate::file_cache::update_cache_after_write;
    use aion_config::file_cache::FileCacheConfig;

    fn make_cache() -> Arc<RwLock<FileStateCache>> {
        let config = FileCacheConfig {
            max_entries: 100,
            max_size_bytes: 25 * 1024 * 1024,
            enabled: true,
        };
        Arc::new(RwLock::new(FileStateCache::new(&config)))
    }

    /// Simulate a Read by inserting a cache entry for the given file path.
    fn simulate_read(cache: &Arc<RwLock<FileStateCache>>, path: &Path) {
        let content = std::fs::read_to_string(path).unwrap_or_default();
        update_cache_after_write(cache, path, &content);
    }

    // -- Legacy tests (no cache) --

    #[tokio::test]
    async fn test_edit_replace_block() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "hello world").unwrap();

        let tool = EditTool::new(None);
        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "old_string": "hello",
            "new_string": "goodbye"
        });

        let result = tool.execute(input).await;

        assert!(!result.is_error, "unexpected error: {}", result.content);
        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "goodbye world");
    }

    #[tokio::test]
    async fn test_edit_old_string_not_found() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "hello world").unwrap();

        let tool = EditTool::new(None);
        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "old_string": "nonexistent",
            "new_string": "replacement"
        });

        let result = tool.execute(input).await;

        assert!(result.is_error);
        assert!(
            result.content.contains("not found"),
            "expected 'not found' in error message, got: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn test_edit_preserves_surrounding() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "aaa\nbbb\nccc\n").unwrap();

        let tool = EditTool::new(None);
        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "old_string": "bbb",
            "new_string": "XXX"
        });

        let result = tool.execute(input).await;

        assert!(!result.is_error, "unexpected error: {}", result.content);
        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "aaa\nXXX\nccc\n");
    }

    #[tokio::test]
    async fn test_edit_nonexistent_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("does_not_exist.txt");

        let tool = EditTool::new(None);
        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "old_string": "anything",
            "new_string": "replacement"
        });

        let result = tool.execute(input).await;

        assert!(result.is_error);
        assert!(
            result.content.contains("Failed to read file"),
            "expected read failure message, got: {}",
            result.content
        );
    }

    // -- Cache guard tests --

    #[tokio::test]
    async fn edit_without_read_returns_error() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("unread.txt");
        std::fs::write(&file_path, "hello").unwrap();

        let cache = make_cache();
        let tool = EditTool::new(Some(cache));

        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "old_string": "hello",
            "new_string": "bye"
        });

        let result = tool.execute(input).await;

        assert!(result.is_error);
        assert!(
            result.content.contains("must Read"),
            "expected 'must Read' in error: {}",
            result.content
        );
        // File must be unchanged.
        assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "hello");
    }

    #[tokio::test]
    async fn edit_after_read_succeeds() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("read_then_edit.txt");
        std::fs::write(&file_path, "hello world").unwrap();

        let cache = make_cache();
        simulate_read(&cache, &file_path);

        let tool = EditTool::new(Some(cache));
        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "old_string": "hello",
            "new_string": "goodbye"
        });

        let result = tool.execute(input).await;

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert_eq!(
            std::fs::read_to_string(&file_path).unwrap(),
            "goodbye world"
        );
    }

    #[tokio::test]
    async fn edit_detects_external_modification() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("stale.txt");
        std::fs::write(&file_path, "original").unwrap();

        let cache = make_cache();
        simulate_read(&cache, &file_path);

        // External modification: change file after caching.
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&file_path, "externally changed").unwrap();

        let tool = EditTool::new(Some(cache));
        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "old_string": "original",
            "new_string": "new"
        });

        let result = tool.execute(input).await;

        assert!(result.is_error);
        assert!(
            result.content.contains("modified externally"),
            "expected staleness error: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn edit_then_edit_succeeds_via_cache_update() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("double_edit.txt");
        std::fs::write(&file_path, "aaa bbb ccc").unwrap();

        let cache = make_cache();
        simulate_read(&cache, &file_path);

        let tool = EditTool::new(Some(cache));

        // First edit.
        let input1 = json!({
            "file_path": file_path.to_str().unwrap(),
            "old_string": "aaa",
            "new_string": "AAA"
        });
        let r1 = tool.execute(input1).await;
        assert!(!r1.is_error, "first edit failed: {}", r1.content);

        // Second edit should succeed because first edit updated the cache.
        let input2 = json!({
            "file_path": file_path.to_str().unwrap(),
            "old_string": "bbb",
            "new_string": "BBB"
        });
        let r2 = tool.execute(input2).await;
        assert!(!r2.is_error, "second edit failed: {}", r2.content);
        assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "AAA BBB ccc");
    }

    #[tokio::test]
    async fn no_cache_edit_bypasses_guard() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("nocache.txt");
        std::fs::write(&file_path, "hello").unwrap();

        let tool = EditTool::new(None);
        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "old_string": "hello",
            "new_string": "bye"
        });

        let result = tool.execute(input).await;
        assert!(
            !result.is_error,
            "expected success without cache: {}",
            result.content
        );
        assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "bye");
    }

    #[tokio::test]
    async fn replace_all_updates_cache() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("replaceall.txt");
        std::fs::write(&file_path, "a-a-a").unwrap();

        let cache = make_cache();
        simulate_read(&cache, &file_path);

        let tool = EditTool::new(Some(cache.clone()));
        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "old_string": "a",
            "new_string": "b",
            "replace_all": true
        });

        let result = tool.execute(input).await;
        assert!(!result.is_error, "replace_all failed: {}", result.content);

        // Verify cache was updated: mtime should match current disk mtime.
        let disk_mtime = file_mtime_ms(&file_path).unwrap();
        let mut c = cache.write().unwrap();
        let cached = c.get(&file_path).expect("file should be in cache");
        assert_eq!(cached.mtime_ms, disk_mtime);
    }
}
