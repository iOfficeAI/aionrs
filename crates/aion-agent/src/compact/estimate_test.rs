use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use aion_types::message::ImageUrl;
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;

    #[test]
    fn final_tool_result_is_estimated_directly() {
        let result = ContentBlock::ToolResult {
            tool_use_id: "c1".into(),
            content: "x".repeat(400_000),
            is_error: false,
        };

        assert_eq!(estimate_tokens_from_tool_result(&result), 100_000);
    }

    #[test]
    fn non_tool_result_is_not_counted_as_tool_output() {
        let text = ContentBlock::Text { text: "x".repeat(400) };

        assert_eq!(estimate_tokens_from_tool_result(&text), 0);
    }

    #[test]
    fn tool_image_uses_decoded_size_not_base64_length() {
        // 10_000 decoded bytes -> 10_000 / 750 = 13 tokens, clamped to minimum 85.
        let image_bytes = vec![0u8; 10_000];
        let data = STANDARD.encode(&image_bytes);
        let image = ContentBlock::Image {
            image_url: ImageUrl {
                url: format!("data:image/png;base64,{}", data),
            },
        };
        let estimate = estimate_tokens_from_tool_image(&image);

        assert_eq!(estimate, 85);
        // The old base64-length heuristic would have counted ~13_333 chars -> ~3_333 tokens.
        assert!(
            estimate < 1000,
            "image estimate should be much smaller than base64 length heuristic"
        );
    }

    #[test]
    fn tool_image_estimate_respects_maximum() {
        // A huge image should be capped, not grow with base64 length.
        let image_bytes = vec![0u8; 10_000_000];
        let data = STANDARD.encode(&image_bytes);
        let image = ContentBlock::Image {
            image_url: ImageUrl {
                url: format!("data:image/png;base64,{}", data),
            },
        };

        assert_eq!(estimate_tokens_from_tool_image(&image), 2048);
    }
}
