mod common;

use aion_agent::orchestration::execute_tool_calls;
use aion_compact::CompactionLevel;
use aion_tools::registry::ToolRegistry;
use aion_types::message::ContentBlock;
use common::{MockTool, auto_approve_confirmer};
use serde_json::json;

const TEST_OUTPUT: &str = "\x1b[32mSTATUS: OK\x1b[0m\n\n\n\n50%\r100%\nCompiling dep-0 v1.0.0\nCompiling dep-1 v1.0.0\nCompiling dep-2 v1.0.0\nCompiling dep-3 v1.0.0\nCompiling dep-4 v1.0.0\n{\n    \"id\": 1,\n    \"name\": \"Alice Wonderland\",\n    \"email\": \"alice@example.com\",\n    \"age\": 30,\n    \"address\": \"123 Main Street, Anytown, USA 12345\",\n    \"phone\": \"+1-555-0123\"\n}";

const TOON_INPUT: &str =
    r#"[{"id":1,"name":"Alice","role":"admin"},{"id":2,"name":"Bob","role":"user"}]"#;

fn make_tool_use(id: &str, name: &str) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: name.to_string(),
        input: json!({}),
    }
}

fn extract_tool_result_content(blocks: &[ContentBlock]) -> &str {
    for block in blocks {
        if let ContentBlock::ToolResult { content, .. } = block {
            return content;
        }
    }
    panic!("no ToolResult found in blocks");
}

// ---------------------------------------------------------------------------
// A Layer: Case 1-3 (Off / Safe / Full)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn case_1_off_passthrough() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("test_tool", TEST_OUTPUT, false)));

    let tool_calls = vec![make_tool_use("c1", "test_tool")];
    let confirmer = auto_approve_confirmer();

    let outcome = execute_tool_calls(
        &registry,
        &tool_calls,
        &confirmer,
        None,
        CompactionLevel::Off,
        false,
    )
    .await
    .expect("should succeed");

    let content = extract_tool_result_content(&outcome);
    eprintln!("[compaction:A] === Case 1: Off passthrough ===");
    eprintln!(
        "[compaction:A] raw ({} chars): {:?}",
        TEST_OUTPUT.len(),
        &TEST_OUTPUT[..60]
    );
    eprintln!(
        "[compaction:A] result ({} chars): {:?}",
        content.len(),
        &content[..60]
    );

    assert_eq!(
        content, TEST_OUTPUT,
        "Off level should pass content through unchanged"
    );
    eprintln!("[compaction:A] ✓ content unchanged");
}

#[tokio::test]
async fn case_2_safe_sanitizes() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("test_tool", TEST_OUTPUT, false)));

    let tool_calls = vec![make_tool_use("c2", "test_tool")];
    let confirmer = auto_approve_confirmer();

    let outcome = execute_tool_calls(
        &registry,
        &tool_calls,
        &confirmer,
        None,
        CompactionLevel::Safe,
        false,
    )
    .await
    .expect("should succeed");

    let content = extract_tool_result_content(&outcome);
    eprintln!("[compaction:A] === Case 2: Safe sanitizes ===");
    eprintln!("[compaction:A] raw ({} chars)", TEST_OUTPUT.len());
    eprintln!(
        "[compaction:A] result ({} chars): {:?}",
        content.len(),
        content
    );

    assert!(!content.contains("\x1b"), "Safe should strip ANSI escapes");
    assert!(
        !content.contains("\n\n\n"),
        "Safe should merge blank lines"
    );
    assert!(!content.contains("\r"), "Safe should collapse CR lines");
    assert!(
        content.contains("Compiling dep-0"),
        "Safe should keep all repeated lines"
    );
    assert!(
        content.contains("Compiling dep-4"),
        "Safe should keep all repeated lines"
    );
    assert!(
        content.contains("    \"id\""),
        "Safe should preserve original JSON indentation"
    );

    eprintln!("[compaction:A] ✓ ANSI stripped, blanks merged, CR collapsed, repeats & JSON untouched");
}

#[tokio::test]
async fn case_3_full_folds_and_compacts() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("test_tool", TEST_OUTPUT, false)));

    let tool_calls = vec![make_tool_use("c3", "test_tool")];
    let confirmer = auto_approve_confirmer();

    let outcome = execute_tool_calls(
        &registry,
        &tool_calls,
        &confirmer,
        None,
        CompactionLevel::Full,
        false,
    )
    .await
    .expect("should succeed");

    let content = extract_tool_result_content(&outcome);
    eprintln!("[compaction:A] === Case 3: Full folds and compacts ===");
    eprintln!("[compaction:A] raw ({} chars)", TEST_OUTPUT.len());
    eprintln!(
        "[compaction:A] result ({} chars): {:?}",
        content.len(),
        content
    );

    assert!(!content.contains("\x1b"), "Full should strip ANSI");
    assert!(
        content.contains("similar lines") || content.contains("identical lines"),
        "Full should fold repeated lines: {content}"
    );
    assert!(
        content.len() < TEST_OUTPUT.len(),
        "Full should produce shorter output: {} vs {}",
        content.len(),
        TEST_OUTPUT.len()
    );

    eprintln!("[compaction:A] ✓ ANSI stripped, lines folded, output shorter");
}

// ---------------------------------------------------------------------------
// A Layer: Case 4-5 (TOON on / off)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn case_4_toon_encodes_array() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("test_tool", TOON_INPUT, false)));

    let tool_calls = vec![make_tool_use("c4", "test_tool")];
    let confirmer = auto_approve_confirmer();

    let outcome = execute_tool_calls(
        &registry,
        &tool_calls,
        &confirmer,
        None,
        CompactionLevel::Full,
        true,
    )
    .await
    .expect("should succeed");

    let content = extract_tool_result_content(&outcome);
    eprintln!("[compaction:A] === Case 4: TOON encodes array ===");
    eprintln!("[compaction:A] raw: {TOON_INPUT}");
    eprintln!("[compaction:A] result: {content}");

    assert!(
        content.contains("[2]{id,name,role}:"),
        "TOON should produce header: {content}"
    );
    assert!(content.contains("Alice"), "TOON should contain data");
    assert!(content.contains("Bob"), "TOON should contain data");

    eprintln!("[compaction:A] ✓ TOON header present with data rows");
}

#[tokio::test]
async fn case_5_toon_disabled_no_encoding() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("test_tool", TOON_INPUT, false)));

    let tool_calls = vec![make_tool_use("c5", "test_tool")];
    let confirmer = auto_approve_confirmer();

    let outcome = execute_tool_calls(
        &registry,
        &tool_calls,
        &confirmer,
        None,
        CompactionLevel::Full,
        false,
    )
    .await
    .expect("should succeed");

    let content = extract_tool_result_content(&outcome);
    eprintln!("[compaction:A] === Case 5: TOON disabled ===");
    eprintln!("[compaction:A] raw: {TOON_INPUT}");
    eprintln!("[compaction:A] result: {content}");

    assert!(
        !content.contains("[2]{id,name,role}:"),
        "TOON off should not produce TOON header: {content}"
    );

    eprintln!("[compaction:A] ✓ no TOON encoding when disabled");
}
