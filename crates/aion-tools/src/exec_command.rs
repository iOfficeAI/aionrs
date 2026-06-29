use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use aion_config::shell::{resolve_shell, shell_command_builder};
use aion_process::CommandRunner;
use aion_protocol::events::ToolCategory;
use aion_types::tool::{JsonSchema, ToolResult};

use crate::Tool;

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;

pub struct ExecCommandTool {
    cwd: PathBuf,
}

impl ExecCommandTool {
    pub fn new(cwd: PathBuf) -> Self {
        Self { cwd }
    }
}

fn render_exit_result(exit_code: i32, stdout: &[u8], stderr: &[u8]) -> String {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    format!("Exit code: {exit_code}\nSTDOUT:\n{stdout}\nSTDERR:\n{stderr}")
}

fn render_timeout_result(timeout_ms: u64, stdout: &[u8], stderr: &[u8]) -> String {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    format!("Command timed out after {timeout_ms}ms\nSTDOUT:\n{stdout}\nSTDERR:\n{stderr}")
}

#[async_trait]
impl Tool for ExecCommandTool {
    fn name(&self) -> &str {
        "ExecCommand"
    }

    fn description(&self) -> &str {
        "Executes a shell command and returns its output.\n\n\
         IMPORTANT: Do NOT use ExecCommand when a dedicated tool is available:\n\
         - File search: use Glob (not find or ls)\n\
         - Content search: use Grep (not grep or rg)\n\
         - Read files: use Read (not cat, head, or tail)\n\
         - Edit files: use Edit (not sed or awk)\n\
         - Write files: use Write (not echo or cat with heredoc)\n\n\
         # Instructions\n\
         - Use absolute paths to avoid working directory confusion.\n\
         - When issuing multiple independent commands, make parallel tool calls \
         instead of chaining them. Use `&&` only when commands depend on each other.\n\
         - You may specify an optional timeout in milliseconds (default 120000, max 600000).\n\n\
         # Git safety\n\
         - Never force push, reset --hard, or use --no-verify unless explicitly asked.\n\
         - Prefer creating new commits over amending existing ones."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "cmd": {
                    "type": "string",
                    "description": "The command to execute"
                },
                "shell": {
                    "type": "string",
                    "description": "Optional shell override: auto, powershell, pwsh, cmd, bash, zsh, sh, or an executable path"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (default 120000, max 600000)"
                }
            },
            "required": ["cmd"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        false
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let Some(command) = input["cmd"].as_str() else {
            return ToolResult {
                content: "Missing required parameter: cmd".to_string(),
                is_error: true,
            };
        };

        let shell = match resolve_shell(input["shell"].as_str()) {
            Ok(shell) => shell,
            Err(err) => {
                return ToolResult {
                    content: format!("Invalid shell: {}", err),
                    is_error: true,
                };
            }
        };

        tracing::info!(
            cwd = %self.cwd.display(),
            shell_kind = shell.kind.name(),
            shell_path = %shell.path.display(),
            "ExecCommandTool executing"
        );

        let timeout_ms = input["timeout"]
            .as_u64()
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .min(MAX_TIMEOUT_MS);

        let timeout = Duration::from_millis(timeout_ms);

        let cwd = self.cwd.clone();
        let mut command_builder = shell_command_builder(&shell, command, false);
        command_builder.current_dir(&cwd);

        let result = CommandRunner::new(command_builder)
            .timeout(timeout)
            .run()
            .await;

        match result {
            Ok(result) if result.timed_out => ToolResult {
                content: render_timeout_result(timeout_ms, &result.stdout, &result.stderr),
                is_error: true,
            },
            Ok(result) => {
                let exit_code = result.exit_code.unwrap_or(-1);
                ToolResult {
                    content: render_exit_result(exit_code, &result.stdout, &result.stderr),
                    is_error: exit_code != 0,
                }
            }
            Err(err) => ToolResult {
                content: format!("Failed to execute command: {}", err),
                is_error: true,
            },
        }
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Exec
    }

    fn describe(&self, input: &Value) -> String {
        let cmd = input.get("cmd").and_then(|v| v.as_str()).unwrap_or("");
        format!("Execute: {}", crate::truncate_utf8(cmd, 80))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn execute_echo_returns_stdout() {
        let tool = ExecCommandTool::new(std::env::temp_dir());
        let input = json!({"cmd": "echo hello_exec_command"});
        let result = tool.execute(input).await;
        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(result.content.contains("hello_exec_command"));
    }

    #[tokio::test]
    async fn execute_invalid_command_returns_error() {
        let tool = ExecCommandTool::new(std::env::temp_dir());
        let input = json!({"cmd": "nonexistent_command_xyz_123"});
        let result = tool.execute(input).await;
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn execute_respects_cwd() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("cwd_proof.txt"), "proof").unwrap();
        let tool = ExecCommandTool::new(dir.path().to_path_buf());
        let cmd = if cfg!(windows) {
            "type cwd_proof.txt"
        } else {
            "cat cwd_proof.txt"
        };
        let input = json!({"cmd": cmd});
        let result = tool.execute(input).await;
        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(
            result.content.contains("proof"),
            "ExecCommandTool should execute in injected cwd, got: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn execute_timeout_preserves_stdout_emitted_before_timeout() {
        let tool = ExecCommandTool::new(std::env::temp_dir());
        #[cfg(windows)]
        let cmd = "Write-Output aion_stdout_before_timeout; Start-Sleep -Seconds 5";
        #[cfg(not(windows))]
        let cmd = "printf 'aion_stdout_before_timeout\\n'; sleep 5";
        let input = json!({
            "cmd": cmd,
            "timeout": 1500
        });

        let result = tool.execute(input).await;

        assert!(
            result.is_error,
            "timeout should be an error: {}",
            result.content
        );
        assert!(
            result.content.contains("Command timed out after 1500ms"),
            "timeout message missing: {}",
            result.content
        );
        assert!(
            result.content.contains("STDOUT:\n")
                && result.content.contains("aion_stdout_before_timeout"),
            "stdout emitted before timeout should be preserved, got: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn execute_timeout_preserves_stderr_emitted_before_timeout() {
        let tool = ExecCommandTool::new(std::env::temp_dir());
        #[cfg(windows)]
        let cmd =
            "[Console]::Error.WriteLine('aion_stderr_before_timeout'); Start-Sleep -Seconds 5";
        #[cfg(not(windows))]
        let cmd = "printf 'aion_stderr_before_timeout\\n' >&2; sleep 5";
        let input = json!({
            "cmd": cmd,
            "timeout": 1500
        });

        let result = tool.execute(input).await;

        assert!(
            result.is_error,
            "timeout should be an error: {}",
            result.content
        );
        assert!(
            result.content.contains("Command timed out after 1500ms"),
            "timeout message missing: {}",
            result.content
        );
        assert!(
            result.content.contains("STDERR:\n")
                && result.content.contains("aion_stderr_before_timeout"),
            "stderr emitted before timeout should be preserved, got: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn execute_timeout_omits_output_after_timeout() {
        let tool = ExecCommandTool::new(std::env::temp_dir());
        #[cfg(windows)]
        let cmd = "Write-Output aion_before_timeout; Start-Sleep -Seconds 5; Write-Output aion_after_timeout";
        #[cfg(not(windows))]
        let cmd = "printf 'aion_before_timeout\\n'; sleep 5; printf 'aion_after_timeout\\n'";
        let input = json!({
            "cmd": cmd,
            "timeout": 1500
        });

        let result = tool.execute(input).await;

        assert!(
            result.is_error,
            "timeout should be an error: {}",
            result.content
        );
        assert!(
            result.content.contains("Command timed out after 1500ms"),
            "timeout message missing: {}",
            result.content
        );
        assert!(
            result.content.contains("aion_before_timeout"),
            "output emitted before timeout should be preserved, got: {}",
            result.content
        );
        assert!(
            !result.content.contains("aion_after_timeout"),
            "output after timeout should not be present, got: {}",
            result.content
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn execute_powershell_write_output_returns_stdout() {
        let tool = ExecCommandTool::new(std::env::temp_dir());
        let input = json!({
            "cmd": "Write-Output aion_powershell_stdout_probe",
            "shell": "powershell"
        });

        let result = tool.execute(input).await;

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(
            result.content.contains("STDOUT:\n")
                && result.content.contains("aion_powershell_stdout_probe"),
            "PowerShell stdout should be preserved, got: {}",
            result.content
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn execute_powershell_echo_quoted_message_returns_stdout() {
        let tool = ExecCommandTool::new(std::env::temp_dir());
        let input = json!({
            "cmd": "echo \"message\"",
            "shell": "powershell"
        });

        let result = tool.execute(input).await;

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(
            result.content.contains("STDOUT:\n") && result.content.contains("message"),
            "PowerShell quoted echo stdout should be preserved, got: {}",
            result.content
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn execute_cmd_echo_returns_stdout() {
        let tool = ExecCommandTool::new(std::env::temp_dir());
        let input = json!({
            "cmd": "echo aion_cmd_stdout_probe",
            "shell": "cmd"
        });

        let result = tool.execute(input).await;

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(
            result.content.contains("STDOUT:\n")
                && result.content.contains("aion_cmd_stdout_probe"),
            "cmd stdout should be preserved, got: {}",
            result.content
        );
    }
}
