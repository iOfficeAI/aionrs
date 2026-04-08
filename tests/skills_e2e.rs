//! End-to-end skill tests using real files on disk.
//!
//! Each test creates skill files in a temporary directory that mirrors the
//! `.aionrs/skills/` and `.aionrs/commands/` layout, then exercises the full
//! pipeline: discovery -> loading -> system prompt injection -> SkillTool execution.
//!
//! Tests use `load_all_skills` with `add_dirs` or a temp cwd to avoid depending
//! on any pre-existing files in the repo or user home directory.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aionrs::context::build_system_prompt;
use aionrs::skills::loader::load_all_skills;
use aionrs::skills::permissions::SkillPermissionChecker;
use aionrs::skills::types::SkillMetadata;
use aionrs::tools::skill::SkillTool;
use aionrs::tools::Tool;
use serde_json::json;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn find_skill<'a>(skills: &'a [SkillMetadata], name: &str) -> Option<&'a SkillMetadata> {
    skills.iter().find(|s| s.name == name)
}

/// Create a project-like temp directory with `.git` marker and `.aionrs/skills/` + `.aionrs/commands/`.
/// Returns (TempDir guard, root path).
fn make_project() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();

    // Git root marker so walk_up stops here
    fs::create_dir(root.join(".git")).unwrap();

    // Skills directory
    let skills_dir = root.join(".aionrs").join("skills");
    fs::create_dir_all(&skills_dir).unwrap();

    // Commands directory (legacy)
    let commands_dir = root.join(".aionrs").join("commands");
    fs::create_dir_all(&commands_dir).unwrap();

    // --- greet skill ---
    let greet_dir = skills_dir.join("greet");
    fs::create_dir_all(&greet_dir).unwrap();
    fs::write(
        greet_dir.join("SKILL.md"),
        "---\nname: greet\ndescription: Greet a user by name\n---\n\nHello, $ARGUMENTS! Welcome to the project.\n",
    ).unwrap();

    // --- db:migrate (nested namespace) ---
    let migrate_dir = skills_dir.join("db").join("migrate");
    fs::create_dir_all(&migrate_dir).unwrap();
    fs::write(
        migrate_dir.join("SKILL.md"),
        "---\nname: db:migrate\ndescription: Run database migrations\n---\n\nRunning migrations for: $ARGUMENTS\nSkill directory: ${CLAUDE_SKILL_DIR}\n",
    ).unwrap();

    // --- rust-review (conditional paths) ---
    let review_dir = skills_dir.join("rust-review");
    fs::create_dir_all(&review_dir).unwrap();
    fs::write(
        review_dir.join("SKILL.md"),
        "---\nname: rust-review\ndescription: Rust-specific code review checklist\npaths:\n  - \"**/*.rs\"\n  - \"Cargo.toml\"\n---\n\nWhen reviewing Rust code, check:\n- No unwrap() in library code\n",
    ).unwrap();

    // --- shell-demo (shell expansion) ---
    let shell_dir = skills_dir.join("shell-demo");
    fs::create_dir_all(&shell_dir).unwrap();
    fs::write(
        shell_dir.join("SKILL.md"),
        "---\nname: shell-demo\ndescription: Demonstrate shell command expansion\n---\n\nCurrent date: !`date +%Y-%m-%d`\n",
    ).unwrap();

    // --- legacy command (flat .md in commands/) ---
    fs::write(
        commands_dir.join("legacy-cmd.md"),
        "---\nname: legacy-cmd\ndescription: A legacy command for backward compatibility testing\n---\n\nThis is a legacy command loaded from .aionrs/commands/\nArguments: $ARGUMENTS\n",
    ).unwrap();

    (tmp, root)
}

fn make_tool(skills: Vec<SkillMetadata>, cwd: &str) -> SkillTool {
    SkillTool::new(
        Arc::new(skills),
        cwd.to_string(),
        SkillPermissionChecker::new(vec![], vec![], false),
    )
}

// ---------------------------------------------------------------------------
// E1: Project-level skill discovery
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e1_project_skill_discovered() {
    let (_guard, root) = make_project();
    let skills = load_all_skills(&root, &[], false, None).await;

    let greet = find_skill(&skills, "greet");
    assert!(greet.is_some(), "E1 FAIL: 'greet' skill not discovered");
    assert_eq!(greet.unwrap().description, "Greet a user by name");
    println!("E1 PASS: project-level skill 'greet' discovered with correct description");
}

// ---------------------------------------------------------------------------
// E2: Legacy commands discovery
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e2_legacy_commands_discovered() {
    let (_guard, root) = make_project();
    let skills = load_all_skills(&root, &[], false, None).await;

    let legacy = find_skill(&skills, "legacy-cmd");
    assert!(legacy.is_some(), "E2 FAIL: 'legacy-cmd' not discovered");
    println!("E2 PASS: legacy command 'legacy-cmd' discovered from .aionrs/commands/");
}

// ---------------------------------------------------------------------------
// E3: Nested namespace (db:migrate)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e3_nested_namespace() {
    let (_guard, root) = make_project();
    let skills = load_all_skills(&root, &[], false, None).await;

    let migrate = find_skill(&skills, "db:migrate");
    assert!(migrate.is_some(), "E3 FAIL: 'db:migrate' not discovered");
    println!("E3 PASS: nested skill 'db:migrate' discovered with colon namespace");
}

// ---------------------------------------------------------------------------
// E4: Variable substitution ($ARGUMENTS)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e4_variable_substitution() {
    let (_guard, root) = make_project();
    let cwd = root.to_string_lossy().to_string();
    let skills = load_all_skills(&root, &[], false, None).await;
    let tool = make_tool(skills, &cwd);

    let result = tool.execute(json!({"skill": "greet", "args": "Alice"})).await;
    assert!(!result.is_error, "E4 FAIL: error: {}", result.content);
    assert!(
        result.content.contains("Hello, Alice!"),
        "E4 FAIL: $ARGUMENTS not substituted. Got: {}",
        result.content
    );
    println!("E4 PASS: $ARGUMENTS substituted correctly");
}

// ---------------------------------------------------------------------------
// E5: Shell command expansion
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e5_shell_expansion() {
    let (_guard, root) = make_project();
    let cwd = root.to_string_lossy().to_string();
    let skills = load_all_skills(&root, &[], false, None).await;
    let tool = make_tool(skills, &cwd);

    let result = tool.execute(json!({"skill": "shell-demo"})).await;
    assert!(!result.is_error, "E5 FAIL: error: {}", result.content);

    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    assert!(
        result.content.contains(&today),
        "E5 FAIL: shell expansion did not produce today's date. Got: {}",
        result.content
    );
    println!("E5 PASS: shell expansion produced today's date ({})", today);
}

// ---------------------------------------------------------------------------
// E6: Conditional activation (paths filter)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e6_conditional_activation() {
    let (_guard, root) = make_project();
    let skills = load_all_skills(&root, &[], false, None).await;

    let rust_review = find_skill(&skills, "rust-review").expect("E6 FAIL: 'rust-review' not found");
    assert!(!rust_review.paths.is_empty(), "E6 FAIL: paths should not be empty");
    assert!(
        rust_review.paths.iter().any(|p| p.contains("*.rs")),
        "E6 FAIL: paths should contain '*.rs'. Got: {:?}",
        rust_review.paths
    );
    println!("E6 PASS: 'rust-review' has conditional paths: {:?}", rust_review.paths);
}

// ---------------------------------------------------------------------------
// E7: System prompt injection
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e7_system_prompt_injection() {
    let (_guard, root) = make_project();
    let cwd = root.to_string_lossy().to_string();
    let skills = load_all_skills(&root, &[], false, None).await;

    let prompt = build_system_prompt(None, &cwd, &skills, None);
    assert!(prompt.contains("greet"), "E7 FAIL: 'greet' not in system prompt");
    assert!(prompt.contains("db:migrate"), "E7 FAIL: 'db:migrate' not in system prompt");
    assert!(prompt.contains("system-reminder"), "E7 FAIL: missing <system-reminder> wrapper");
    println!("E7 PASS: skills injected into system prompt");
}

// ---------------------------------------------------------------------------
// E8: Full SkillTool execution (db:migrate with $ARGUMENTS + ${CLAUDE_SKILL_DIR})
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e8_full_execution() {
    let (_guard, root) = make_project();
    let cwd = root.to_string_lossy().to_string();
    let skills = load_all_skills(&root, &[], false, None).await;
    let tool = make_tool(skills, &cwd);

    let result = tool.execute(json!({"skill": "db:migrate", "args": "production"})).await;
    assert!(!result.is_error, "E8 FAIL: error: {}", result.content);
    assert!(
        result.content.contains("Running migrations for: production"),
        "E8 FAIL: $ARGUMENTS not substituted. Got: {}",
        result.content
    );
    assert!(
        !result.content.contains("${CLAUDE_SKILL_DIR}"),
        "E8 FAIL: ${{CLAUDE_SKILL_DIR}} not expanded. Got: {}",
        result.content
    );
    println!("E8 PASS: full execution with $ARGUMENTS and ${{CLAUDE_SKILL_DIR}} substitution");
}

// ---------------------------------------------------------------------------
// E9: Deduplication
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e9_deduplication() {
    let (_guard, root) = make_project();
    let skills = load_all_skills(&root, &[], false, None).await;

    let mut name_counts = std::collections::HashMap::new();
    for skill in &skills {
        *name_counts.entry(skill.name.as_str()).or_insert(0usize) += 1;
    }
    for (name, count) in &name_counts {
        assert_eq!(*count, 1, "E9 FAIL: '{}' appears {} times", name, count);
    }
    println!("E9 PASS: all {} skills have unique names", skills.len());
}

// ---------------------------------------------------------------------------
// E10: Skill not found error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e10_skill_not_found() {
    let (_guard, root) = make_project();
    let cwd = root.to_string_lossy().to_string();
    let skills = load_all_skills(&root, &[], false, None).await;
    let tool = make_tool(skills, &cwd);

    let result = tool.execute(json!({"skill": "nonexistent-skill"})).await;
    assert!(result.is_error, "E10 FAIL: should return error");
    assert!(result.content.contains("not found"), "E10 FAIL: got: {}", result.content);
    println!("E10 PASS: nonexistent skill returns clear error message");
}

// ---------------------------------------------------------------------------
// E11: Legacy command execution
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e11_legacy_command_execution() {
    let (_guard, root) = make_project();
    let cwd = root.to_string_lossy().to_string();
    let skills = load_all_skills(&root, &[], false, None).await;
    let tool = make_tool(skills, &cwd);

    let result = tool.execute(json!({"skill": "legacy-cmd", "args": "test-arg"})).await;
    assert!(!result.is_error, "E11 FAIL: error: {}", result.content);
    assert!(result.content.contains("legacy command"), "E11 FAIL: got: {}", result.content);
    assert!(result.content.contains("test-arg"), "E11 FAIL: $ARGUMENTS not substituted. Got: {}", result.content);
    println!("E11 PASS: legacy command executed with variable substitution");
}

// ---------------------------------------------------------------------------
// E12: Additional skills dir (simulates user-level via add_dirs + bare mode)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e12_additional_skills_dir() {
    let tmp = TempDir::new().unwrap();
    let add_dir = tmp.path().to_path_buf();

    let skill_dir = add_dir.join(".aionrs").join("skills").join("user-greeting");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: user-greeting\ndescription: A user-level global skill for E2E testing\n---\n\nHi from user-level skill! Args: $ARGUMENTS\n",
    ).unwrap();

    // bare=true + add_dirs exercises the same code path as user-level discovery
    let skills = load_all_skills(Path::new("/nonexistent"), &[add_dir], true, None).await;

    let user_skill = find_skill(&skills, "user-greeting");
    assert!(user_skill.is_some(), "E12 FAIL: 'user-greeting' not discovered via add_dirs");
    assert_eq!(user_skill.unwrap().description, "A user-level global skill for E2E testing");
    println!("E12 PASS: skill discovered via additional skills dir (add_dirs)");
}

// ---------------------------------------------------------------------------
// E13: Additional skills dir execution
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e13_additional_skills_dir_execution() {
    let tmp = TempDir::new().unwrap();
    let add_dir = tmp.path().to_path_buf();

    let skill_dir = add_dir.join(".aionrs").join("skills").join("user-greeting");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: user-greeting\ndescription: test\n---\n\nHi from user-level skill! Args: $ARGUMENTS\n",
    ).unwrap();

    let skills = load_all_skills(Path::new("/nonexistent"), &[add_dir], true, None).await;
    let tool = make_tool(skills, "/tmp");

    let result = tool.execute(json!({"skill": "user-greeting", "args": "World"})).await;
    assert!(!result.is_error, "E13 FAIL: error: {}", result.content);
    assert!(result.content.contains("Hi from user-level skill!"), "E13 FAIL: got: {}", result.content);
    assert!(result.content.contains("World"), "E13 FAIL: $ARGUMENTS not substituted. Got: {}", result.content);
    println!("E13 PASS: additional dir skill executed with $ARGUMENTS substitution");
}

// ---------------------------------------------------------------------------
// E14: Additional skills dir appears in system prompt
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e14_additional_skills_in_system_prompt() {
    let tmp = TempDir::new().unwrap();
    let add_dir = tmp.path().to_path_buf();

    let skill_dir = add_dir.join(".aionrs").join("skills").join("user-greeting");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: user-greeting\ndescription: A global skill\n---\n\nContent\n",
    ).unwrap();

    let skills = load_all_skills(Path::new("/nonexistent"), &[add_dir], true, None).await;
    let prompt = build_system_prompt(None, "/tmp", &skills, None);
    assert!(prompt.contains("user-greeting"), "E14 FAIL: 'user-greeting' not in system prompt");
    println!("E14 PASS: additional dir skill appears in system prompt");
}
