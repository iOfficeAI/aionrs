use super::*;

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
        assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "goodbye world");
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
        assert!(!result.is_error, "expected success without cache: {}", result.content);
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
