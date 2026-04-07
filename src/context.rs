use crate::skills::prompt::format_skills_within_budget;
use crate::skills::types::SkillMetadata;
use crate::types::message::{ContentBlock, Message, Role};

/// Build the system prompt from config and environment
pub fn build_system_prompt(
    custom_prompt: Option<&str>,
    cwd: &str,
    skills: &[SkillMetadata],
    context_window_tokens: Option<usize>,
) -> String {
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

    // Inject visible skill listing (exclude skills hidden from model invocation)
    let visible_skills: Vec<SkillMetadata> = skills
        .iter()
        .filter(|s| !s.disable_model_invocation)
        .cloned()
        .collect();

    if !visible_skills.is_empty() {
        let listing = format_skills_within_budget(&visible_skills, context_window_tokens);
        if !listing.is_empty() {
            parts.push(format!(
                "<system-reminder>\nThe following skills are available for use with the Skill tool:\n\n{listing}\n</system-reminder>"
            ));
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
        let prompt = build_system_prompt(None, cwd, &[], None);
        assert!(prompt.contains(cwd), "system prompt should contain the cwd");
    }

    #[test]
    fn test_build_system_prompt_with_custom_instructions() {
        // Verify that custom instructions are included in the returned prompt
        let custom = "Always respond in haiku.";
        let prompt = build_system_prompt(Some(custom), "/tmp", &[], None);
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

    // --- build_system_prompt Phase 9 tests ---

    use crate::skills::types::{ExecutionContext, LoadedFrom, SkillMetadata, SkillSource};

    fn make_test_skill(
        name: &str,
        description: &str,
        bundled: bool,
        hidden: bool,
    ) -> SkillMetadata {
        SkillMetadata {
            name: name.to_string(),
            display_name: None,
            description: description.to_string(),
            has_user_specified_description: false,
            allowed_tools: vec![],
            argument_hint: None,
            argument_names: vec![],
            when_to_use: None,
            version: None,
            model: None,
            disable_model_invocation: hidden,
            user_invocable: true,
            execution_context: ExecutionContext::Inline,
            agent: None,
            effort: None,
            shell: None,
            paths: vec![],
            hooks_raw: None,
            source: if bundled {
                SkillSource::Bundled
            } else {
                SkillSource::User
            },
            loaded_from: if bundled {
                LoadedFrom::Bundled
            } else {
                LoadedFrom::Skills
            },
            content: String::new(),
            content_length: 0,
            skill_root: None,
        }
    }

    #[test]
    fn test_build_system_prompt_no_skills_no_reminder() {
        let result = build_system_prompt(None, "/tmp", &[], None);
        assert!(
            !result.contains("The following skills are available"),
            "empty skills should not inject skill reminder"
        );
    }

    #[test]
    fn test_build_system_prompt_with_skills_injects_reminder() {
        let skills = vec![
            make_test_skill("skill-one", "Does one", false, false),
            make_test_skill("skill-two", "Does two", false, false),
        ];
        let result = build_system_prompt(None, "/tmp", &skills, None);
        assert!(
            result.contains("<system-reminder>"),
            "result should contain <system-reminder>"
        );
        assert!(
            result.contains("The following skills are available for use with the Skill tool:"),
            "result should contain skills header"
        );
        assert!(
            result.contains("</system-reminder>"),
            "result should close <system-reminder>"
        );
        assert!(result.contains("skill-one"), "result should list skill-one");
        assert!(result.contains("skill-two"), "result should list skill-two");
    }

    #[test]
    fn test_build_system_prompt_hidden_skill_filtered() {
        let skills = vec![
            make_test_skill("visible-skill", "Visible", false, false),
            make_test_skill("hidden-skill", "Hidden", false, true),
        ];
        let result = build_system_prompt(None, "/tmp", &skills, None);
        assert!(
            result.contains("visible-skill"),
            "visible skill should appear"
        );
        assert!(
            !result.contains("hidden-skill"),
            "hidden skill should be filtered out"
        );
    }

    #[test]
    fn test_build_system_prompt_all_hidden_no_reminder() {
        let skills = vec![
            make_test_skill("hidden-a", "Hidden A", false, true),
            make_test_skill("hidden-b", "Hidden B", false, true),
        ];
        let result = build_system_prompt(None, "/tmp", &skills, None);
        assert!(
            !result.contains("The following skills are available"),
            "all-hidden skills should not inject reminder"
        );
    }

    #[test]
    fn test_build_system_prompt_custom_prompt_and_skills() {
        let skills = vec![make_test_skill("my-skill", "My desc", false, false)];
        let result =
            build_system_prompt(Some("Custom instructions here"), "/tmp", &skills, None);
        assert!(
            result.contains("Custom instructions here"),
            "custom prompt should appear"
        );
        assert!(
            result.contains("The following skills are available for use with the Skill tool:"),
            "skills reminder should also appear"
        );
    }

    #[test]
    fn test_build_system_prompt_skills_reminder_after_custom_prompt() {
        let skills = vec![make_test_skill("my-skill", "My desc", false, false)];
        let result =
            build_system_prompt(Some("Custom text"), "/tmp", &skills, None);
        let custom_pos = result.find("Custom text").unwrap();
        let reminder_pos = result.rfind("<system-reminder>").unwrap();
        assert!(
            reminder_pos > custom_pos,
            "skills reminder should appear after custom prompt"
        );
    }

    #[test]
    fn test_build_system_prompt_small_budget_triggers_minimal_mode() {
        // context_window_tokens = 50 → budget = 2 chars, triggers minimal mode for non-bundled
        let skill = make_test_skill("nb-skill", &"x".repeat(100), false, false);
        let result = build_system_prompt(None, "/tmp", &[skill], Some(50));
        // Minimal mode: skill appears as name only, no ': '
        assert!(
            result.contains("- nb-skill"),
            "skill name should appear in minimal mode"
        );
        assert!(
            !result.contains("- nb-skill: "),
            "non-bundled should not have description in minimal mode"
        );
    }

    #[test]
    fn test_build_system_prompt_cwd_in_prompt() {
        let result = build_system_prompt(None, "/workspace/my-project", &[], None);
        assert!(
            result.contains("/workspace/my-project"),
            "cwd should appear in the system prompt"
        );
    }
}
