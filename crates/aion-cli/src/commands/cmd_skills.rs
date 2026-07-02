use std::env;
use std::path::Path;

use aion_skills::paths::{project_commands_dirs, project_skills_dirs, user_commands_dir, user_skills_dir};

use crate::cli::SkillsAction;

pub(crate) fn run(action: SkillsAction) -> anyhow::Result<()> {
    match action {
        SkillsAction::Path => print_skills_paths(),
    }
    Ok(())
}

fn print_skills_paths() {
    fn status(p: &Path) -> &'static str {
        if p.is_dir() { "exists" } else { "not found" }
    }

    match user_skills_dir() {
        Some(dir) => println!("User:    {}  ({})", dir.display(), status(&dir)),
        None => println!("User:    <unable to determine config directory>"),
    }

    let cwd = env::current_dir().unwrap_or_default();
    let project_dirs = project_skills_dirs(&cwd);
    if project_dirs.is_empty() {
        println!("Project: <none found>");
    } else {
        for dir in &project_dirs {
            println!("Project: {}  ({})", dir.display(), status(dir));
        }
    }

    let mut has_legacy = false;
    if let Some(dir) = user_commands_dir()
        && dir.is_dir()
    {
        println!("Legacy:  {}  ({})", dir.display(), status(&dir));
        has_legacy = true;
    }
    for dir in project_commands_dirs(&cwd) {
        println!("Legacy:  {}  ({})", dir.display(), status(&dir));
        has_legacy = true;
    }
    if !has_legacy {
        println!("Legacy:  <none found>");
    }
}
