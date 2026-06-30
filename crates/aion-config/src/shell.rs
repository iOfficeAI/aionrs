use std::path::{Path, PathBuf};
use std::process::Output;

use serde::{Deserialize, Serialize};
use tokio::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShellKind {
    Bash,
    Zsh,
    Sh,
    PowerShell,
    Cmd,
}

impl ShellKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Zsh => "zsh",
            Self::Sh => "sh",
            Self::PowerShell => "powershell",
            Self::Cmd => "cmd",
        }
    }

    pub fn syntax_label(self) -> &'static str {
        self.name()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedShell {
    pub kind: ShellKind,
    pub path: PathBuf,
}

impl ResolvedShell {
    pub fn new(kind: ShellKind, path: PathBuf) -> Self {
        Self { kind, path }
    }

    pub fn derive_exec_args(&self, command: &str, login: bool) -> Vec<String> {
        match self.kind {
            ShellKind::Bash | ShellKind::Zsh | ShellKind::Sh => {
                vec![if login { "-lc" } else { "-c" }.to_string(), command.to_string()]
            }
            ShellKind::PowerShell => {
                let command = prefix_powershell_script_with_utf8(command);
                let mut args = Vec::new();
                if !login {
                    args.push("-NoProfile".to_string());
                }
                args.push("-Command".to_string());
                args.push(command);
                args
            }
            ShellKind::Cmd => vec!["/C".to_string(), command.to_string()],
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ShellConfig {
    #[serde(default = "default_shell_default")]
    pub default: String,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            default: default_shell_default(),
        }
    }
}

fn default_shell_default() -> String {
    "auto".to_string()
}

#[derive(Debug, thiserror::Error)]
pub enum ShellError {
    #[error(
        "unsupported shell '{0}'; supported shells are auto, pwsh, powershell, cmd, bash, zsh, sh, or a path to one of those executables"
    )]
    UnsupportedShell(String),
    #[error("configured shell '{0}' was recognized as {1}, but no executable was found")]
    ShellUnavailable(String, &'static str),
    #[error("configured shell path '{0}' does not exist or is not a file")]
    PathUnavailable(String),
}

pub fn resolve_shell_config(config: &ShellConfig) -> Result<ResolvedShell, ShellError> {
    let shell = resolve_shell(Some(config.default.as_str()));
    if let Ok(shell) = &shell {
        tracing::info!(
            target: "aion_config::shell",
            shell_kind = shell.kind.name(),
            shell_path = %shell.path.display(),
            "resolved configured shell"
        );
    }
    shell
}

pub fn resolve_shell(requested: Option<&str>) -> Result<ResolvedShell, ShellError> {
    let requested = requested.map(str::trim).filter(|s| !s.is_empty()).unwrap_or("auto");

    if requested.eq_ignore_ascii_case("auto") {
        return Ok(default_shell());
    }

    if let Some(kind) = shell_kind_from_alias(requested) {
        return resolve_kind(kind, Some(requested)).ok_or_else(|| {
            tracing::warn!(
                target: "aion_config::shell",
                requested_shell = requested,
                shell_kind = kind.name(),
                "configured shell executable was not found"
            );
            ShellError::ShellUnavailable(requested.to_string(), kind.name())
        });
    }

    let path = PathBuf::from(requested);
    if path.components().count() > 1 || path.is_absolute() || requested.contains('\\') {
        let kind = detect_shell_kind(&path).ok_or_else(|| ShellError::UnsupportedShell(requested.to_string()))?;
        if !path.is_file() {
            return Err(ShellError::PathUnavailable(requested.to_string()));
        }
        return Ok(ResolvedShell::new(kind, path));
    }

    Err(ShellError::UnsupportedShell(requested.to_string()))
}

pub fn default_shell() -> ResolvedShell {
    let shell = default_shell_from_user_shell(user_shell_path());
    tracing::debug!(
        target: "aion_config::shell",
        shell_kind = shell.kind.name(),
        shell_path = %shell.path.display(),
        "resolved default shell"
    );
    shell
}

pub fn detect_shell_kind(path: impl AsRef<Path>) -> Option<ShellKind> {
    let value = path.as_ref().as_os_str().to_string_lossy();
    let file_name = value
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(value.as_ref())
        .to_ascii_lowercase();
    let stem = file_name.strip_suffix(".exe").unwrap_or(&file_name);
    shell_kind_from_alias(stem)
}

pub fn shell_command_builder(shell: &ResolvedShell, command_str: &str, login: bool) -> Command {
    let mut cmd = Command::new(&shell.path);
    cmd.args(shell.derive_exec_args(command_str, login));
    cmd
}

pub async fn shell_command(command_str: &str) -> std::io::Result<Output> {
    let shell = default_shell();
    shell_command_builder(&shell, command_str, false).output().await
}

pub fn render_shell_prompt(shell: &ResolvedShell) -> String {
    format!(
        "Default shell: {}\nShell path: {}\nShell syntax: {}",
        shell.kind.name(),
        shell.path.display(),
        shell.kind.syntax_label()
    )
}

fn shell_kind_from_alias(value: &str) -> Option<ShellKind> {
    match value.to_ascii_lowercase().as_str() {
        "bash" => Some(ShellKind::Bash),
        "zsh" => Some(ShellKind::Zsh),
        "sh" => Some(ShellKind::Sh),
        "pwsh" | "powershell" => Some(ShellKind::PowerShell),
        "cmd" => Some(ShellKind::Cmd),
        _ => None,
    }
}

fn default_shell_from_user_shell(user_shell: Option<PathBuf>) -> ResolvedShell {
    if cfg!(windows) {
        resolve_kind(ShellKind::PowerShell, None)
            .unwrap_or_else(|| ResolvedShell::new(ShellKind::Cmd, PathBuf::from("cmd.exe")))
    } else {
        let user_default = user_shell
            .as_ref()
            .and_then(detect_shell_kind)
            .and_then(|kind| resolve_kind(kind, None));

        let fallback = if cfg!(target_os = "macos") {
            user_default
                .or_else(|| resolve_kind(ShellKind::Zsh, None))
                .or_else(|| resolve_kind(ShellKind::Bash, None))
        } else {
            user_default
                .or_else(|| resolve_kind(ShellKind::Bash, None))
                .or_else(|| resolve_kind(ShellKind::Zsh, None))
        };

        fallback
            .or_else(|| resolve_kind(ShellKind::Sh, None))
            .unwrap_or_else(|| ResolvedShell::new(ShellKind::Sh, PathBuf::from("/bin/sh")))
    }
}

fn resolve_kind(kind: ShellKind, requested: Option<&str>) -> Option<ResolvedShell> {
    let requested_path = requested.map(Path::new);
    let candidates: Vec<PathBuf> = match (kind, requested, requested_path) {
        (_, Some(path), Some(path_ref))
            if path_ref.components().count() > 1 || path_ref.is_absolute() || path.contains('\\') =>
        {
            vec![PathBuf::from(path)]
        }
        (ShellKind::PowerShell, Some("pwsh"), _) => candidate_paths("pwsh", pwsh_fallback_paths()),
        (ShellKind::PowerShell, _, _) => {
            let mut paths = candidate_paths("pwsh", pwsh_fallback_paths());
            paths.extend(candidate_paths("powershell", powershell_fallback_paths()));
            paths
        }
        (ShellKind::Cmd, _, _) => candidate_paths("cmd", &[]),
        (ShellKind::Bash, _, _) => candidate_paths("bash", &["/bin/bash", "/usr/bin/bash"]),
        (ShellKind::Zsh, _, _) => candidate_paths("zsh", &["/bin/zsh"]),
        (ShellKind::Sh, _, _) => candidate_paths("sh", &["/bin/sh"]),
    };

    candidates
        .into_iter()
        .find(|path| path.is_file() || path.components().count() == 1)
        .map(|path| ResolvedShell::new(kind, path))
}

fn candidate_paths(binary: &str, fallback_paths: &[&str]) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(path) = which::which(binary) {
        paths.push(path);
    }
    paths.extend(fallback_paths.iter().map(PathBuf::from));
    paths
}

fn pwsh_fallback_paths() -> &'static [&'static str] {
    if cfg!(windows) {
        &[r"C:\Program Files\PowerShell\7\pwsh.exe"]
    } else {
        &["/usr/local/bin/pwsh"]
    }
}

fn powershell_fallback_paths() -> &'static [&'static str] {
    if cfg!(windows) {
        &[r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"]
    } else {
        &[]
    }
}

#[cfg(unix)]
fn user_shell_path() -> Option<PathBuf> {
    std::env::var_os("SHELL").map(PathBuf::from)
}

#[cfg(not(unix))]
fn user_shell_path() -> Option<PathBuf> {
    None
}

fn prefix_powershell_script_with_utf8(command: &str) -> String {
    format!("try {{ [Console]::OutputEncoding=[System.Text.Encoding]::UTF8 }} catch {{}}\n{command}")
}

#[cfg(test)]
#[path = "shell_test.rs"]
mod shell_test;
