use std::path::Path;

use aion_memory::prompt::build_memory_prompt;
use aion_skills::prompt::format_skills_within_budget;
use aion_skills::types::SkillMetadata;
use aion_types::message::{ContentBlock, Message, Role};

use crate::plan::prompt as plan_prompt;

/// Return the tool-usage guidance section for the system prompt.
///
/// This section teaches the model when to prefer dedicated tools over Bash,
/// how to handle parallel vs sequential calls, and cross-tool best practices.
/// Intentionally redundant with individual tool descriptions — the dual
/// placement ensures the model follows the rules regardless of attention span.
fn tool_usage_guidance() -> &'static str {
    "\
# Using your tools
 - Do NOT use Bash when a dedicated tool is available. Using dedicated tools \
allows the user to better understand and review your work:
   - File search: Glob (not find or ls)
   - Content search: Grep (not grep or rg)
   - Read files: Read (not cat, head, or tail)
   - Edit files: Edit (not sed or awk)
   - Write files: Write (not echo redirection or cat with heredoc)
 - You can call multiple tools in a single response. If there are no \
dependencies between them, make all independent calls in parallel. \
However, if one call depends on a previous result, run them sequentially.
 - Prefer Edit over Write for modifying existing files — Edit sends only \
the diff, which is easier to review.
 - Always Read a file before editing it."
}

/// Build the system prompt from config and environment.
///
/// Sections are assembled in this order:
/// 1. Base intro (role, working directory, date)
/// 2. Tool usage guidance (dedicated tools, parallel calls, etc.)
/// 3. Custom prompt (user config)
/// 4. AGENTS.md (project instructions)
/// 5. Memory system prompt (behavioral instructions + MEMORY.md content)
/// 6. Plan mode instructions (when active)
/// 7. Skills reminder (available skills listing)
pub fn build_system_prompt(
    custom_prompt: Option<&str>,
    cwd: &str,
    skills: &[SkillMetadata],
    context_window_tokens: Option<usize>,
    memory_dir: Option<&Path>,
    plan_mode_active: bool,
) -> String {
    let mut parts = Vec::new();

    parts.push(format!(
        "You are an AI assistant that can use tools to help with tasks.\n\
         Working directory: {cwd}\n\
         Current date: {}",
        chrono::Local::now().format("%Y-%m-%d")
    ));

    // Tool usage guidance — placed early so the model sees it before making any tool call
    parts.push(tool_usage_guidance().to_string());

    if let Some(custom) = custom_prompt {
        parts.push(custom.to_string());
    }

    // Read AGENTS.md if it exists
    let agents_md = Path::new(cwd).join("AGENTS.md");
    if agents_md.exists()
        && let Ok(content) = std::fs::read_to_string(&agents_md)
    {
        parts.push(format!("# Project Instructions (AGENTS.md)\n\n{content}"));
    }

    // Inject memory system prompt (behavioral instructions + MEMORY.md content)
    if let Some(dir) = memory_dir {
        let memory_prompt = build_memory_prompt(dir);
        if !memory_prompt.is_empty() {
            parts.push(memory_prompt);
        }
    }

    // Inject plan mode instructions when active
    if plan_mode_active {
        parts.push(plan_prompt::plan_mode_instructions().to_string());
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

    let summary_msg = Message::new(Role::User, vec![ContentBlock::Text { text: summary_text }]);

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
            Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "hello".to_string(),
                }],
            ),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::Text {
                    text: "hi".to_string(),
                }],
            ),
        ];
        compact_messages(&mut messages, 4);
        assert_eq!(messages.len(), 2); // no change
    }

    #[test]
    fn test_compact_messages() {
        let mut messages: Vec<Message> = (0..10)
            .map(|i| {
                Message::new(
                    if i % 2 == 0 {
                        Role::User
                    } else {
                        Role::Assistant
                    },
                    vec![ContentBlock::Text {
                        text: format!("msg {}", i),
                    }],
                )
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
        let prompt = build_system_prompt(None, cwd, &[], None, None, false);
        assert!(prompt.contains(cwd), "system prompt should contain the cwd");
    }

    #[test]
    fn test_build_system_prompt_with_custom_instructions() {
        // Verify that custom instructions are included in the returned prompt
        let custom = "Always respond in haiku.";
        let prompt = build_system_prompt(Some(custom), "/tmp", &[], None, None, false);
        assert!(
            prompt.contains(custom),
            "system prompt should contain the custom instructions"
        );
    }

    #[test]
    fn test_compact_messages_preserves_first_and_last() {
        // Build 8 messages (indices 0–7); keep_tail = 3
        let mut messages: Vec<Message> = (0..8)
            .map(|i| {
                Message::new(
                    if i % 2 == 0 {
                        Role::User
                    } else {
                        Role::Assistant
                    },
                    vec![ContentBlock::Text {
                        text: format!("msg {}", i),
                    }],
                )
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
            .map(|i| {
                Message::new(
                    if i % 2 == 0 {
                        Role::User
                    } else {
                        Role::Assistant
                    },
                    vec![ContentBlock::Text {
                        text: format!("msg {}", i),
                    }],
                )
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

    use aion_skills::types::{ExecutionContext, LoadedFrom, SkillMetadata, SkillSource};

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
        let result = build_system_prompt(None, "/tmp", &[], None, None, false);
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
        let result = build_system_prompt(None, "/tmp", &skills, None, None, false);
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
        let result = build_system_prompt(None, "/tmp", &skills, None, None, false);
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
        let result = build_system_prompt(None, "/tmp", &skills, None, None, false);
        assert!(
            !result.contains("The following skills are available"),
            "all-hidden skills should not inject reminder"
        );
    }

    #[test]
    fn test_build_system_prompt_custom_prompt_and_skills() {
        let skills = vec![make_test_skill("my-skill", "My desc", false, false)];
        let result = build_system_prompt(
            Some("Custom instructions here"),
            "/tmp",
            &skills,
            None,
            None,
            false,
        );
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
        let result = build_system_prompt(Some("Custom text"), "/tmp", &skills, None, None, false);
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
        let result = build_system_prompt(None, "/tmp", &[skill], Some(50), None, false);
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
        let result = build_system_prompt(None, "/workspace/my-project", &[], None, None, false);
        assert!(
            result.contains("/workspace/my-project"),
            "cwd should appear in the system prompt"
        );
    }

    #[test]
    fn test_build_system_prompt_loads_agents_md_not_claude_md() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cwd = tmp.path();

        // Create both AGENTS.md and CLAUDE.md
        std::fs::write(cwd.join("AGENTS.md"), "AGENTS_CONTENT_HERE").unwrap();
        std::fs::write(cwd.join("CLAUDE.md"), "CLAUDE_CONTENT_HERE").unwrap();

        let result = build_system_prompt(None, &cwd.to_string_lossy(), &[], None, None, false);

        assert!(
            result.contains("AGENTS_CONTENT_HERE"),
            "should load AGENTS.md content"
        );
        assert!(
            !result.contains("CLAUDE_CONTENT_HERE"),
            "should NOT load CLAUDE.md content"
        );
        assert!(
            result.contains("Project Instructions (AGENTS.md)"),
            "header should reference AGENTS.md"
        );
    }

    #[test]
    fn test_build_system_prompt_no_agents_md_no_injection() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cwd = tmp.path();

        // Only CLAUDE.md exists, no AGENTS.md
        std::fs::write(cwd.join("CLAUDE.md"), "SHOULD_NOT_APPEAR").unwrap();

        let result = build_system_prompt(None, &cwd.to_string_lossy(), &[], None, None, false);

        assert!(
            !result.contains("SHOULD_NOT_APPEAR"),
            "CLAUDE.md should be ignored"
        );
        assert!(
            !result.contains("Project Instructions"),
            "no project instructions should be injected"
        );
    }

    // --- Memory integration tests ---

    #[test]
    fn memory_none_dir_no_injection() {
        let result = build_system_prompt(None, "/tmp", &[], None, None, false);
        assert!(
            !result.contains("auto memory"),
            "no memory content when memory_dir is None"
        );
    }

    #[test]
    fn memory_with_dir_injects_prompt() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mem_dir = tmp.path().join("memory");
        std::fs::create_dir_all(&mem_dir).unwrap();
        std::fs::write(
            mem_dir.join("MEMORY.md"),
            "- [Role](user_role.md) \u{2014} senior engineer\n",
        )
        .unwrap();

        let result = build_system_prompt(None, "/tmp", &[], None, Some(&mem_dir), false);

        assert!(
            result.contains("auto memory"),
            "should contain memory system display name"
        );
        assert!(
            result.contains("Types of memory"),
            "should contain type definitions"
        );
        assert!(
            result.contains("user_role.md"),
            "should contain MEMORY.md content"
        );
    }

    #[test]
    fn memory_nonexistent_dir_graceful_degradation() {
        let result = build_system_prompt(
            None,
            "/tmp",
            &[],
            None,
            Some(Path::new("/nonexistent/memory/dir")),
            false,
        );

        // Should not panic and should show empty state
        assert!(
            result.contains("currently empty"),
            "nonexistent memory dir should show empty state"
        );
    }

    #[test]
    fn memory_empty_dir_shows_empty_state() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mem_dir = tmp.path().join("memory");
        std::fs::create_dir_all(&mem_dir).unwrap();
        // No MEMORY.md

        let result = build_system_prompt(None, "/tmp", &[], None, Some(&mem_dir), false);

        assert!(
            result.contains("currently empty"),
            "empty memory dir should show empty state"
        );
    }

    #[test]
    fn memory_appears_after_agents_md_before_skills() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cwd = tmp.path();

        // Create AGENTS.md
        std::fs::write(cwd.join("AGENTS.md"), "PROJECT_RULES_HERE").unwrap();

        // Create memory dir with content
        let mem_dir = tmp.path().join("memory");
        std::fs::create_dir_all(&mem_dir).unwrap();
        std::fs::write(mem_dir.join("MEMORY.md"), "- [A](a.md) \u{2014} test\n").unwrap();

        let skills = vec![make_test_skill("test-skill", "A skill", false, false)];

        let result = build_system_prompt(
            None,
            &cwd.to_string_lossy(),
            &skills,
            None,
            Some(&mem_dir),
            false,
        );

        let agents_pos = result.find("PROJECT_RULES_HERE").unwrap();
        let memory_pos = result.find("auto memory").unwrap();
        let skills_pos = result.find("test-skill").unwrap();

        assert!(
            agents_pos < memory_pos,
            "AGENTS.md should appear before memory"
        );
        assert!(
            memory_pos < skills_pos,
            "memory should appear before skills"
        );
    }

    #[test]
    fn memory_no_bb_brand_in_prompt() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mem_dir = tmp.path().join("memory");
        std::fs::create_dir_all(&mem_dir).unwrap();
        std::fs::write(
            mem_dir.join("MEMORY.md"),
            "- [Test](test.md) \u{2014} entry\n",
        )
        .unwrap();

        let result = build_system_prompt(None, "/tmp", &[], None, Some(&mem_dir), false);

        assert!(
            !result.contains("~/.claude"),
            "should not contain bb brand path"
        );
        assert!(
            !result.contains("CLAUDE.md"),
            "should not reference CLAUDE.md"
        );
    }

    // --- Tool usage guidance tests (task 4.3) ---

    #[test]
    fn tool_guidance_section_exists() {
        let result = build_system_prompt(None, "/tmp", &[], None, None, false);
        assert!(
            result.contains("# Using your tools"),
            "system prompt should contain the tool guidance heading"
        );
    }

    #[test]
    fn tool_guidance_contains_bash_prohibition_list() {
        let result = build_system_prompt(None, "/tmp", &[], None, None, false);
        assert!(
            result.contains("Glob"),
            "should mention Glob as find/ls replacement"
        );
        assert!(
            result.contains("Grep"),
            "should mention Grep as grep/rg replacement"
        );
        assert!(
            result.contains("Read"),
            "should mention Read as cat/head/tail replacement"
        );
        assert!(
            result.contains("Edit"),
            "should mention Edit as sed/awk replacement"
        );
        assert!(
            result.contains("Write"),
            "should mention Write as echo/heredoc replacement"
        );
    }

    #[test]
    fn tool_guidance_contains_parallel_call_rules() {
        let result = build_system_prompt(None, "/tmp", &[], None, None, false);
        assert!(
            result.contains("parallel"),
            "should contain parallel call guidance"
        );
        assert!(
            result.contains("sequentially"),
            "should explain when to run sequentially"
        );
    }

    #[test]
    fn tool_guidance_contains_edit_over_write_preference() {
        let result = build_system_prompt(None, "/tmp", &[], None, None, false);
        assert!(
            result.contains("Prefer Edit over Write"),
            "should contain Edit-over-Write preference"
        );
    }

    #[test]
    fn tool_guidance_contains_read_before_edit_rule() {
        let result = build_system_prompt(None, "/tmp", &[], None, None, false);
        assert!(
            result.contains("Read a file before editing"),
            "should contain Read-before-Edit rule"
        );
    }

    #[test]
    fn tool_guidance_after_intro_before_custom_prompt() {
        let result = build_system_prompt(Some("CUSTOM_MARKER_43"), "/tmp", &[], None, None, false);
        let intro_pos = result.find("Working directory").unwrap();
        let guidance_pos = result.find("# Using your tools").unwrap();
        let custom_pos = result.find("CUSTOM_MARKER_43").unwrap();
        assert!(
            guidance_pos > intro_pos,
            "tool guidance should appear after intro"
        );
        assert!(
            guidance_pos < custom_pos,
            "tool guidance should appear before custom prompt"
        );
    }

    #[test]
    fn tool_guidance_before_skills_reminder() {
        let skills = vec![make_test_skill("guide-test-skill", "A skill", false, false)];
        let result = build_system_prompt(None, "/tmp", &skills, None, None, false);
        let guidance_pos = result.find("# Using your tools").unwrap();
        let skills_pos = result.find("guide-test-skill").unwrap();
        assert!(
            guidance_pos < skills_pos,
            "tool guidance should appear before skills reminder"
        );
    }

    #[test]
    fn tool_guidance_present_in_plan_mode() {
        let result = build_system_prompt(None, "/tmp", &[], None, None, true);
        assert!(
            result.contains("# Using your tools"),
            "tool guidance should be present in plan mode"
        );
    }

    #[test]
    fn tool_guidance_before_memory() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mem_dir = tmp.path().join("memory");
        std::fs::create_dir_all(&mem_dir).unwrap();
        std::fs::write(mem_dir.join("MEMORY.md"), "- [X](x.md) \u{2014} test\n").unwrap();

        let result = build_system_prompt(None, "/tmp", &[], None, Some(&mem_dir), false);
        let guidance_pos = result.find("# Using your tools").unwrap();
        let memory_pos = result.find("auto memory").unwrap();
        assert!(
            guidance_pos < memory_pos,
            "tool guidance should appear before memory section"
        );
    }
}
