use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::{ShellKind, default_shell};
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

    fn slow_stdout_command(message: &str) -> String {
        match default_shell().kind {
            ShellKind::PowerShell => {
                format!("Write-Output {message}; Start-Sleep -Seconds 5")
            }
            ShellKind::Cmd => format!("echo {message} & ping -n 6 127.0.0.1 > nul"),
            ShellKind::Bash | ShellKind::Zsh | ShellKind::Sh => {
                format!("printf '{message}\\n'; sleep 5")
            }
        }
    }

    fn env_equals_command(name: &str, expected: &str) -> String {
        match default_shell().kind {
            ShellKind::PowerShell => {
                format!("if ($env:{name} -eq '{expected}') {{ exit 0 }} else {{ exit 1 }}")
            }
            ShellKind::Cmd => {
                format!(r#"if "%{name}%"=="{expected}" (exit /b 0) else (exit /b 1)"#)
            }
            ShellKind::Bash | ShellKind::Zsh | ShellKind::Sh => {
                format!(r#"[ "${name}" = "{expected}" ]"#)
            }
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
        let engine = HookEngine::new(HooksConfig::default(), std::env::temp_dir());
        assert!(!engine.has_hooks());
    }

    #[test]
    fn test_has_hooks_with_config() {
        let config = HooksConfig {
            pre_tool_use: vec![make_hook("pre", vec!["*"], "echo ok")],
            post_tool_use: vec![],
            stop: vec![],
        };
        let engine = HookEngine::new(config, std::env::temp_dir());
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
        let engine = HookEngine::new(config, std::env::temp_dir());
        let result = engine.run_pre_tool_use("Read", &json!({})).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_pre_hook_receives_runtime_env() {
        let config = HooksConfig {
            pre_tool_use: vec![make_hook(
                "runtime-env",
                vec!["Read"],
                &env_equals_command("AION_RUNTIME_ENV_TEST", "hook-value"),
            )],
            post_tool_use: vec![],
            stop: vec![],
        };
        let engine = HookEngine::new_with_env(
            config,
            std::env::temp_dir(),
            vec![("AION_RUNTIME_ENV_TEST".to_string(), "hook-value".to_string())],
        );

        let result = engine.run_pre_tool_use("Read", &json!({})).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_hook_vars_override_runtime_env() {
        let config = HooksConfig {
            pre_tool_use: vec![make_hook(
                "tool-env",
                vec!["Read"],
                &env_equals_command("TOOL_NAME", "Read"),
            )],
            post_tool_use: vec![],
            stop: vec![],
        };
        let engine = HookEngine::new_with_env(
            config,
            std::env::temp_dir(),
            vec![("TOOL_NAME".to_string(), "from-runtime".to_string())],
        );

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
        let engine = HookEngine::new(config, std::env::temp_dir());
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
        let engine = HookEngine::new(config, std::env::temp_dir());
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
        let engine = HookEngine::new(config, std::env::temp_dir());
        let result = engine.run_pre_tool_use("Read", &json!({})).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), HookError::Timeout { .. }));
    }

    #[tokio::test]
    async fn test_hook_timeout_preserves_stdout_emitted_before_timeout() {
        let command = slow_stdout_command("hook_stdout_before_timeout");
        let timeout_ms = if cfg!(windows) { 1500 } else { 100 };

        let result = run_hook_command(&command, &HashMap::new(), timeout_ms, &std::env::temp_dir()).await;

        let err = match result {
            Ok(_) => panic!("hook command should time out"),
            Err(err) => err,
        };
        let message = err.to_string();
        assert!(
            message.contains(&format!("Hook timed out after {timeout_ms}ms")),
            "timeout message missing: {message}"
        );
        assert!(
            message.contains("hook_stdout_before_timeout"),
            "stdout emitted before timeout should be preserved, got: {message}"
        );
    }
}

// ---------------------------------------------------------------------------
// Phase 11 tests — merge_hooks() (TC-11.30 ~ TC-11.38)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod phase11_tests {
    use super::*;

    fn make_hook(name: &str) -> HookDef {
        HookDef {
            name: name.to_string(),
            tool_match: vec![],
            file_match: vec![],
            command: "echo ok".to_string(),
            timeout_ms: 30_000,
        }
    }

    fn make_config_pre(names: &[&str]) -> HooksConfig {
        HooksConfig {
            pre_tool_use: names.iter().map(|n| make_hook(n)).collect(),
            post_tool_use: vec![],
            stop: vec![],
        }
    }

    // TC-11.30: pre_tool_use count accumulates correctly
    #[test]
    fn tc_11_30_pre_tool_use_count_accumulates() {
        let mut engine = HookEngine::new(make_config_pre(&["pre-a"]), std::env::temp_dir());
        let additional = HooksConfig {
            pre_tool_use: vec![make_hook("pre-b"), make_hook("pre-c")],
            post_tool_use: vec![],
            stop: vec![],
        };
        engine.merge_hooks(additional);
        assert_eq!(engine.config.pre_tool_use.len(), 3);
    }

    // TC-11.31: post_tool_use count accumulates correctly
    #[test]
    fn tc_11_31_post_tool_use_count_accumulates() {
        let mut engine = HookEngine::new(HooksConfig::default(), std::env::temp_dir());
        let additional = HooksConfig {
            pre_tool_use: vec![],
            post_tool_use: vec![make_hook("post-a")],
            stop: vec![],
        };
        engine.merge_hooks(additional);
        assert_eq!(engine.config.post_tool_use.len(), 1);
    }

    // TC-11.32: stop count accumulates correctly
    #[test]
    fn tc_11_32_stop_count_accumulates() {
        let initial = HooksConfig {
            pre_tool_use: vec![],
            post_tool_use: vec![],
            stop: vec![make_hook("stop-a")],
        };
        let mut engine = HookEngine::new(initial, std::env::temp_dir());
        let additional = HooksConfig {
            pre_tool_use: vec![],
            post_tool_use: vec![],
            stop: vec![make_hook("stop-b")],
        };
        engine.merge_hooks(additional);
        assert_eq!(engine.config.stop.len(), 2);
    }

    // TC-11.33: merging empty config doesn't change existing hooks
    #[test]
    fn tc_11_33_merge_empty_does_not_change_existing() {
        let mut engine = HookEngine::new(make_config_pre(&["pre-a", "pre-b"]), std::env::temp_dir());
        engine.merge_hooks(HooksConfig::default());
        assert_eq!(engine.config.pre_tool_use.len(), 2);
    }

    // TC-11.34: has_hooks() is true after merging
    #[test]
    fn tc_11_34_has_hooks_true_after_merge() {
        let mut engine = HookEngine::new(HooksConfig::default(), std::env::temp_dir());
        assert!(!engine.has_hooks(), "precondition: engine starts with no hooks");
        engine.merge_hooks(make_config_pre(&["pre-a"]));
        assert!(engine.has_hooks(), "TC-11.34: has_hooks must be true after merge");
    }

    // TC-11.35: multiple successive merges accumulate correctly (different names)
    #[test]
    fn tc_11_35_successive_merges_accumulate() {
        let mut engine = HookEngine::new(HooksConfig::default(), std::env::temp_dir());
        engine.merge_hooks(make_config_pre(&["a"]));
        engine.merge_hooks(make_config_pre(&["b"]));
        engine.merge_hooks(make_config_pre(&["c"]));
        assert_eq!(engine.config.pre_tool_use.len(), 3);
    }

    // TC-11.36: merging stop hooks does not affect pre_tool_use
    #[test]
    fn tc_11_36_merge_stop_does_not_affect_pre() {
        let mut engine = HookEngine::new(make_config_pre(&["pre-a"]), std::env::temp_dir());
        let additional = HooksConfig {
            pre_tool_use: vec![],
            post_tool_use: vec![],
            stop: vec![make_hook("stop-x")],
        };
        engine.merge_hooks(additional);
        assert_eq!(engine.config.pre_tool_use.len(), 1, "TC-11.36: pre unchanged");
        assert_eq!(engine.config.stop.len(), 1, "TC-11.36: stop added");
    }

    // TC-11.37: same-name hook not duplicated (idempotent dedup — C-4)
    #[test]
    fn tc_11_37_same_name_hook_not_duplicated() {
        let mut engine = HookEngine::new(HooksConfig::default(), std::env::temp_dir());
        let config = make_config_pre(&["skill:my-skill:pre_tool_use:0"]);
        engine.merge_hooks(config.clone());
        engine.merge_hooks(config);
        assert_eq!(
            engine.config.pre_tool_use.len(),
            1,
            "TC-11.37: same-name hook must not be duplicated"
        );
    }

    // TC-11.38: different-name hooks both appended (no false dedup — C-4)
    #[test]
    fn tc_11_38_different_name_hooks_both_appended() {
        let mut engine = HookEngine::new(HooksConfig::default(), std::env::temp_dir());
        engine.merge_hooks(make_config_pre(&["hook-a"]));
        engine.merge_hooks(make_config_pre(&["hook-b"]));
        assert_eq!(
            engine.config.pre_tool_use.len(),
            2,
            "TC-11.38: different-name hooks must both be appended"
        );
    }
}
