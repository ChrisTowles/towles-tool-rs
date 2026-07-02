//! `tt agentboard` (alias `ag`): manage the agentboard desktop app's watched
//! repos. Operates on the same `~/.config/towles-tool/agentboard/repos.json` the
//! app reads; the app re-reads it on every scan, so changes here are picked up
//! live (no restart).

use std::path::Path;

use crate::cli::{AgentboardCommands, ReposCommands};
use crate::ui;
use tt_agentboard::repos::{add_repo, default_repos_path, load_repos, repo_entries, save_repos};

pub fn run(command: AgentboardCommands) -> i32 {
    match command {
        AgentboardCommands::Repos(args) => match args.command {
            None => list_repos(),
            Some(ReposCommands::Add { path }) => add(&path),
            Some(ReposCommands::Remove { target }) => remove(&target),
        },
    }
}

fn list_repos() -> i32 {
    let path = default_repos_path();
    let repos = load_repos(&path);
    if repos.is_empty() {
        ui::info("No repos configured. Add one with `tt agentboard repos add <path>`.");
        return 0;
    }
    for entry in repo_entries(&repos) {
        println!("{}  {}", entry.name, entry.dir);
    }
    0
}

fn add(path: &str) -> i32 {
    // Resolve to an absolute path so the app (with its own cwd) agrees.
    let abs = match std::fs::canonicalize(path) {
        Ok(p) => p.to_string_lossy().to_string(),
        Err(_) => {
            ui::error(&format!("Path does not exist: {path}"));
            return 1;
        }
    };
    if !Path::new(&abs).is_dir() {
        ui::error(&format!("Not a directory: {abs}"));
        return 1;
    }
    if !Path::new(&abs).join(".git").exists() {
        ui::warning(&format!("{abs} is not a git repository (adding anyway)"));
    }

    let repos_path = default_repos_path();
    let mut repos = load_repos(&repos_path);
    if !add_repo(&mut repos, &abs) {
        ui::info(&format!("Already watching: {abs}"));
        return 0;
    }
    if let Err(e) = save_repos(&repos_path, &repos) {
        ui::error(&format!("Failed to save repos: {e}"));
        return 1;
    }
    ui::success(&format!("Added {abs}"));
    0
}

fn remove(target: &str) -> i32 {
    let repos_path = default_repos_path();
    let mut repos = load_repos(&repos_path);

    // Match by session name first, then by exact configured path.
    let by_name = repo_entries(&repos).into_iter().find(|e| e.name == target).map(|e| e.dir);
    let dir_to_remove = by_name.or_else(|| repos.iter().find(|p| *p == target).cloned());

    let Some(dir) = dir_to_remove else {
        ui::error(&format!("No watched repo matching: {target}"));
        return 1;
    };
    repos.retain(|p| p != &dir);
    if let Err(e) = save_repos(&repos_path, &repos) {
        ui::error(&format!("Failed to save repos: {e}"));
        return 1;
    }
    ui::success(&format!("Removed {dir}"));
    0
}
