use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn grep_tool_finds_pattern_in_own_source() {
        let tool = GrepTool::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
        let input = json!({
            "pattern": "GrepTool",
            "path": env!("CARGO_MANIFEST_DIR")
        });
        let result = tool.execute(input).await;
        assert!(!result.is_error, "grep failed: {}", result.content);
        assert!(result.content.contains("GrepTool"));
    }

    #[tokio::test]
    async fn execute_uses_cwd_for_relative_path() {
        use std::fs;
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("searchable.txt"), "unique_grep_marker_xyz").unwrap();

        let tool = GrepTool::new(tmp.path().to_path_buf());
        let input = json!({"pattern": "unique_grep_marker_xyz", "path": "."});
        let result = tool.execute(input).await;
        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(
            result.content.contains("unique_grep_marker_xyz"),
            "should find pattern, got: {}",
            result.content
        );
    }
}
