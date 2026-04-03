use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::process::Command;

/// Hook system configuration
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct HooksConfig {
    #[serde(default)]
    pub pre_tool_use: Vec<HookDef>,
    #[serde(default)]
    pub post_tool_use: Vec<HookDef>,
    #[serde(default)]
    pub stop: Vec<HookDef>,
}

/// A single hook definition
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HookDef {
    pub name: String,
    /// Tool name patterns to match (glob). Empty = match all.
    #[serde(default)]
    pub tool_match: Vec<String>,
    /// File path patterns to match (glob). Empty = match all.
    #[serde(default)]
    pub file_match: Vec<String>,
    /// Shell command to execute. Supports ${VAR} interpolation.
    pub command: String,
    /// Timeout in ms (default 30000)
    #[serde(default = "default_hook_timeout")]
    pub timeout_ms: u64,
}

fn default_hook_timeout() -> u64 {
    30_000
}

/// Event-driven hook engine
pub struct HookEngine {
    config: HooksConfig,
}

impl HookEngine {
    pub fn new(config: HooksConfig) -> Self {
        Self { config }
    }

    /// Run pre-tool-use hooks. Returns Err if any hook blocks execution.
    pub async fn run_pre_tool_use(
        &self,
        tool_name: &str,
        tool_input: &serde_json::Value,
    ) -> Result<(), HookError> {
        let matching: Vec<_> = self
            .config
            .pre_tool_use
            .iter()
            .filter(|h| matches_tool(h, tool_name, tool_input))
            .collect();

        for hook in matching {
            let env = build_env_vars(tool_name, tool_input);
            let result = run_hook_command(&hook.command, &env, hook.timeout_ms).await?;
            if !result.success {
                return Err(HookError::Blocked {
                    hook_name: hook.name.clone(),
                    output: result.output,
                });
            }
        }
        Ok(())
    }

    /// Run post-tool-use hooks. Errors are logged but don't block.
    pub async fn run_post_tool_use(
        &self,
        tool_name: &str,
        tool_input: &serde_json::Value,
        tool_output: &str,
    ) -> Vec<String> {
        let matching: Vec<_> = self
            .config
            .post_tool_use
            .iter()
            .filter(|h| matches_tool(h, tool_name, tool_input))
            .collect();

        let mut messages = Vec::new();
        for hook in matching {
            let mut env = build_env_vars(tool_name, tool_input);
            env.insert("TOOL_OUTPUT".to_string(), tool_output.to_string());

            match run_hook_command(&hook.command, &env, hook.timeout_ms).await {
                Ok(result) => {
                    if !result.output.is_empty() {
                        messages.push(format!("[hook:{}] {}", hook.name, result.output.trim()));
                    }
                }
                Err(e) => {
                    messages.push(format!("[hook:{}] error: {}", hook.name, e));
                }
            }
        }
        messages
    }

    /// Run stop hooks when agent session ends.
    pub async fn run_stop(&self) -> Vec<String> {
        let mut messages = Vec::new();
        for hook in &self.config.stop {
            match run_hook_command(&hook.command, &HashMap::new(), hook.timeout_ms).await {
                Ok(result) => {
                    if !result.output.is_empty() {
                        messages.push(format!("[hook:{}] {}", hook.name, result.output.trim()));
                    }
                }
                Err(e) => {
                    messages.push(format!("[hook:{}] error: {}", hook.name, e));
                }
            }
        }
        messages
    }

    /// Check if any hooks are configured
    pub fn has_hooks(&self) -> bool {
        !self.config.pre_tool_use.is_empty()
            || !self.config.post_tool_use.is_empty()
            || !self.config.stop.is_empty()
    }
}

/// Environment variables available to hook commands
fn build_env_vars(
    tool_name: &str,
    tool_input: &serde_json::Value,
) -> HashMap<String, String> {
    let mut env = HashMap::new();
    env.insert("TOOL_NAME".to_string(), tool_name.to_string());
    env.insert("TOOL_INPUT".to_string(), tool_input.to_string());

    // Extract common fields for convenience
    if let Some(fp) = tool_input["file_path"].as_str() {
        env.insert("TOOL_INPUT_FILE_PATH".to_string(), fp.to_string());
    }
    if let Some(cmd) = tool_input["command"].as_str() {
        env.insert("TOOL_INPUT_COMMAND".to_string(), cmd.to_string());
    }
    if let Some(pattern) = tool_input["pattern"].as_str() {
        env.insert("TOOL_INPUT_PATTERN".to_string(), pattern.to_string());
    }

    env
}

fn matches_tool(hook: &HookDef, tool_name: &str, tool_input: &serde_json::Value) -> bool {
    // Check tool_match
    if !hook.tool_match.is_empty() {
        let matches = hook
            .tool_match
            .iter()
            .any(|pattern| glob_match(pattern, tool_name));
        if !matches {
            return false;
        }
    }

    // Check file_match (if tool has a file_path input)
    if !hook.file_match.is_empty() {
        if let Some(file_path) = tool_input["file_path"].as_str() {
            let matches = hook
                .file_match
                .iter()
                .any(|pattern| glob_match(pattern, file_path));
            if !matches {
                return false;
            }
        } else {
            return false; // file_match specified but tool has no file_path
        }
    }

    true
}

fn glob_match(pattern: &str, value: &str) -> bool {
    glob::Pattern::new(pattern)
        .map(|p| p.matches(value))
        .unwrap_or(false)
}

/// Interpolate ${VAR} in a command string with provided env vars
fn interpolate_command(command: &str, env_vars: &HashMap<String, String>) -> String {
    let mut result = command.to_string();
    for (key, value) in env_vars {
        result = result.replace(&format!("${{{}}}", key), value);
    }
    result
}

struct HookResult {
    success: bool,
    output: String,
}

async fn run_hook_command(
    command: &str,
    env_vars: &HashMap<String, String>,
    timeout_ms: u64,
) -> Result<HookResult, HookError> {
    let interpolated = interpolate_command(command, env_vars);
    let timeout = Duration::from_millis(timeout_ms);

    let result = tokio::time::timeout(timeout, async {
        Command::new("sh")
            .arg("-c")
            .arg(&interpolated)
            .envs(env_vars)
            .output()
            .await
    })
    .await;

    match result {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let combined = if stderr.is_empty() {
                stdout
            } else if stdout.is_empty() {
                stderr
            } else {
                format!("{}\n{}", stdout, stderr)
            };

            Ok(HookResult {
                success: output.status.success(),
                output: combined,
            })
        }
        Ok(Err(e)) => Err(HookError::ExecutionFailed(e.to_string())),
        Err(_) => Err(HookError::Timeout(timeout_ms)),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HookError {
    #[error("Hook '{hook_name}' blocked execution: {output}")]
    Blocked { hook_name: String, output: String },
    #[error("Hook execution failed: {0}")]
    ExecutionFailed(String),
    #[error("Hook timed out after {0}ms")]
    Timeout(u64),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_hook(name: &str, tool_match: Vec<&str>, command: &str) -> HookDef {
        HookDef {
            name: name.to_string(),
            tool_match: tool_match.into_iter().map(|s| s.to_string()).collect(),
            file_match: vec![],
            command: command.to_string(),
            timeout_ms: 30_000,
        }
    }

    // --- Pure logic tests ---

    #[test]
    fn test_hook_matches_exact_tool_name() {
        let hook = make_hook("test", vec!["Read"], "echo ok");
        let input = json!({});
        assert!(matches_tool(&hook, "Read", &input));
    }

    #[test]
    fn test_hook_matches_glob_pattern() {
        let hook = make_hook("test", vec!["Read*"], "echo ok");
        let input = json!({});
        assert!(matches_tool(&hook, "ReadFile", &input));
    }

    #[test]
    fn test_hook_no_match() {
        let hook = make_hook("test", vec!["Write"], "echo ok");
        let input = json!({});
        assert!(!matches_tool(&hook, "Read", &input));
    }

    #[test]
    fn test_has_hooks_empty() {
        let engine = HookEngine::new(HooksConfig::default());
        assert!(!engine.has_hooks());
    }

    #[test]
    fn test_has_hooks_with_config() {
        let config = HooksConfig {
            pre_tool_use: vec![make_hook("pre", vec!["*"], "echo ok")],
            post_tool_use: vec![],
            stop: vec![],
        };
        let engine = HookEngine::new(config);
        assert!(engine.has_hooks());
    }

    // --- Shell command tests ---

    #[tokio::test]
    async fn test_pre_hook_allows_execution() {
        let config = HooksConfig {
            pre_tool_use: vec![make_hook("allow", vec!["Read"], "echo ok")],
            post_tool_use: vec![],
            stop: vec![],
        };
        let engine = HookEngine::new(config);
        let result = engine.run_pre_tool_use("Read", &json!({})).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_pre_hook_blocks_on_nonzero_exit() {
        let config = HooksConfig {
            pre_tool_use: vec![make_hook("blocker", vec!["Read"], "exit 1")],
            post_tool_use: vec![],
            stop: vec![],
        };
        let engine = HookEngine::new(config);
        let result = engine.run_pre_tool_use("Read", &json!({})).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), HookError::Blocked { .. }));
    }

    #[tokio::test]
    async fn test_post_hook_runs_after_tool() {
        let config = HooksConfig {
            pre_tool_use: vec![],
            post_tool_use: vec![make_hook("post", vec!["Read"], "echo done")],
            stop: vec![],
        };
        let engine = HookEngine::new(config);
        let messages = engine.run_post_tool_use("Read", &json!({}), "output").await;
        assert!(!messages.is_empty());
        assert!(messages[0].contains("done"));
    }

    #[tokio::test]
    async fn test_hook_timeout() {
        let config = HooksConfig {
            pre_tool_use: vec![HookDef {
                name: "slow".to_string(),
                tool_match: vec!["Read".to_string()],
                file_match: vec![],
                command: "sleep 10".to_string(),
                timeout_ms: 100,
            }],
            post_tool_use: vec![],
            stop: vec![],
        };
        let engine = HookEngine::new(config);
        let result = engine.run_pre_tool_use("Read", &json!({})).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), HookError::Timeout(_)));
    }
}
