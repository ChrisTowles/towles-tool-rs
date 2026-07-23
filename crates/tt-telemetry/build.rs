//! Embeds the building commit's git SHA into the binary via `TT_BUILD_SHA`,
//! read back at compile time by `env!("TT_BUILD_SHA")` in `src/lib.rs`. This
//! is what lets a telemetry record say which commit produced it.

use std::process::Command;

fn main() {
    let sha = git_head_sha().unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=TT_BUILD_SHA={sha}");

    // Worktree tasks' `.git` is a file pointing at the main checkout's real
    // git dir, so ask git to resolve it rather than assuming `../../.git`.
    if let Some(git_dir) = git_dir() {
        println!("cargo:rerun-if-changed={git_dir}/HEAD");
        println!("cargo:rerun-if-changed={git_dir}/logs/HEAD");
    }
}

fn git_dir() -> Option<String> {
    let output = Command::new("git").args(["rev-parse", "--git-dir"]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let dir = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if dir.is_empty() { None } else { Some(dir) }
}

fn git_head_sha() -> Option<String> {
    let output = Command::new("git").args(["rev-parse", "HEAD"]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if sha.is_empty() { None } else { Some(sha) }
}
