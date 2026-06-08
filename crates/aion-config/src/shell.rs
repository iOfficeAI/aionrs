use std::process::Output;

use tokio::process::Command;

pub struct ShellInfo {
    pub program: &'static str,
    pub flag: &'static str,
}

pub fn shell_info() -> ShellInfo {
    if cfg!(windows) {
        ShellInfo {
            program: "cmd",
            flag: "/C",
        }
    } else {
        ShellInfo {
            program: "sh",
            flag: "-c",
        }
    }
}

/// Escape cmd.exe metacharacters in unquoted URL arguments.
///
/// When Rust's `Command` passes a command string to `cmd /C` via
/// `CreateProcess`, `"..."` in the string already protect their contents —
/// `&`, `|`, `<`, `>`, `^`, and `%` inside quotes are treated as literal
/// characters by cmd.exe. However, **unquoted** URL arguments are not
/// protected. For example, `curl https://example.com/?a=1&b=2` would be
/// split at `&`, and curl would receive only `https://example.com/?a=1`.
///
/// This function handles both cases correctly:
///
/// - **Quoted content** (`"..."`) — passed through unchanged; the double
///   quotes already provide protection. Note: `^` inside `"..."` is NOT
///   an escape in cmd.exe; it is literal. So we must not insert `^` there.
/// - **Unquoted URL tokens** (tokens outside `"..."` that start with
///   `https://`, `http://`, `ftp://`, or `file://`) — `^` is prefixed
///   before each metacharacter (`&`, `|`, `<`, `>`, `^`, `%`). Outside
///   `"..."`, cmd.exe DOES consume `^` as an escape and passes the next
///   character literally to the child process.
/// - **Everything else** (shell syntax, plain args, `%VAR%` expansion) —
///   left completely unchanged so pipes, redirects, and variable expansion
///   continue to work.
///
/// On non-Windows platforms the input is returned unchanged.
///
/// # Examples
///
/// ```
/// use aion_config::shell::escape_for_cmd;
///
/// // Unquoted URL: & is escaped on Windows so cmd doesn't split the command.
/// let unquoted = "curl https://example.com/?a=1&b=2";
/// #[cfg(windows)]
/// assert_eq!(escape_for_cmd(unquoted), "curl https://example.com/?a=1^&b=2");
/// #[cfg(not(windows))]
/// assert_eq!(escape_for_cmd(unquoted), unquoted);
///
/// // Quoted URL: left unchanged on all platforms (quotes already protect it).
/// let quoted = "curl \"https://example.com/?a=1&b=2\"";
/// assert_eq!(escape_for_cmd(quoted), quoted);
///
/// // Shell syntax outside a URL: left unchanged on all platforms.
/// assert_eq!(escape_for_cmd("echo hello | findstr h"), "echo hello | findstr h");
/// ```
pub fn escape_for_cmd(input: &str) -> String {
    if !cfg!(windows) {
        return input.to_string();
    }

    const URL_SCHEMES: &[&str] = &["https://", "http://", "ftp://", "file://"];

    let mut out = String::with_capacity(input.len() + 16);
    let mut in_double_quotes = false;
    let mut pos = 0;

    while pos < input.len() {
        let rest = &input[pos..];
        let ch = rest.chars().next().unwrap();
        let ch_len = ch.len_utf8();

        if ch == '"' {
            in_double_quotes = !in_double_quotes;
            out.push(ch);
            pos += ch_len;
            continue;
        }

        if in_double_quotes {
            out.push(ch);
            pos += ch_len;
            continue;
        }

        // Outside quotes: check if we are at the start of a URL token.
        if URL_SCHEMES.iter().any(|s| rest.starts_with(s)) {
            // Consume until whitespace, escaping cmd metacharacters as we go.
            // Outside "...", cmd DOES consume ^ as an escape prefix, so the
            // child process receives the bare metacharacter (e.g. &, |) rather
            // than the caret.
            while pos < input.len() {
                let c = input[pos..].chars().next().unwrap();
                if c.is_ascii_whitespace() {
                    break;
                }
                match c {
                    '^' | '&' | '|' | '<' | '>' | '%' => {
                        out.push('^');
                        out.push(c);
                    }
                    _ => out.push(c),
                }
                pos += c.len_utf8();
            }
            continue;
        }

        // Regular character — pass through unchanged.
        out.push(ch);
        pos += ch_len;
    }

    out
}

/// Normalize a few common Unix shell device paths to their Windows cmd
/// equivalents. This is intentionally narrow: only redirection targets to
/// `/dev/null` outside double quotes are rewritten.
fn normalize_windows_shell_compat(input: &str) -> String {
    if !cfg!(windows) {
        return input.to_string();
    }

    let mut out = String::with_capacity(input.len());
    let mut in_double_quotes = false;
    let mut pos = 0;

    while pos < input.len() {
        let rest = &input[pos..];
        let ch = rest.chars().next().unwrap();
        let ch_len = ch.len_utf8();

        if ch == '"' {
            in_double_quotes = !in_double_quotes;
            out.push(ch);
            pos += ch_len;
            continue;
        }

        if !in_double_quotes && rest.starts_with("/dev/null") {
            let preceded_by_redirect = out
                .chars()
                .rev()
                .find(|c| !c.is_ascii_whitespace())
                .is_some_and(|c| c == '>' || c == '<');

            if preceded_by_redirect {
                out.push_str("nul");
                pos += "/dev/null".len();
                continue;
            }
        }

        out.push(ch);
        pos += ch_len;
    }

    out
}

pub fn shell_command_builder(command_str: &str) -> Command {
    let info = shell_info();
    let command_str = normalize_windows_shell_compat(command_str);
    let command_str = escape_for_cmd(&command_str);
    let mut cmd = Command::new(info.program);
    cmd.arg(info.flag).arg(&command_str);
    cmd
}

pub async fn shell_command(command_str: &str) -> std::io::Result<Output> {
    shell_command_builder(command_str).output().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_info_returns_platform_appropriate_values() {
        let info = shell_info();
        if cfg!(windows) {
            assert_eq!(info.program, "cmd");
            assert_eq!(info.flag, "/C");
        } else {
            assert_eq!(info.program, "sh");
            assert_eq!(info.flag, "-c");
        }
    }

    // ── Unit tests: pure string transformation ──────────────────────────────

    // Quoted URLs and non-URL content are always returned unchanged.

    #[test]
    fn escape_for_cmd_is_identity_for_quoted_url_with_ampersand() {
        let input = "curl.exe -s \"https://example.com/?q=1&r=2\"";
        assert_eq!(escape_for_cmd(input), input);
    }

    #[test]
    fn escape_for_cmd_is_identity_for_pipe_outside_quotes() {
        let input = "echo hello | findstr h";
        assert_eq!(escape_for_cmd(input), input);
    }

    #[test]
    fn escape_for_cmd_is_identity_for_caret_inside_quotes() {
        let input = "echo \"^a\"";
        assert_eq!(escape_for_cmd(input), input);
    }

    #[test]
    fn escape_for_cmd_is_identity_for_percent_expansion() {
        // %VAR% outside a URL must be preserved so variable expansion works.
        let input = "echo %MY_VAR%";
        assert_eq!(escape_for_cmd(input), input);
    }

    #[test]
    fn escape_for_cmd_is_identity_for_percent_inside_quotes() {
        let input = "echo \"100%done\"";
        assert_eq!(escape_for_cmd(input), input);
    }

    #[test]
    fn escape_for_cmd_is_identity_for_safe_string() {
        let input = "echo hello world";
        assert_eq!(escape_for_cmd(input), input);
    }

    #[test]
    fn escape_for_cmd_is_identity_for_quoted_url_with_multiple_params() {
        let input = "curl.exe -s \"https://example.com/?lat=3.14&lon=101.69&tz=Asia/KL\"";
        assert_eq!(escape_for_cmd(input), input);
    }

    #[test]
    fn escape_for_cmd_is_identity_for_mixed_metacharacters_in_quotes() {
        let input = "echo \"a&b|c>d<e^f%g\" | findstr a";
        assert_eq!(escape_for_cmd(input), input);
    }

    #[test]
    fn escape_for_cmd_is_identity_for_shell_command_chaining() {
        // & between two plain commands is intentional syntax — must not be escaped.
        let input = "echo hello & echo world";
        assert_eq!(escape_for_cmd(input), input);
    }

    // Unquoted URL tokens: metacharacters must be escaped so cmd doesn't
    // interpret them as shell operators.

    #[test]
    fn escape_for_cmd_escapes_ampersand_in_unquoted_url() {
        let input = "curl https://example.com/?a=1&b=2";
        let escaped = escape_for_cmd(input);
        #[cfg(windows)]
        assert_eq!(escaped, "curl https://example.com/?a=1^&b=2");
        #[cfg(not(windows))]
        assert_eq!(escaped, input);
    }

    #[test]
    fn escape_for_cmd_escapes_multiple_params_in_unquoted_url() {
        let input = "curl https://example.com/?lat=3.14&lon=101.69&tz=Asia/KL";
        let escaped = escape_for_cmd(input);
        #[cfg(windows)]
        assert_eq!(
            escaped,
            "curl https://example.com/?lat=3.14^&lon=101.69^&tz=Asia/KL"
        );
        #[cfg(not(windows))]
        assert_eq!(escaped, input);
    }

    #[test]
    fn escape_for_cmd_escapes_url_ampersand_but_preserves_pipe() {
        // The pipe (|) after the URL is shell syntax and must be preserved;
        // the & inside the URL must be escaped.
        let input = "curl https://example.com/?a=1&b=2 | findstr something";
        let escaped = escape_for_cmd(input);
        #[cfg(windows)]
        assert_eq!(
            escaped,
            "curl https://example.com/?a=1^&b=2 | findstr something"
        );
        #[cfg(not(windows))]
        assert_eq!(escaped, input);
    }

    #[test]
    fn escape_for_cmd_handles_mixed_quoted_and_unquoted_urls() {
        // First URL is unquoted (escaped), second is quoted (unchanged).
        let input =
            "curl https://a.com/?x=1&y=2 -o out && curl \"https://b.com/?p=1&q=2\"";
        let escaped = escape_for_cmd(input);
        #[cfg(windows)]
        assert_eq!(
            escaped,
            "curl https://a.com/?x=1^&y=2 -o out && curl \"https://b.com/?p=1&q=2\""
        );
        #[cfg(not(windows))]
        assert_eq!(escaped, input);
    }

    #[test]
    fn normalize_windows_shell_compat_rewrites_dev_null_redirection() {
        let input = "curl -s \"https://example.com\" 2>/dev/null || echo fallback";
        #[cfg(windows)]
        assert_eq!(
            normalize_windows_shell_compat(input),
            "curl -s \"https://example.com\" 2>nul || echo fallback"
        );
        #[cfg(not(windows))]
        assert_eq!(normalize_windows_shell_compat(input), input);
    }

    #[test]
    fn normalize_windows_shell_compat_leaves_quoted_dev_null_unchanged() {
        let input = "echo \"/dev/null\"";
        assert_eq!(normalize_windows_shell_compat(input), input);
    }

    #[test]
    fn normalize_windows_shell_compat_handles_weather_command_shape() {
        let input =
            "curl -fsS \"https://wttr.in/KL?format=%C+%t+%h+%w\" 2>/dev/null || echo \"Unable to fetch weather\"";
        #[cfg(windows)]
        assert_eq!(
            normalize_windows_shell_compat(input),
            "curl -fsS \"https://wttr.in/KL?format=%C+%t+%h+%w\" 2>nul || echo \"Unable to fetch weather\""
        );
        #[cfg(not(windows))]
        assert_eq!(normalize_windows_shell_compat(input), input);
    }

    #[tokio::test]
    async fn shell_command_runs_echo() {
        let output = shell_command("echo hello")
            .await
            .expect("shell_command failed");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("hello"));
    }

    #[tokio::test]
    async fn shell_command_builder_allows_env_and_cwd() {
        let tmp = std::env::temp_dir();
        // %MY_VAR% on Windows and $MY_VAR on Unix are both passed through
        // unchanged by escape_for_cmd (neither is a URL token), so the shell
        // expands the variable correctly.
        let cmd_str = if cfg!(windows) {
            "echo %MY_VAR%"
        } else {
            "echo $MY_VAR"
        };
        let output = shell_command_builder(cmd_str)
            .env("MY_VAR", "test_value")
            .current_dir(&tmp)
            .output()
            .await
            .expect("builder failed");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("test_value"));
    }

    #[tokio::test]
    async fn shell_command_builder_preserves_shell_syntax_outside_quotes() {
        let cmd_str = if cfg!(windows) {
            "echo hello | findstr h"
        } else {
            "echo hello | grep h"
        };
        let output = shell_command_builder(cmd_str)
            .output()
            .await
            .expect("builder failed");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("hello"));
    }

    // ── Integration tests: full cmd /C round-trip (Windows only) ───────────

    #[cfg(windows)]
    #[tokio::test]
    async fn shell_command_builder_preserves_quoted_url_with_ampersand() {
        // Quoted URL: `"..."` already protect & at the CreateProcess level.
        // If & were split by cmd, echo would only output the left half of the
        // URL and the assertion would fail.
        let url = "https://example.com/?a=1&b=2";
        let command = format!("echo \"{url}\"");
        let output = shell_command_builder(&command)
            .output()
            .await
            .expect("builder failed");

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "cmd failed; stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            stdout.contains("a=1&b=2"),
            "stdout should contain the full URL with & intact, got: {stdout}"
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn shell_command_builder_preserves_unquoted_url_with_ampersand() {
        // Unquoted URL: escape_for_cmd inserts ^ before & so cmd treats it as
        // literal rather than a command separator.  Outside "...", cmd consumes
        // the ^ and passes the bare & to echo, so the output contains the full
        // URL with & intact.
        let url = "https://example.com/?a=1&b=2";
        let command = format!("echo {url}"); // no quotes around URL
        let output = shell_command_builder(&command)
            .output()
            .await
            .expect("builder failed");

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "cmd failed; stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            stdout.contains("a=1&b=2"),
            "stdout should contain the full URL with & intact, got: {stdout}"
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn shell_command_builder_converts_dev_null_redirection_on_windows() {
        let output = shell_command_builder("echo hello 2>/dev/null")
            .output()
            .await
            .expect("builder failed");

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "cmd failed; stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(stdout.contains("hello"));
    }
}
