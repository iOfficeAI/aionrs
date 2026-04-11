use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::process::Command;

use aion_protocol::events::ToolCategory;
use aion_types::tool::{JsonSchema, ToolResult};

use crate::Tool;

pub struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "Grep"
    }

    fn description(&self) -> &str {
        "Searches file contents using regex patterns (powered by ripgrep).\n\n\
         IMPORTANT: ALWAYS use this Grep tool for content search. \
         NEVER run grep or rg as a Bash command.\n\n\
         - Supports full regex syntax (e.g., \"log.*Error\", \"fn\\\\s+\\\\w+\").\n\
         - Use the glob parameter to filter by file pattern (e.g., \"*.rs\").\n\
         - Output is truncated to 250 lines.\n\
         - Set case_insensitive to true for case-insensitive search."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The regex pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in (default: cwd)"
                },
                "glob": {
                    "type": "string",
                    "description": "File filter pattern, e.g. \"*.rs\""
                },
                "case_insensitive": {
                    "type": "boolean",
                    "description": "Case insensitive search"
                }
            },
            "required": ["pattern"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let Some(pattern) = input["pattern"].as_str() else {
            return ToolResult {
                content: "Missing required parameter: pattern".to_string(),
                is_error: true,
            };
        };

        let path = input["path"].as_str().unwrap_or(".");

        let glob_pattern = input["glob"].as_str();
        let case_insensitive = input["case_insensitive"].as_bool().unwrap_or(false);

        // Try ripgrep first, fallback to grep
        let result = try_ripgrep(pattern, path, glob_pattern, case_insensitive).await;

        match result {
            Ok(output) => output,
            Err(_) => {
                // Fallback to grep
                try_grep(pattern, path, case_insensitive).await
            }
        }
    }

    fn max_result_size(&self) -> usize {
        20_000
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    fn describe(&self, input: &Value) -> String {
        let pattern = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
        let path = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        format!("Grep '{}' in {}", pattern, path)
    }
}

async fn try_ripgrep(
    pattern: &str,
    path: &str,
    glob_pattern: Option<&str>,
    case_insensitive: bool,
) -> Result<ToolResult, std::io::Error> {
    let mut cmd = Command::new("rg");
    cmd.arg(pattern).arg(path).arg("-n");

    if let Some(g) = glob_pattern {
        cmd.arg("--glob").arg(g);
    }
    if case_insensitive {
        cmd.arg("-i");
    }

    let output = cmd.output().await?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if output.status.code() == Some(1) && stdout.is_empty() {
        return Ok(ToolResult {
            content: "No matches found".to_string(),
            is_error: false,
        });
    }

    if !output.status.success() && output.status.code() != Some(1) {
        return Ok(ToolResult {
            content: format!("rg error: {}", stderr),
            is_error: true,
        });
    }

    // Truncate to 250 lines (global limit, not per-file)
    let lines: Vec<&str> = stdout.lines().take(250).collect();
    Ok(ToolResult {
        content: lines.join("\n"),
        is_error: false,
    })
}

async fn try_grep(pattern: &str, path: &str, case_insensitive: bool) -> ToolResult {
    let mut cmd = Command::new("grep");
    cmd.arg("-rn").arg(pattern).arg(path);

    if case_insensitive {
        cmd.arg("-i");
    }

    match cmd.output().await {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.is_empty() {
                ToolResult {
                    content: "No matches found".to_string(),
                    is_error: false,
                }
            } else {
                // Limit to 250 lines
                let lines: Vec<&str> = stdout.lines().take(250).collect();
                ToolResult {
                    content: lines.join("\n"),
                    is_error: false,
                }
            }
        }
        Err(e) => ToolResult {
            content: format!("grep failed: {}", e),
            is_error: true,
        },
    }
}
