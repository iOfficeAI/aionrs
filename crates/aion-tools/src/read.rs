use std::path::Path;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde_json::{Value, json};

use aion_protocol::events::ToolCategory;
use aion_types::file_state::FileState;
use aion_types::tool::{JsonSchema, ToolResult};

use crate::Tool;
use crate::file_cache::{FileStateCache, file_mtime_ms};

/// Stub returned when a file has not changed since the model last read it.
/// Saves tokens by avoiding re-sending identical content.
const FILE_UNCHANGED_STUB: &str = "File unchanged since last read. The content from the earlier Read \
     tool_result in this conversation is still current — refer to that \
     instead of re-reading.";

pub struct ReadTool {
    file_cache: Option<Arc<RwLock<FileStateCache>>>,
}

impl ReadTool {
    /// Create a ReadTool with optional file state cache for dedup.
    ///
    /// Pass `None` to disable caching (all reads return full content).
    pub fn new(file_cache: Option<Arc<RwLock<FileStateCache>>>) -> Self {
        Self { file_cache }
    }
}

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str {
        "Read"
    }

    fn description(&self) -> &str {
        "Reads a file from the local filesystem. Returns content with line numbers.\n\n\
         Usage:\n\
         - The file_path parameter must be an absolute path, not a relative path.\n\
         - By default, it reads the entire file. Use offset and limit for partial reads on large files.\n\
         - Results are returned with line numbers (1-based) followed by a tab and the line content.\n\
         - Binary files return \"(binary file, N bytes)\" instead of content.\n\
         - This tool can only read files, not directories. To list a directory, use Bash with ls."
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

        let offset = input["offset"].as_u64().map(|v| v as usize);
        let limit = input["limit"].as_u64().map(|v| v as usize);

        // Get file mtime for dedup and cache.
        let mtime_ms = file_mtime_ms(Path::new(file_path));

        // Dedup check: if cache has the same file with matching offset/limit and mtime,
        // return a short stub instead of full content.
        if let (Some(cache_arc), Some(current_mtime)) = (&self.file_cache, mtime_ms)
            && let Ok(mut cache) = cache_arc.write()
            && let Some(cached) = cache.get(Path::new(file_path))
            && cached.offset == offset
            && cached.limit == limit
            && cached.mtime_ms == current_mtime
        {
            return ToolResult {
                content: FILE_UNCHANGED_STUB.to_string(),
                is_error: false,
            };
        }

        // Read file from disk.
        let content = match std::fs::read(file_path) {
            Ok(bytes) => bytes,
            Err(e) => {
                return ToolResult {
                    content: format!("Failed to read file {}: {}", file_path, e),
                    is_error: true,
                };
            }
        };

        // Check if binary.
        if content.iter().take(8192).any(|&b| b == 0) {
            return ToolResult {
                content: format!("(binary file, {} bytes)", content.len()),
                is_error: false,
            };
        }

        let text = String::from_utf8_lossy(&content);
        let lines: Vec<&str> = text.lines().collect();

        let effective_offset = offset.unwrap_or(0);
        let effective_limit = limit.unwrap_or(lines.len());

        let end = (effective_offset + effective_limit).min(lines.len());
        let slice = &lines[effective_offset.min(lines.len())..end];

        let numbered: Vec<String> = slice
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{:>6}\t{}", effective_offset + i + 1, line))
            .collect();

        let result_content = numbered.join("\n");

        // Update cache after successful read.
        if let Some(cache_arc) = &self.file_cache
            && let (Ok(mut cache), Some(mtime)) = (cache_arc.write(), mtime_ms)
        {
            cache.insert(
                file_path.into(),
                FileState {
                    content: result_content.clone(),
                    mtime_ms: mtime,
                    offset,
                    limit,
                },
            );
        }

        ToolResult {
            content: result_content,
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
        let path = input
            .get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        format!("Read {}", path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;
    use tempfile::tempdir;

    use aion_config::file_cache::FileCacheConfig;

    fn make_cache() -> Arc<RwLock<FileStateCache>> {
        let config = FileCacheConfig {
            max_entries: 100,
            max_size_bytes: 25 * 1024 * 1024,
            enabled: true,
        };
        Arc::new(RwLock::new(FileStateCache::new(&config)))
    }

    // -- Basic read tests (no cache) --

    #[tokio::test]
    async fn test_read_file_full() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        let mut file = std::fs::File::create(&file_path).unwrap();
        writeln!(file, "line one").unwrap();
        writeln!(file, "line two").unwrap();
        writeln!(file, "line three").unwrap();
        drop(file);

        let tool = ReadTool::new(None);
        let input = json!({ "file_path": file_path.to_str().unwrap() });
        let result = tool.execute(input).await;

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

        let tool = ReadTool::new(None);
        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "offset": 2,
            "limit": 3
        });
        let result = tool.execute(input).await;

        assert!(!result.is_error);
        let lines: Vec<&str> = result.content.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("3\tline 3"));
        assert!(lines[1].contains("4\tline 4"));
        assert!(lines[2].contains("5\tline 5"));
    }

    #[tokio::test]
    async fn test_read_nonexistent_file() {
        let tool = ReadTool::new(None);
        let input = json!({ "file_path": "/tmp/nonexistent_file_abc123.txt" });
        let result = tool.execute(input).await;

        assert!(result.is_error);
        assert!(result.content.contains("Failed to read file"));
    }

    #[tokio::test]
    async fn test_read_empty_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("empty.txt");
        std::fs::File::create(&file_path).unwrap();

        let tool = ReadTool::new(None);
        let input = json!({ "file_path": file_path.to_str().unwrap() });
        let result = tool.execute(input).await;

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

        let tool = ReadTool::new(None);
        let input = json!({ "file_path": file_path.to_str().unwrap() });
        let result = tool.execute(input).await;

        assert!(!result.is_error);
        let lines: Vec<&str> = result.content.lines().collect();
        assert_eq!(lines.len(), 200);
        assert!(lines[0].contains("1\tline number 1"));
        assert!(lines[199].contains("200\tline number 200"));
    }

    // -- Dedup tests (with cache) --

    #[tokio::test]
    async fn dedup_returns_stub_on_unchanged_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("dedup.txt");
        std::fs::write(&file_path, "hello\n").unwrap();

        let cache = make_cache();
        let tool = ReadTool::new(Some(cache));

        let input = json!({ "file_path": file_path.to_str().unwrap() });

        // First read: full content.
        let r1 = tool.execute(input.clone()).await;
        assert!(!r1.is_error);
        assert!(r1.content.contains("hello"));

        // Second read: dedup stub.
        let r2 = tool.execute(input).await;
        assert!(!r2.is_error);
        assert_eq!(r2.content, FILE_UNCHANGED_STUB);
    }

    #[tokio::test]
    async fn dedup_returns_new_content_after_modification() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("modified.txt");
        std::fs::write(&file_path, "version1\n").unwrap();

        let cache = make_cache();
        let tool = ReadTool::new(Some(cache));

        let input = json!({ "file_path": file_path.to_str().unwrap() });

        let r1 = tool.execute(input.clone()).await;
        assert!(r1.content.contains("version1"));

        // Modify the file — ensure mtime changes.
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&file_path, "version2\n").unwrap();

        let r2 = tool.execute(input).await;
        assert!(!r2.is_error);
        assert!(r2.content.contains("version2"));
    }

    #[tokio::test]
    async fn dedup_different_offset_limit_returns_full() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("multi.txt");
        let mut file = std::fs::File::create(&file_path).unwrap();
        for i in 1..=20 {
            writeln!(file, "line {}", i).unwrap();
        }
        drop(file);

        let cache = make_cache();
        let tool = ReadTool::new(Some(cache));

        let input1 = json!({
            "file_path": file_path.to_str().unwrap(),
            "offset": 0,
            "limit": 10
        });
        let r1 = tool.execute(input1).await;
        assert!(!r1.is_error);
        assert!(r1.content.contains("line 1"));

        // Different range: should return full content, not stub.
        let input2 = json!({
            "file_path": file_path.to_str().unwrap(),
            "offset": 10,
            "limit": 10
        });
        let r2 = tool.execute(input2).await;
        assert!(!r2.is_error);
        assert!(r2.content.contains("line 11"));
        assert!(!r2.content.contains(FILE_UNCHANGED_STUB));
    }

    #[tokio::test]
    async fn no_cache_always_returns_full_content() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("nocache.txt");
        std::fs::write(&file_path, "data\n").unwrap();

        let tool = ReadTool::new(None);
        let input = json!({ "file_path": file_path.to_str().unwrap() });

        let r1 = tool.execute(input.clone()).await;
        assert!(r1.content.contains("data"));

        let r2 = tool.execute(input).await;
        assert!(r2.content.contains("data"));
        assert_ne!(r2.content, FILE_UNCHANGED_STUB);
    }

    #[tokio::test]
    async fn nonexistent_file_not_cached() {
        let cache = make_cache();
        let tool = ReadTool::new(Some(cache.clone()));

        let input = json!({ "file_path": "/tmp/nonexistent_xyz_789.txt" });
        let r = tool.execute(input).await;
        assert!(r.is_error);

        // Cache should be empty.
        let c = cache.read().unwrap();
        assert!(c.is_empty());
    }

    #[tokio::test]
    async fn dedup_empty_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("empty.txt");
        std::fs::File::create(&file_path).unwrap();

        let cache = make_cache();
        let tool = ReadTool::new(Some(cache));

        let input = json!({ "file_path": file_path.to_str().unwrap() });

        let r1 = tool.execute(input.clone()).await;
        assert!(!r1.is_error);

        let r2 = tool.execute(input).await;
        assert!(!r2.is_error);
        assert_eq!(r2.content, FILE_UNCHANGED_STUB);
    }
}
