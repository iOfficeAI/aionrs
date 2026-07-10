use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Unique identifier for a tool call
pub type ToolUseId = String;

/// A single content block within a message
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    /// Plain text content
    #[serde(rename = "text")]
    Text { text: String },

    /// An image content block (base64 encoded data URI)
    #[serde(rename = "image_url")]
    Image { image_url: ImageUrl },

    /// A tool invocation from the assistant
    #[serde(rename = "tool_use")]
    ToolUse {
        id: ToolUseId,
        name: String,
        input: Value,
        /// Opaque provider-specific metadata (e.g. Gemini thought_signature).
        /// Round-tripped verbatim so the provider can include it in follow-up requests.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        extra: Option<Value>,
    },

    /// Result of a tool execution, sent back as user message
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: ToolUseId,
        content: String,
        is_error: bool,
    },

    /// Thinking / reasoning block. Serialized as `thinking` for Anthropic
    /// and as `reasoning_content` for OpenAI-compatible providers.
    #[serde(rename = "thinking")]
    Thinking {
        thinking: String,
        /// Opaque provider signature required when round-tripping Anthropic thinking blocks.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
}

/// Image URL for content blocks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageUrl {
    pub url: String,
}

/// Media types that are widely accepted as vision input by major providers.
pub const SUPPORTED_IMAGE_MEDIA_TYPES: &[&str] = &["image/jpeg", "image/png", "image/gif", "image/webp"];

/// Map a file extension to a supported image media type.
///
/// Returns `None` for extensions that are not reliably accepted as image
/// inputs by the supported providers (e.g. `svg`, `bmp`, `tiff`).
pub fn extension_to_image_media_type(ext: &str) -> Option<&'static str> {
    let ext = ext.trim_start_matches('.').to_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

/// Errors that can occur when validating an image data URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageUrlError {
    /// The URL is not a well-formed `data:` URI with a `;base64,` payload.
    InvalidFormat,
    /// The media type is missing or not in the supported image set.
    UnsupportedMediaType(String),
    /// The base64 payload could not be decoded.
    InvalidBase64,
}

impl std::fmt::Display for ImageUrlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidFormat => write!(f, "image URL is not a data URI with base64 payload"),
            Self::UnsupportedMediaType(mime) => {
                write!(f, "unsupported image media type: {mime}")
            }
            Self::InvalidBase64 => write!(f, "image base64 payload is invalid"),
        }
    }
}

impl std::error::Error for ImageUrlError {}

impl ImageUrl {
    /// Validate that this URL is a supported base64-encoded image data URI.
    pub fn validate(&self) -> Result<(), ImageUrlError> {
        let rest = self.url.strip_prefix("data:").ok_or(ImageUrlError::InvalidFormat)?;
        let (mime_and_params, _) = rest.split_once(",").ok_or(ImageUrlError::InvalidFormat)?;
        if !mime_and_params.ends_with(";base64") {
            return Err(ImageUrlError::InvalidFormat);
        }
        let mime = &mime_and_params[..mime_and_params.len() - ";base64".len()];
        if mime.is_empty() || !SUPPORTED_IMAGE_MEDIA_TYPES.contains(&mime) {
            return Err(ImageUrlError::UnsupportedMediaType(mime.to_string()));
        }
        let payload = &self.url[self.url.find(',').unwrap() + 1..];
        base64::engine::general_purpose::STANDARD
            .decode(payload)
            .map_err(|_| ImageUrlError::InvalidBase64)?;
        Ok(())
    }

    /// Return an estimate of the decoded byte size of the base64 payload.
    ///
    /// This is an upper-bound estimate returned by `base64::decoded_len_estimate`
    /// and is intended for cost heuristics. Returns `None` if the URL is not a
    /// well-formed base64 data URI.
    pub fn decoded_byte_size(&self) -> Option<usize> {
        let (_, payload) = self.url.strip_prefix("data:")?.split_once(",")?;
        Some(base64::decoded_len_estimate(payload.len()))
    }
}

/// A message in the conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
    /// When this message was created.  Used by microcompact to decide
    /// whether old tool results should be cleared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<DateTime<Utc>>,
}

impl Message {
    /// Create a message without a timestamp (backward-compatible default).
    pub fn new(role: Role, content: Vec<ContentBlock>) -> Self {
        Self {
            role,
            content,
            timestamp: None,
        }
    }

    /// Create a message stamped with the current UTC time.
    pub fn now(role: Role, content: Vec<ContentBlock>) -> Self {
        Self {
            role,
            content,
            timestamp: Some(Utc::now()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
    Tool,
}

/// Why the model stopped generating
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// Model finished naturally
    EndTurn,
    /// Model wants to call tools
    ToolUse,
    /// Hit max_tokens limit
    MaxTokens,
    /// Hit max_turns limit
    MaxTurns,
}

/// Token usage statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
}

#[cfg(test)]
#[path = "message_test.rs"]
mod message_test;
