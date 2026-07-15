use super::*;

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
    async fn execute_injects_runtime_env() {
        let tool = ExecCommandTool::new_with_env(
            std::env::temp_dir(),
            vec![("AION_RUNTIME_ENV_TEST".to_string(), "exec-env-value".to_string())],
        );
        #[cfg(windows)]
        let input = json!({"cmd": "Write-Output $env:AION_RUNTIME_ENV_TEST", "shell": "powershell"});
        #[cfg(not(windows))]
        let input = json!({"cmd": "printf '%s' \"$AION_RUNTIME_ENV_TEST\"", "shell": "sh"});

        let result = tool.execute(input).await;

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(result.content.contains("exec-env-value"));
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

        assert!(result.is_error, "timeout should be an error: {}", result.content);
        assert!(
            result.content.contains("Command timed out after 1500ms"),
            "timeout message missing: {}",
            result.content
        );
        assert!(
            result.content.contains("STDOUT:\n") && result.content.contains("aion_stdout_before_timeout"),
            "stdout emitted before timeout should be preserved, got: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn execute_timeout_preserves_stderr_emitted_before_timeout() {
        let tool = ExecCommandTool::new(std::env::temp_dir());
        #[cfg(windows)]
        let cmd = "[Console]::Error.WriteLine('aion_stderr_before_timeout'); Start-Sleep -Seconds 5";
        #[cfg(not(windows))]
        let cmd = "printf 'aion_stderr_before_timeout\\n' >&2; sleep 5";
        let input = json!({
            "cmd": cmd,
            "timeout": 1500
        });

        let result = tool.execute(input).await;

        assert!(result.is_error, "timeout should be an error: {}", result.content);
        assert!(
            result.content.contains("Command timed out after 1500ms"),
            "timeout message missing: {}",
            result.content
        );
        assert!(
            result.content.contains("STDERR:\n") && result.content.contains("aion_stderr_before_timeout"),
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

        assert!(result.is_error, "timeout should be an error: {}", result.content);
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
            result.content.contains("STDOUT:\n") && result.content.contains("aion_powershell_stdout_probe"),
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
            result.content.contains("STDOUT:\n") && result.content.contains("aion_cmd_stdout_probe"),
            "cmd stdout should be preserved, got: {}",
            result.content
        );
    }
}
