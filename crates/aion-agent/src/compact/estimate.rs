use aion_types::message::{ContentBlock, ImageUrl, Message};

const CHARS_PER_TOKEN_TEXT: usize = 4;

const CHARS_PER_TOKEN_JSON: usize = 3;

/// Estimate the total token count for a slice of messages.
///
/// Intentionally conservative (slightly over-estimates) to ensure
/// compaction triggers rather than being skipped.
pub fn estimate_tokens_from_messages(messages: &[Message]) -> u64 {
    let mut total_chars: usize = 0;
    let mut json_chars: usize = 0;

    for msg in messages {
        for block in &msg.content {
            match block {
                ContentBlock::Text { text } => {
                    total_chars += text.len();
                }
                ContentBlock::Thinking { thinking, .. } => {
                    total_chars += thinking.len();
                }
                ContentBlock::ToolUse { name, input, .. } => {
                    let input_str = input.to_string();
                    json_chars += name.len() + input_str.len();
                }
                ContentBlock::ToolResult { content, .. } => {
                    total_chars += content.len();
                }
                ContentBlock::Image { image_url } => {
                    total_chars += estimate_image_tokens(image_url) as usize * CHARS_PER_TOKEN_TEXT;
                }
                ContentBlock::ProviderItem { item, .. } => {
                    json_chars += item.to_string().len();
                }
            }
        }
    }

    let text_tokens = total_chars / CHARS_PER_TOKEN_TEXT;
    let json_tokens = json_chars / CHARS_PER_TOKEN_JSON;

    (text_tokens + json_tokens) as u64
}

/// Estimate one final tool result that will be added after the provider's
/// exact usage measurement.
pub(crate) fn estimate_tokens_from_tool_result(block: &ContentBlock) -> u64 {
    match block {
        ContentBlock::ToolResult { content, .. } => (content.len() / CHARS_PER_TOKEN_TEXT) as u64,
        _ => 0,
    }
}

/// Estimate an image block emitted by a tool for the next provider request.
pub(crate) fn estimate_tokens_from_tool_image(block: &ContentBlock) -> u64 {
    match block {
        ContentBlock::Image { image_url } => estimate_image_tokens(image_url),
        _ => 0,
    }
}

fn estimate_image_tokens(image_url: &ImageUrl) -> u64 {
    // Image token cost is not proportional to base64 string length. Use a
    // provider-agnostic heuristic based on decoded byte size and clamp it to
    // reasonable per-image bounds.
    const BYTES_PER_TOKEN: usize = 750;
    const MIN_IMAGE_TOKENS: usize = 85;
    const MAX_IMAGE_TOKENS: usize = 2048;

    let bytes = image_url.decoded_byte_size().unwrap_or(0);
    (bytes / BYTES_PER_TOKEN).clamp(MIN_IMAGE_TOKENS, MAX_IMAGE_TOKENS) as u64
}

#[cfg(test)]
#[path = "estimate_test.rs"]
mod estimate_test;
