//! `ttr agentboard` (alias `ag`): manage the agentboard desktop app's watched
//! repos. Operates on the same `~/.config/towles-tool/agentboard/repos.json` the
//! app reads; the app re-reads it on every scan, so changes here are picked up
//! live (no restart).

use std::path::Path;

use crate::cli::{AgentboardCommands, ReposCommands, SessionsCommands};
use crate::ui;
use tt_agentboard::engine::now_ms;
use tt_agentboard::repos::{
    add_repo_persisted, default_repos_path, load_repos, remove_repo_persisted, repo_entries,
};
use tt_agentboard::sessions::{SessionStore, default_sessions_path};

pub fn run(command: AgentboardCommands) -> i32 {
    match command {
        AgentboardCommands::Repos(args) => match args.command {
            None => list_repos(),
            Some(ReposCommands::Add { path }) => add(&path),
            Some(ReposCommands::Remove { target }) => remove(&target),
        },
        AgentboardCommands::Sessions(args) => match args.command {
            None => list_sessions(),
            Some(SessionsCommands::Add { path, name }) => add_session(&path, name.as_deref()),
            Some(SessionsCommands::Rename { id, name }) => rename_session(&id, &name),
            Some(SessionsCommands::Remove { id }) => remove_session(&id),
        },
    }
}

fn list_repos() -> i32 {
    let path = default_repos_path();
    let repos = load_repos(&path);
    if repos.is_empty() {
        ui::info("No repos configured. Add one with `ttr agentboard repos add <path>`.");
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
    match add_repo_persisted(&repos_path, &abs) {
        Ok((_, true)) => {
            ui::success(&format!("Added {abs}"));
            0
        }
        Ok((_, false)) => {
            ui::info(&format!("Already watching: {abs}"));
            0
        }
        Err(e) => {
            ui::error(&format!("Failed to save repos: {e}"));
            1
        }
    }
}

fn remove(target: &str) -> i32 {
    let repos_path = default_repos_path();
    let repos = load_repos(&repos_path);

    // Match by session name first, then by exact configured path.
    let by_name = repo_entries(&repos).into_iter().find(|e| e.name == target).map(|e| e.dir);
    let dir_to_remove = by_name.or_else(|| repos.iter().find(|p| *p == target).cloned());

    let Some(dir) = dir_to_remove else {
        ui::error(&format!("No watched repo matching: {target}"));
        return 1;
    };
    match remove_repo_persisted(&repos_path, &dir) {
        Ok(_) => {
            ui::success(&format!("Removed {dir}"));
            0
        }
        Err(e) => {
            ui::error(&format!("Failed to save repos: {e}"));
            1
        }
    }
}

fn list_sessions() -> i32 {
    let store = SessionStore::new(Some(default_sessions_path()));
    let mut any = false;
    for (dir, sessions) in store.iter() {
        any = true;
        println!("{dir}");
        for s in sessions {
            println!("  {}  {}", s.id, s.name);
        }
    }
    if !any {
        ui::info(
            "No sessions yet. The app seeds a default shell per folder, or add one with `ttr agentboard sessions add <path>`.",
        );
    }
    0
}

fn add_session(path: &str, name: Option<&str>) -> i32 {
    let abs = match std::fs::canonicalize(path) {
        Ok(p) => p.to_string_lossy().to_string(),
        Err(_) => {
            ui::error(&format!("Path does not exist: {path}"));
            return 1;
        }
    };
    let sessions_path = default_sessions_path();
    let mut store = SessionStore::new(Some(sessions_path));
    let record = store.add(&abs, name, now_ms());
    if let Err(e) = store.save() {
        ui::error(&format!("Failed to save sessions: {e}"));
        return 1;
    }
    ui::success(&format!("Added session {} ({}) to {abs}", record.name, record.id));
    0
}

fn rename_session(id: &str, name: &str) -> i32 {
    let mut store = SessionStore::new(Some(default_sessions_path()));
    if !store.rename(id, name) {
        ui::error(&format!("No session with id: {id}"));
        return 1;
    }
    if let Err(e) = store.save() {
        ui::error(&format!("Failed to save sessions: {e}"));
        return 1;
    }
    ui::success(&format!("Renamed {id} to {name}"));
    0
}

fn remove_session(id: &str) -> i32 {
    let mut store = SessionStore::new(Some(default_sessions_path()));
    if !store.remove(id) {
        ui::error(&format!("No session with id: {id}"));
        return 1;
    }
    if let Err(e) = store.save() {
        ui::error(&format!("Failed to save sessions: {e}"));
        return 1;
    }
    ui::success(&format!("Removed session {id}"));
    0
}
