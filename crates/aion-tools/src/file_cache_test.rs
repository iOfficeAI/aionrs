use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(max_entries: usize, max_size_bytes: usize) -> FileCacheConfig {
        FileCacheConfig {
            max_entries,
            max_size_bytes,
            enabled: true,
        }
    }

    fn make_state(content: &str, mtime_ms: u64) -> FileState {
        FileState {
            content: content.to_string(),
            mtime_ms,
            offset: None,
            limit: None,
        }
    }

    // -- normalize_path tests --

    #[test]
    fn normalize_resolves_parent_dir() {
        let result = normalize_path(Path::new("/a/../b/file"));
        assert_eq!(result, PathBuf::from("/b/file"));
    }

    #[test]
    fn normalize_resolves_cur_dir() {
        let result = normalize_path(Path::new("/a/./b/file"));
        assert_eq!(result, PathBuf::from("/a/b/file"));
    }

    #[test]
    fn normalize_above_root_is_clamped() {
        let result = normalize_path(Path::new("/../b"));
        assert_eq!(result, PathBuf::from("/b"));
    }

    #[test]
    fn normalize_preserves_leading_parent_in_relative() {
        let result = normalize_path(Path::new("../../foo"));
        assert_eq!(result, PathBuf::from("../../foo"));
    }

    #[test]
    fn normalize_mixed() {
        let result = normalize_path(Path::new("a/./b/../c"));
        assert_eq!(result, PathBuf::from("a/c"));
    }

    #[test]
    fn normalize_absolute_identity() {
        let result = normalize_path(Path::new("/usr/local/bin"));
        assert_eq!(result, PathBuf::from("/usr/local/bin"));
    }

    // -- FileStateCache core tests --

    #[test]
    fn insert_and_get() {
        let config = make_config(10, 1_000_000);
        let mut cache = FileStateCache::new(&config);

        let path = PathBuf::from("/tmp/test.rs");
        let state = make_state("hello", 1000);

        cache.insert(path.clone(), state);
        let got = cache.get(&path).unwrap();
        assert_eq!(got.content, "hello");
        assert_eq!(got.mtime_ms, 1000);
    }

    #[test]
    fn get_nonexistent_returns_none() {
        let config = make_config(10, 1_000_000);
        let mut cache = FileStateCache::new(&config);
        assert!(cache.get(Path::new("/does/not/exist")).is_none());
    }

    #[test]
    fn lru_eviction_by_count() {
        let config = make_config(3, 1_000_000);
        let mut cache = FileStateCache::new(&config);

        cache.insert(PathBuf::from("/a"), make_state("a", 1));
        cache.insert(PathBuf::from("/b"), make_state("b", 2));
        cache.insert(PathBuf::from("/c"), make_state("c", 3));
        // Cache is at capacity (3). Inserting a 4th evicts the LRU (/a).
        cache.insert(PathBuf::from("/d"), make_state("d", 4));

        assert!(cache.get(Path::new("/a")).is_none(), "/a should be evicted");
        assert!(cache.get(Path::new("/b")).is_some());
        assert!(cache.get(Path::new("/c")).is_some());
        assert!(cache.get(Path::new("/d")).is_some());
        assert_eq!(cache.len(), 3);
    }

    #[test]
    fn path_normalization_hits_same_slot() {
        let config = make_config(10, 1_000_000);
        let mut cache = FileStateCache::new(&config);

        cache.insert(PathBuf::from("/a/../b/file"), make_state("v1", 100));
        let got = cache.get(Path::new("/b/file")).unwrap();
        assert_eq!(got.content, "v1");
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn clear_removes_all() {
        let config = make_config(10, 1_000_000);
        let mut cache = FileStateCache::new(&config);

        cache.insert(PathBuf::from("/a"), make_state("a", 1));
        cache.insert(PathBuf::from("/b"), make_state("b", 2));
        assert_eq!(cache.len(), 2);

        cache.clear();
        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
        assert_eq!(cache.current_size_bytes(), 0);
    }

    #[test]
    fn remove_deletes_entry() {
        let config = make_config(10, 1_000_000);
        let mut cache = FileStateCache::new(&config);

        cache.insert(PathBuf::from("/a"), make_state("a-content", 1));
        let removed = cache.remove(Path::new("/a"));
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().content, "a-content");
        assert!(cache.get(Path::new("/a")).is_none());
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.current_size_bytes(), 0);
    }

    #[test]
    fn byte_size_eviction() {
        // max_size_bytes = 10, each entry ~5 bytes ("aaaaa").
        let config = make_config(100, 10);
        let mut cache = FileStateCache::new(&config);

        cache.insert(PathBuf::from("/a"), make_state("aaaaa", 1)); // 5 bytes
        cache.insert(PathBuf::from("/b"), make_state("bbbbb", 2)); // 5 bytes -> total 10
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.current_size_bytes(), 10);

        // Inserting /c (5 bytes) would exceed 10 -> evicts /a (LRU)
        cache.insert(PathBuf::from("/c"), make_state("ccccc", 3));
        assert!(cache.get(Path::new("/a")).is_none(), "/a should be evicted");
        assert!(cache.get(Path::new("/b")).is_some());
        assert!(cache.get(Path::new("/c")).is_some());
        assert_eq!(cache.current_size_bytes(), 10);
    }

    #[test]
    fn overwrite_same_key() {
        let config = make_config(10, 1_000_000);
        let mut cache = FileStateCache::new(&config);

        cache.insert(PathBuf::from("/a"), make_state("v1", 100));
        cache.insert(PathBuf::from("/a"), make_state("v2-longer", 200));

        let got = cache.get(Path::new("/a")).unwrap();
        assert_eq!(got.content, "v2-longer");
        assert_eq!(got.mtime_ms, 200);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.current_size_bytes(), "v2-longer".len());
    }

    #[test]
    fn size_accounting_after_remove() {
        let config = make_config(10, 1_000_000);
        let mut cache = FileStateCache::new(&config);

        cache.insert(PathBuf::from("/a"), make_state("hello", 1)); // 5 bytes
        cache.insert(PathBuf::from("/b"), make_state("world!", 2)); // 6 bytes
        assert_eq!(cache.current_size_bytes(), 11);

        cache.remove(Path::new("/a"));
        assert_eq!(cache.current_size_bytes(), 6);
    }

    #[test]
    fn zero_max_entries_defaults_to_100() {
        let config = make_config(0, 1_000_000);
        let mut cache = FileStateCache::new(&config);
        // Should not panic; defaults to capacity 100.
        for i in 0..100 {
            cache.insert(PathBuf::from(format!("/f{}", i)), make_state("x", i as u64));
        }
        assert_eq!(cache.len(), 100);
    }

    #[test]
    fn get_promotes_entry_preventing_eviction() {
        let config = make_config(3, 1_000_000);
        let mut cache = FileStateCache::new(&config);

        cache.insert(PathBuf::from("/a"), make_state("a", 1));
        cache.insert(PathBuf::from("/b"), make_state("b", 2));
        cache.insert(PathBuf::from("/c"), make_state("c", 3));

        // Access /a to promote it; now /b is the LRU.
        cache.get(Path::new("/a"));

        // Insert /d -> evicts /b (LRU), not /a.
        cache.insert(PathBuf::from("/d"), make_state("d", 4));
        assert!(cache.get(Path::new("/a")).is_some(), "/a should survive");
        assert!(cache.get(Path::new("/b")).is_none(), "/b should be evicted");
    }

    #[test]
    fn empty_content_cached() {
        let config = make_config(10, 1_000_000);
        let mut cache = FileStateCache::new(&config);

        cache.insert(PathBuf::from("/empty"), make_state("", 1));
        assert!(cache.get(Path::new("/empty")).is_some());
        assert_eq!(cache.current_size_bytes(), 0);
    }

    #[test]
    fn partial_read_state_preserved() {
        let config = make_config(10, 1_000_000);
        let mut cache = FileStateCache::new(&config);

        let state = FileState {
            content: "partial content".to_string(),
            mtime_ms: 500,
            offset: Some(10),
            limit: Some(20),
        };
        cache.insert(PathBuf::from("/file"), state);
        let got = cache.get(Path::new("/file")).unwrap();
        assert_eq!(got.offset, Some(10));
        assert_eq!(got.limit, Some(20));
    }
}
