//! Task creation: worktree add, `.env` render with port claims, sibling
//! secret inheritance, and the setup step.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::render::render_task_env;
use super::{
    FETCH_TIMEOUT, OpsError, Result, base_branch, discover_root, effective_origin_base,
    fast_forward_base_if_behind, git_checkout, git_checkout_timeout, note_if_slow, run_setup,
    validate_branch_name,
};
use crate::{envfile, layout};

#[derive(Debug, Default)]
pub struct CreateOpts {
    /// Task root; `None` walks up from the current working directory.
    pub root: Option<PathBuf>,
    /// Branch to create and check out. Tasks are branch-named and ephemeral —
    /// there is no detached/parked mode.
    pub branch: String,
    /// Base ref for the new branch; `None` = the checkout's branch.
    pub base: Option<String>,
    /// Run the setup step in the new task (declared `TT_TASK_SETUP` from the
    /// rendered `.env`, else lockfile-detected package-manager install).
    pub run_setup: bool,
}

pub struct CreatedTask {
    pub name: String,
    pub dir: PathBuf,
    pub branch: String,
    pub base: String,
    /// The ref the task effectively branched from — `origin/<base>` when the
    /// creation-time fast-forward applied ([`effective_origin_base`]), else
    /// `base`. Display/prompt honesty; `base` stays the branch-name value.
    pub base_label: String,
    pub ports: Vec<(String, u16)>,
    pub inherited: usize,
    pub warnings: Vec<String>,
}

/// Create the task for `branch`: worktree under `tasks/`, rendered `.env`
/// with port claims, sibling-secrets inheritance, setup step.
/// `now_ms` (epoch ms) stamps the port registry's `claimed_at_ms` — read at
/// the CLI/app boundary, never here.
pub fn create_task(opts: &CreateOpts, now_ms: i64) -> Result<CreatedTask> {
    let sr = discover_root(opts.root.as_deref())?;
    validate_branch_name(&opts.branch)?;
    let mut warnings = Vec::new();
    let _ = git_checkout(&sr.checkout, &["worktree", "prune"]);

    let fetch_start = Instant::now();
    match git_checkout_timeout(&sr.checkout, &["fetch", "--quiet", "origin"], FETCH_TIMEOUT) {
        Ok(out) if out.ok() => {}
        Ok(out) => warnings
            .push(format!("fetch failed (offline?) — using local refs: {}", out.stderr.trim())),
        // Includes a timed-out fetch (a stalled/inspected connection) — the
        // old `if let Ok(..) = .. && !out.ok()` form silently dropped this
        // case instead of warning on it.
        Err(e) => warnings.push(format!("fetch failed — using local refs: {e}")),
    }
    note_if_slow(&mut warnings, "fetch", fetch_start.elapsed());

    let base = opts.base.clone().unwrap_or_else(|| base_branch(&sr.checkout));
    // The ref this task effectively branches from — probed after the fetch so
    // a just-created remote counterpart counts, and carried on the result as
    // `base_label` so the UI and the dynamic-flow prompt name the same ref
    // creation actually used (agreeing with `checkout_branches`' labels).
    let effective = effective_origin_base(&sr.checkout, &base);
    if let Some(upstream) = &effective {
        fast_forward_base_if_behind(&sr, &base, upstream, &mut warnings);
    }
    let base_label = effective.unwrap_or_else(|| base.clone());
    let name = layout::task_name_from_branch(&opts.branch)
        .ok_or_else(|| OpsError::BadBranchName(opts.branch.clone()))?;
    let dir = sr.task_dir(&name);
    if dir.exists() {
        return Err(OpsError::TaskExists { name, dir: dir.display().to_string() });
    }
    fs::create_dir_all(sr.tasks_dir())
        .map_err(|e| OpsError::Io(format!("cannot create {}: {e}", sr.tasks_dir().display())))?;
    let dir_s = dir.to_string_lossy().to_string();

    let worktree_start = Instant::now();
    let add_result =
        git_checkout(&sr.checkout, &["worktree", "add", "-b", &opts.branch, &dir_s, &base])?;
    if !add_result.ok() {
        return Err(OpsError::Git(format!(
            "git worktree add failed:\n{}",
            add_result.stderr.trim()
        )));
    }
    note_if_slow(&mut warnings, "git worktree add", worktree_start.elapsed());

    // From here on, any failure must remove the worktree just added above —
    // otherwise (e.g. a template render error) it leaves a half-set-up task
    // behind: a real worktree with no rendered `.env`, invisible as "failed"
    // to `tt task ls` and blocking a retry with `TaskExists`.
    let created = (|| -> Result<CreatedTask> {
        let summary = render_task_env(&sr, &dir, Some(&base), now_ms)?;
        warnings.extend(summary.warnings);

        // Inherit secrets from the first sibling checkout that has a .env —
        // the main checkout first (`sr.checkouts()` orders it that way; it's
        // the longest-lived and least likely to carry stale branch-specific
        // values), else the alphabetically-first task. Surfaced in a warning
        // when it wasn't the main checkout, since a task's secrets can be
        // branch-specific or stale in a way the main checkout's never are.
        let mut inherited = 0;
        for sib_dir in sr.checkouts() {
            if sib_dir == dir {
                continue;
            }
            if let Ok(sib_env) = fs::read_to_string(sib_dir.join(".env")) {
                let env_path = dir.join(".env");
                let current = fs::read_to_string(&env_path).unwrap_or_default();
                let (merged, count) = envfile::merge_missing_keys(&current, &sib_env);
                fs::write(&env_path, merged)
                    .map_err(|e| OpsError::Io(format!("cannot write .env: {e}")))?;
                inherited = count;
                if count > 0 && sib_dir != sr.checkout {
                    let source =
                        sib_dir.file_name().and_then(|n| n.to_str()).unwrap_or("a sibling task");
                    warnings.push(format!(
                        "inherited {count} .env key(s) from {source}, not the main checkout — \
                         the main checkout has no .env yet, so these may be branch-specific or stale"
                    ));
                }
                break;
            }
        }

        if opts.run_setup {
            let setup_start = Instant::now();
            let setup_warning = run_setup(&dir)?;
            note_if_slow(&mut warnings, "setup", setup_start.elapsed());
            if let Some(warning) = setup_warning {
                warnings.push(warning);
            }
        }

        Ok(CreatedTask {
            name,
            dir,
            branch: opts.branch.clone(),
            base,
            base_label,
            ports: summary.ports,
            inherited,
            warnings,
        })
    })();

    created.inspect_err(|_| {
        let _ = git_checkout(&sr.checkout, &["worktree", "remove", "--force", &dir_s]);
        let _ = fs::remove_dir_all(Path::new(&dir_s));
        // `worktree add -b` succeeded, so the branch is ours and still points
        // at base — delete it too, or the retry dies on "branch already
        // exists" after e.g. fixing a template error.
        let _ = git_checkout(&sr.checkout, &["branch", "-D", &opts.branch]);
    })
}
