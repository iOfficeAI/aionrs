use crate::types::message::{ContentBlock, Message, Role};

/// Build the system prompt from config and environment
pub fn build_system_prompt(custom_prompt: Option<&str>, cwd: &str) -> String {
    let mut parts = Vec::new();

    parts.push(format!(
        "You are an AI assistant that can use tools to help with tasks.\n\
         Working directory: {cwd}\n\
         Current date: {}",
        chrono::Local::now().format("%Y-%m-%d")
    ));

    if let Some(custom) = custom_prompt {
        parts.push(custom.to_string());
    }

    // Read CLAUDE.md if it exists
    let claude_md = std::path::Path::new(cwd).join("CLAUDE.md");
    if claude_md.exists() {
        if let Ok(content) = std::fs::read_to_string(&claude_md) {
            parts.push(format!("# Project Instructions (CLAUDE.md)\n\n{content}"));
        }
    }

    parts.join("\n\n")
}

/// Compact old messages to reduce context size.
/// Keeps first message (user input) and last `keep_tail` messages,
/// replaces middle with a summary.
pub fn compact_messages(messages: &mut Vec<Message>, keep_tail: usize) {
    let min_messages = keep_tail + 2; // first + summary + tail
    if messages.len() <= min_messages {
        return;
    }

    let tail_start = messages.len() - keep_tail;
    let summarized_count = tail_start - 1;

    let summary_text = format!(
        "[Previous conversation summary: {} messages exchanged, \
         including tool calls and results. Key context preserved in recent messages.]",
        summarized_count
    );

    let summary_msg = Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: summary_text,
        }],
    };

    let tail: Vec<Message> = messages.drain(tail_start..).collect();
    messages.truncate(1); // keep first message
    messages.push(summary_msg);
    messages.extend(tail);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compact_messages_too_few() {
        let mut messages = vec![
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "hello".to_string(),
                }],
            },
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text {
                    text: "hi".to_string(),
                }],
            },
        ];
        compact_messages(&mut messages, 4);
        assert_eq!(messages.len(), 2); // no change
    }

    #[test]
    fn test_compact_messages() {
        let mut messages: Vec<Message> = (0..10)
            .map(|i| Message {
                role: if i % 2 == 0 { Role::User } else { Role::Assistant },
                content: vec![ContentBlock::Text {
                    text: format!("msg {}", i),
                }],
            })
            .collect();

        compact_messages(&mut messages, 4);
        // first + summary + 4 tail = 6
        assert_eq!(messages.len(), 6);
        assert_eq!(messages[0].role, Role::User);
        // Second message should be the summary
        if let ContentBlock::Text { text } = &messages[1].content[0] {
            assert!(text.contains("summary"));
        }
    }

    #[test]
    fn test_build_system_prompt_includes_cwd() {
        // Verify that the returned prompt contains the provided working directory path
        let cwd = "/some/test/path";
        let prompt = build_system_prompt(None, cwd);
        assert!(prompt.contains(cwd), "system prompt should contain the cwd");
    }

    #[test]
    fn test_build_system_prompt_with_custom_instructions() {
        // Verify that custom instructions are included in the returned prompt
        let custom = "Always respond in haiku.";
        let prompt = build_system_prompt(Some(custom), "/tmp");
        assert!(
            prompt.contains(custom),
            "system prompt should contain the custom instructions"
        );
    }

    #[test]
    fn test_compact_messages_preserves_first_and_last() {
        // Build 8 messages (indices 0–7); keep_tail = 3
        let mut messages: Vec<Message> = (0..8)
            .map(|i| Message {
                role: if i % 2 == 0 { Role::User } else { Role::Assistant },
                content: vec![ContentBlock::Text {
                    text: format!("msg {}", i),
                }],
            })
            .collect();

        compact_messages(&mut messages, 3);

        // First message must be unchanged
        if let ContentBlock::Text { text } = &messages[0].content[0] {
            assert_eq!(text, "msg 0");
        } else {
            panic!("first message content block is not Text");
        }

        // Last message must be the original last message (index 7)
        let last = messages.last().expect("messages should not be empty");
        if let ContentBlock::Text { text } = &last.content[0] {
            assert_eq!(text, "msg 7");
        } else {
            panic!("last message content block is not Text");
        }
    }

    #[test]
    fn test_compact_messages_boundary_count() {
        // When the message count equals min_messages (keep_tail + 2), no compaction occurs
        let keep_tail = 4;
        let min_messages = keep_tail + 2; // = 6
        let mut messages: Vec<Message> = (0..min_messages)
            .map(|i| Message {
                role: if i % 2 == 0 { Role::User } else { Role::Assistant },
                content: vec![ContentBlock::Text {
                    text: format!("msg {}", i),
                }],
            })
            .collect();

        compact_messages(&mut messages, keep_tail);

        // Exactly at the boundary: no modification expected
        assert_eq!(
            messages.len(),
            min_messages,
            "messages at boundary should not be compacted"
        );
    }
}
