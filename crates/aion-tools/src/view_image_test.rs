use std::fs;

use serde_json::json;
use tempfile::TempDir;

use aion_types::message::ContentBlock;

use super::ViewImageTool;
use crate::Tool;

#[tokio::test]
async fn returns_image_as_follow_up_block() {
    let directory = TempDir::new().expect("temp dir");
    let path = directory.path().join("sample.png");
    fs::write(&path, b"fake-png").expect("write image fixture");
    let tool = ViewImageTool::new();

    let output = tool.execute_with_follow_up(json!({ "file_path": path })).await;

    assert!(!output.result.is_error);
    assert_eq!(output.follow_up_blocks.len(), 1);
    assert!(matches!(
        &output.follow_up_blocks[0],
        ContentBlock::Image { image_url }
            if image_url.url.starts_with("data:image/png;base64,")
    ));
}

#[tokio::test]
async fn rejects_relative_paths_without_follow_up() {
    let tool = ViewImageTool::new();

    let output = tool
        .execute_with_follow_up(json!({ "file_path": "relative.png" }))
        .await;

    assert!(output.result.is_error);
    assert!(output.result.content.contains("absolute path"));
    assert!(output.follow_up_blocks.is_empty());
}

#[tokio::test]
async fn rejects_unsupported_image_extensions() {
    let directory = TempDir::new().expect("temp dir");
    let path = directory.path().join("sample.svg");
    fs::write(&path, b"<svg/>").expect("write image fixture");
    let tool = ViewImageTool::new();

    let output = tool.execute_with_follow_up(json!({ "file_path": path })).await;

    assert!(output.result.is_error);
    assert!(output.result.content.contains("Unsupported image extension"));
    assert!(output.follow_up_blocks.is_empty());
}
