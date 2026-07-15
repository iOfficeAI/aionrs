use std::path::Path;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

use aion_protocol::events::ToolCategory;
use aion_types::message::{ContentBlock, ImageUrl, extension_to_image_media_type};
use aion_types::tool::{JsonSchema, ToolResult};

use crate::{Tool, ToolExecutionOutput};

const MAX_IMAGE_SIZE_BYTES: u64 = 20 * 1024 * 1024;

pub struct ViewImageTool;

impl ViewImageTool {
    pub fn new() -> Self {
        Self
    }

    async fn load_image(&self, input: &Value) -> Result<ImageUrl, String> {
        let file_path = input
            .get("file_path")
            .and_then(Value::as_str)
            .filter(|path| !path.trim().is_empty())
            .ok_or_else(|| "Missing required parameter: file_path".to_owned())?;
        let path = Path::new(file_path);
        if !path.is_absolute() {
            return Err("file_path must be an absolute path".to_owned());
        }

        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .ok_or_else(|| "Image path must have a supported extension".to_owned())?;
        let mime_type = extension_to_image_media_type(extension)
            .ok_or_else(|| format!("Unsupported image extension: {extension}"))?;

        let metadata = tokio::fs::metadata(path)
            .await
            .map_err(|error| format!("Failed to read image metadata: {error}"))?;
        if !metadata.is_file() {
            return Err("Image path is not a regular file".to_owned());
        }
        if metadata.len() > MAX_IMAGE_SIZE_BYTES {
            return Err(format!("Image exceeds the {} byte size limit", MAX_IMAGE_SIZE_BYTES));
        }

        let bytes = tokio::fs::read(path)
            .await
            .map_err(|error| format!("Failed to read image: {error}"))?;
        if bytes.len() as u64 > MAX_IMAGE_SIZE_BYTES {
            return Err(format!("Image exceeds the {} byte size limit", MAX_IMAGE_SIZE_BYTES));
        }

        let image_url = ImageUrl {
            url: format!("data:{mime_type};base64,{}", STANDARD.encode(bytes)),
        };
        image_url
            .validate()
            .map_err(|error| format!("Failed to prepare image input: {error}"))?;
        Ok(image_url)
    }

    fn success_result(file_path: &str) -> ToolResult {
        ToolResult {
            content: format!("Image loaded from {file_path} and attached to the next model turn."),
            is_error: false,
        }
    }

    fn error_result(error: String) -> ToolResult {
        ToolResult {
            content: error,
            is_error: true,
        }
    }
}

impl Default for ViewImageTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ViewImageTool {
    fn name(&self) -> &str {
        "ViewImage"
    }

    fn description(&self) -> &str {
        "Loads an image from an absolute local file path and attaches it to the next model turn. Use this when you need to inspect an image attachment."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to a JPEG, PNG, GIF, or WebP image"
                }
            },
            "required": ["file_path"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let file_path = input.get("file_path").and_then(Value::as_str).unwrap_or("unknown");
        match self.load_image(&input).await {
            Ok(_) => Self::success_result(file_path),
            Err(error) => Self::error_result(error),
        }
    }

    async fn execute_with_follow_up(&self, input: Value) -> ToolExecutionOutput {
        let file_path = input.get("file_path").and_then(Value::as_str).unwrap_or("unknown");
        match self.load_image(&input).await {
            Ok(image_url) => ToolExecutionOutput {
                result: Self::success_result(file_path),
                follow_up_blocks: vec![ContentBlock::Image { image_url }],
            },
            Err(error) => Self::error_result(error).into(),
        }
    }

    fn requires_image_input(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    fn describe(&self, input: &Value) -> String {
        let path = input.get("file_path").and_then(Value::as_str).unwrap_or("unknown");
        format!("View image {path}")
    }
}

#[cfg(test)]
#[path = "view_image_test.rs"]
mod view_image_test;
