use std::path::{Path, PathBuf};

use aion_config::config::app_config_dir;

// ---------------------------------------------------------------------------
// User-level directories (<config_dir>/aionrs/)
// ---------------------------------------------------------------------------

/// Return the user-level skills directory: `<config_dir>/aionrs/skills/`
///
/// Returns `None` if the platform config directory cannot be determined.
pub fn user_skills_dir() -> Option<PathBuf> {
    app_config_dir().map(|d| d.join("skills"))
}

/// Return the user-level legacy commands directory: `<config_dir>/aionrs/commands/`
pub fn user_commands_dir() -> Option<PathBuf> {
    app_config_dir().map(|d| d.join("commands"))
}

// ---------------------------------------------------------------------------
// Project-level directories (walk up from cwd)
// ---------------------------------------------------------------------------

/// Find all project-level `.aionrs/skills/` directories by walking up from
/// `cwd` to the nearest git root (or home directory), returning deepest-first.
///
/// Deepest-first means the most-specific project directory wins in the
/// priority ordering (closer to cwd = higher priority).
pub fn project_skills_dirs(cwd: &Path) -> Vec<PathBuf> {
    walk_up_dirs(cwd, "skills")
}

/// Find all project-level `.aionrs/commands/` directories (legacy), same walk.
pub fn project_commands_dirs(cwd: &Path) -> Vec<PathBuf> {
    walk_up_dirs(cwd, "commands")
}

/// Resolve additional skill directories from `--add-dir` paths.
///
/// Each path in `add_dirs` is checked for a `.aionrs/skills/` subdirectory.
/// Only directories that exist are included.
pub fn additional_skills_dirs(add_dirs: &[PathBuf]) -> Vec<PathBuf> {
    add_dirs
        .iter()
        .map(|d| d.join(".aionrs").join("skills"))
        .filter(|p| p.is_dir())
        .collect()
}

// ---------------------------------------------------------------------------
// Git root detection
// ---------------------------------------------------------------------------

/// Find the nearest git root from `start` by walking up looking for a `.git`
/// entry (file or directory). Returns `None` if no `.git` is found before
/// reaching the filesystem root.
pub fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        if current.join(".git").exists() {
            return Some(current);
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => return None,
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Walk up from `cwd` to the git root (or home directory), collecting all
/// `.aionrs/<subdir>/` directories that exist. Returns deepest-first.
fn walk_up_dirs(cwd: &Path, subdir: &str) -> Vec<PathBuf> {
    let stop_at = stop_boundary(cwd);
    let mut dirs = Vec::new();
    let mut current = cwd.to_path_buf();

    loop {
        let candidate = current.join(".aionrs").join(subdir);
        if candidate.is_dir() {
            dirs.push(candidate);
        }

        // Stop if we've reached the boundary or the filesystem root
        if Some(&current) == stop_at.as_ref() || current.parent().is_none() {
            break;
        }

        match current.parent() {
            Some(parent) if parent != current.as_path() => {
                current = parent.to_path_buf();
            }
            _ => break,
        }
    }

    dirs
}

/// Determine where to stop walking up. Stops at git root if found,
/// otherwise at the user home directory.
pub fn stop_boundary(cwd: &Path) -> Option<PathBuf> {
    find_git_root(cwd).or_else(dirs::home_dir)
}

#[cfg(test)]
#[path = "paths_test.rs"]
mod paths_test;
