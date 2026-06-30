use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    use crate::Tool;
    use crate::file_cache::file_mtime_ms;
    use aion_config::file_cache::FileCacheConfig;

    fn make_cache() -> Arc<RwLock<FileStateCache>> {
        let config = FileCacheConfig {
            max_entries: 100,
            max_size_bytes: 25 * 1024 * 1024,
            enabled: true,
        };
        Arc::new(RwLock::new(FileStateCache::new(&config)))
    }

    // -- Legacy tests (no cache) --

    #[tokio::test]
    async fn test_write_new_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("hello.txt");

        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "content": "hello world"
        });

        let tool = WriteTool::new(None);
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

        let tool = WriteTool::new(None);
        let result = tool.execute(input).await;

        assert!(!result.is_error, "expected success, got: {}", result.content);
        assert!(file_path.parent().unwrap().exists(), "parent dirs should be created");
        assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "nested content");
    }

    #[tokio::test]
    async fn test_write_overwrite_existing() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("overwrite.txt");

        let tool = WriteTool::new(None);

        let input1 = json!({
            "file_path": file_path.to_str().unwrap(),
            "content": "original"
        });
        let result1 = tool.execute(input1).await;
        assert!(!result1.is_error);
        assert!(result1.content.contains("Created"));

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

        let tool = WriteTool::new(None);
        let result = tool.execute(input).await;

        assert!(!result.is_error, "expected success, got: {}", result.content);

        let read_back = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(
            read_back, content,
            "read-back content must exactly match written content"
        );
    }

    // -- Cache integration tests --

    #[tokio::test]
    async fn write_populates_cache() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("cached.txt");

        let cache = make_cache();
        let tool = WriteTool::new(Some(cache.clone()));

        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "content": "cached content"
        });
        let result = tool.execute(input).await;
        assert!(!result.is_error, "write failed: {}", result.content);

        // Cache should have an entry with correct mtime.
        let disk_mtime = file_mtime_ms(&file_path).unwrap();
        let mut c = cache.write().unwrap();
        let cached = c.get(&file_path).expect("file should be in cache after write");
        assert_eq!(cached.mtime_ms, disk_mtime);
        assert!(cached.content.contains("cached content"));
    }

    #[tokio::test]
    async fn write_then_edit_succeeds() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("write_edit.txt");

        let cache = make_cache();
        let write_tool = WriteTool::new(Some(cache.clone()));
        let edit_tool = crate::edit::EditTool::new(Some(cache));

        // Write creates the file and populates cache.
        let write_input = json!({
            "file_path": file_path.to_str().unwrap(),
            "content": "hello world"
        });
        let wr = write_tool.execute(write_input).await;
        assert!(!wr.is_error, "write failed: {}", wr.content);

        // Edit should succeed without needing a separate Read.
        let edit_input = json!({
            "file_path": file_path.to_str().unwrap(),
            "old_string": "hello",
            "new_string": "goodbye"
        });
        let er = edit_tool.execute(edit_input).await;
        assert!(!er.is_error, "edit after write failed: {}", er.content);
        assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "goodbye world");
    }

    #[tokio::test]
    async fn write_overwrite_updates_cache_mtime() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("overwrite_cache.txt");

        let cache = make_cache();
        let tool = WriteTool::new(Some(cache.clone()));

        // First write.
        let input1 = json!({
            "file_path": file_path.to_str().unwrap(),
            "content": "v1"
        });
        tool.execute(input1).await;

        let mtime1 = {
            let mut c = cache.write().unwrap();
            c.get(&file_path).unwrap().mtime_ms
        };

        // Brief delay to ensure mtime changes.
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Second write.
        let input2 = json!({
            "file_path": file_path.to_str().unwrap(),
            "content": "v2"
        });
        tool.execute(input2).await;

        let mtime2 = {
            let mut c = cache.write().unwrap();
            c.get(&file_path).unwrap().mtime_ms
        };

        assert!(mtime2 >= mtime1, "cache mtime should update after overwrite");
    }
}
