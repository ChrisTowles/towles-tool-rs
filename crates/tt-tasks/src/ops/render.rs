//! Rendering a checkout's `.env` from the repo's template — plus the
//! template sidecar and the `.git/info/exclude` maintenance that keeps the
//! task machinery's files out of `git status`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use super::claims::{
    ClaimLock, port_occupied, port_registry_path, record_task_ports, registry_claims,
};
use super::{OpsError, Result, TEMPLATE_SIDECAR, TaskRoot, base_branch, git_task, write_atomic};
use crate::{envfile, layout};

/// The template sidecar's path: `<checkout>/.claude/task-env.template`,
/// next to the repo's Claude Code settings (committable, but gitignoring it
/// works too — render only reads it).
pub fn template_sidecar_path(sr: &TaskRoot) -> PathBuf {
    sr.checkout.join(layout::CLAUDE_DIR).join(TEMPLATE_SIDECAR)
}

/// Create the [`TEMPLATE_SIDECAR`] for repos that don't commit a tokenized
/// `.env.example` (`tt task init`). Purely a starting point: a repo with no
/// template at all still renders tasks (an empty `.env` — see
/// [`render_task_env`]), so the sidecar exists to give `${tt:...}` tokens an
/// obvious home when the repo later needs one.
/// Idempotent: an existing sidecar is left untouched.
pub fn init_template_sidecar(sr: &TaskRoot) -> Result<PathBuf> {
    let sidecar = template_sidecar_path(sr);
    if sidecar.is_file() {
        return Ok(sidecar);
    }
    let claude_dir = sr.checkout.join(layout::CLAUDE_DIR);
    fs::create_dir_all(&claude_dir)
        .map_err(|e| OpsError::Io(format!("cannot create {}: {e}", claude_dir.display())))?;
    fs::write(
        &sidecar,
        "# tt task env template — this repo declares no ports/env vars for tasks.\n\
         # Add ${tt:port A-B} / ${tt:var NAME} / ${tt:task-name} / ${tt:base} tokens\n\
         # here (or commit a tokenized .env.example in the repo instead) if a task\n\
         # ever needs one.\n",
    )
    .map_err(|e| OpsError::Io(format!("cannot write {}: {e}", sidecar.display())))?;
    Ok(sidecar)
}

#[derive(Debug)]
pub struct RenderSummary {
    pub ports: Vec<(String, u16)>,
    pub reused: usize,
    pub claimed: usize,
    pub preserved: usize,
    pub warnings: Vec<String>,
}

/// Render a checkout's `.env`: template → text (reusing existing claims),
/// then merge back any keys the template doesn't know (inherited secrets,
/// local adds). Works for tasks and for the checkout itself — the checkout is
/// where the user runs the app, so it claims ports like any task. Task dirs
/// also get the `.tt-task` marker.
///
/// `new_task_base` seeds the marker's `base=` field the *first* time a task
/// is rendered (at creation, when `dir` has no marker yet) — it should be the
/// actual ref the worktree was created from ([`create_task`]'s resolved
/// `base`), not the checkout's current branch. A re-render of an *existing*
/// task (`tt task env <name>`) ignores this and keeps the marker's already
/// recorded base: it's fixed at creation and must never drift just because
/// the checkout's branch or default has since changed.
///
/// `now_ms` (epoch ms) stamps the registry's `claimed_at_ms` — passed in at
/// the CLI/app boundary, never read from the clock here.
pub fn render_task_env(
    sr: &TaskRoot,
    dir: &Path,
    new_task_base: Option<&str>,
    now_ms: i64,
) -> Result<RenderSummary> {
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| OpsError::Io(format!("bad task path {}", dir.display())))?
        .to_string();
    let is_task = dir.parent().is_some_and(|p| p == sr.tasks_dir());

    // template: the repo's own .env.example when it carries ${tt:...} tokens
    // (the committed convention), else the .claude/ sidecar (repos that
    // don't commit tt tokens in their .env.example), else empty — a repo
    // that declares nothing to template (no ports, no per-task config) still
    // renders (an empty .env), so any plain checkout is task-capable with no
    // onboarding step.
    let repo_template = dir.join(".env.example");
    let sidecar = template_sidecar_path(sr);
    let (template_path, template) = match fs::read_to_string(&repo_template) {
        Ok(text) if text.contains("${tt:") => (repo_template, text),
        _ if sidecar.is_file() => {
            let text = fs::read_to_string(&sidecar)
                .map_err(|e| OpsError::Io(format!("cannot read {}: {e}", sidecar.display())))?;
            (sidecar, text)
        }
        _ => (PathBuf::new(), String::new()),
    };

    let _lock = ClaimLock::acquire(&sr.checkout)?;

    // Resolved once; a failed resolution (no home dir) degrades to the live
    // `.env` scan alone rather than failing the render — surfaced as a
    // warning where the registry would have been written below.
    let registry_path = port_registry_path(&sr.checkout);

    let env_path = dir.join(".env");
    let old_text = fs::read_to_string(&env_path).unwrap_or_default();
    let existing: BTreeMap<String, String> = envfile::parse(&old_text).into_iter().collect();

    let mut sibling_claims = BTreeSet::new();
    for sib_dir in sr.checkouts() {
        if sib_dir == dir {
            continue;
        }
        if let Ok(text) = fs::read_to_string(sib_dir.join(".env")) {
            sibling_claims.extend(envfile::port_claims(&text));
        }
    }
    // The persistent ledger backstops the live scan above: a task whose
    // `.env` is gone, unreadable, or hand-edited still keeps its claimed
    // ports off the table until it's actually removed.
    if let Ok(path) = &registry_path {
        sibling_claims.extend(registry_claims(sr, path, &name));
    }

    // A marker already on disk (re-rendering an existing task) wins over
    // `new_task_base` — the base is set once at creation, not re-derived on
    // every `tt task env`. Only a fresh task (no marker yet) or the checkout
    // (never gets a marker) falls back to `new_task_base`/the checkout's branch.
    let recorded_base = layout::read_task_base(dir);
    let ctx_base = recorded_base
        .clone()
        .or_else(|| new_task_base.map(str::to_string))
        .unwrap_or_else(|| base_branch(&sr.checkout));
    let ctx = crate::TaskContext { task_name: &name, base_branch: &ctx_base };
    let outcome = crate::render(&template, &ctx, &existing, &sibling_claims, |p| !port_occupied(p))
        .map_err(|source| OpsError::Template {
            // an empty (no-template) render can't fail, so this always names
            // a real file
            path: template_path.display().to_string(),
            source,
        })?;

    let (merged, preserved) = envfile::merge_missing_keys(&outcome.text, &old_text);
    write_atomic(&env_path, &merged)
        .map_err(|e| OpsError::Io(format!("cannot write {}: {e}", env_path.display())))?;

    if is_task {
        let marker = layout::marker_contents(&name, &ctx_base, "main");
        fs::write(dir.join(layout::MARKER_FILE), marker)
            .map_err(|e| OpsError::Io(format!("cannot write {}: {e}", layout::MARKER_FILE)))?;
    }

    ensure_excludes(&sr.checkout)?;

    let mut warnings = Vec::new();

    // Best-effort like the rest of the registry: it's a backstop for the
    // live `.env` scan, so a failed write degrades protection, it doesn't
    // invalidate the `.env` this render just successfully wrote.
    let claimed_now: Vec<(String, u16)> =
        outcome.reused.iter().chain(outcome.claimed.iter()).cloned().collect();
    let recorded = registry_path.as_ref().map_err(|e| e.to_string()).and_then(|path| {
        record_task_ports(sr, path, &name, &claimed_now, now_ms).map_err(|e| e.to_string())
    });
    if let Err(e) = recorded {
        warnings.push(format!("could not update the port registry: {e}"));
    }

    // A manual `.env.local` pin bypasses the claim system entirely — warn
    // when it collides with a port some sibling checkout has claimed, since
    // nothing else ever surfaces that (the pin silently wins at dev-server
    // launch and the two servers fight over the port at runtime).
    if let Ok(local) = fs::read_to_string(dir.join(".env.local")) {
        for (var, port) in envfile::port_claims_by_key(&local) {
            if sibling_claims.contains(&port) {
                warnings.push(format!(
                    ".env.local pins {var}={port}, but a sibling checkout has claimed that port \
                     — the pin wins at launch and the two servers will collide"
                ));
            }
        }
    }
    if let Ok(out) = git_task(dir, &["check-ignore", "-q", ".env"])
        && !out.ok()
    {
        warnings.push(".env is NOT gitignored in this repo — it will dirty the task's tree".into());
    }

    let ports = outcome.reused.iter().chain(outcome.claimed.iter()).cloned().collect();
    Ok(RenderSummary {
        ports,
        reused: outcome.reused.len(),
        claimed: outcome.claimed.len(),
        preserved,
        warnings,
    })
}

/// Ignore the marker and the nested worktrees dir via the main checkout's
/// `.git/info/exclude` — no repo `.gitignore` commit needed. The worktrees
/// entry keeps `git status` at the checkout root clean even in repos that
/// never added `.claude/worktrees/` to their `.gitignore`.
pub(crate) fn ensure_excludes(checkout: &Path) -> Result<()> {
    let info = checkout.join(".git").join("info");
    let exclude = info.join("exclude");
    let current = fs::read_to_string(&exclude).unwrap_or_default();
    let worktrees_entry = format!("{}/{}/", layout::CLAUDE_DIR, layout::WORKTREES_DIR);
    let missing: Vec<&str> = [layout::MARKER_FILE, worktrees_entry.as_str()]
        .into_iter()
        .filter(|entry| !current.lines().any(|l| l.trim() == *entry))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    fs::create_dir_all(&info)
        .map_err(|e| OpsError::Io(format!("cannot create {}: {e}", info.display())))?;
    let mut next = current;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    for entry in missing {
        next.push_str(entry);
        next.push('\n');
    }
    fs::write(&exclude, next)
        .map_err(|e| OpsError::Io(format!("cannot write {}: {e}", exclude.display())))
}
