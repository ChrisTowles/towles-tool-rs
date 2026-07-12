//! `ttr slot migrate` — convert a root of full clones (`<repo>-slot-N` plus
//! an optional unnumbered primary `<repo>`) into a bare hub + worktree slots.
//!
//! This layer gathers git state and executes; every decision is delegated to
//! `tt_slots::migrate` (see its docs for the rules encoded from the manual
//! migrations). Execution order per clone: sweep its branch tips into the
//! hub → verify every tip with `cat-file` → only then delete the directory
//! and re-create it as a worktree. The donor's `.git` is *moved* to become
//! the hub, so its stashes and reflogs survive wholesale.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use tt_slots::migrate::{
    self, Checkout, CloneHead, CloneInfo, CloneKind, ConfigScope, MigrationLayout, SweepAction,
};
use tt_slots::{guards, layout};

use super::{
    SlotRoot, dir_names, ensure_hub_excludes, git_hub, git_slot, render_slot_env,
    slot_env_template_available,
};
use crate::ui;

/// Slot-local config files carried from each clone into its new worktree —
/// they are gitignored, so the dirty-tree patch cannot cover them.
const ENV_FILES: &[&str] = &[".env", ".env.local"];

/// In-flight git operations that block migration, by their `.git` marker.
const OP_MARKERS: &[(&str, &str)] = &[
    ("rebase-merge", "rebase"),
    ("rebase-apply", "rebase"),
    ("MERGE_HEAD", "merge"),
    ("CHERRY_PICK_HEAD", "cherry-pick"),
    ("REVERT_HEAD", "revert"),
    ("BISECT_LOG", "bisect"),
];

pub fn cmd_migrate(repo: Option<&str>, dry_run: bool, root: Option<&Path>) -> Result<(), String> {
    let (root, found) = discover_migrate_root(root, repo)?;

    // Primary last: numbered slots keep their names; the primary is renamed
    // to the next free number at its turn.
    let mut names = found.slots.clone();
    names.extend(found.primary.clone());
    let clones: Vec<CloneInfo> = names.iter().map(|n| gather_clone(&root, n)).collect();

    let donor = migrate::choose_donor(&found, &clones).map(|c| c.name.clone());
    let blocked = migrate::check_migration(&clones, donor.as_deref());
    if !blocked.is_empty() {
        let reasons: Vec<String> = blocked.iter().map(ToString::to_string).collect();
        return Err(format!("refusing to migrate {}:\n  {}", root.display(), reasons.join("\n  ")));
    }

    let full: Vec<&CloneInfo> = clones.iter().filter(|c| c.kind == CloneKind::FullClone).collect();
    if full.is_empty() {
        ui::success(&format!("nothing to migrate — no full clones under {}", root.display()));
        return Ok(());
    }

    let hub = root.join(format!("{}.git", found.repo));
    if dry_run {
        print_plan(&root, &hub, &found, &full, donor.as_deref());
        return Ok(());
    }

    // capture phase, before anything moves: dirty trees → patches, env files
    // → copies (both into the backup dir, which outlives the migration)
    let backup = root.join(migrate::backup_dir_name(&found.repo));
    fs::create_dir_all(&backup).map_err(|e| format!("cannot create {}: {e}", backup.display()))?;
    for c in &full {
        capture_clone(&root, &backup, c)?;
    }

    let mut hub_note = format!("hub already existed: {}", hub.display());
    if !found.hub_exists {
        let donor_name = donor.as_deref().ok_or("no full clone can donate its .git for the hub")?;
        hub_note = create_hub(&root, &hub, donor_name)?;
    }

    let default_branch = resolve_default_branch(&hub);
    let _ = git_hub(&hub, &["worktree", "prune"]);
    let sr = SlotRoot { root: root.clone(), hub: hub.clone(), repo: found.repo.clone() };

    // donor first (its refs are already the hub's), then the rest in order
    let ordered: Vec<&CloneInfo> = full
        .iter()
        .copied()
        .filter(|c| Some(c.name.as_str()) == donor.as_deref())
        .chain(full.iter().copied().filter(|c| Some(c.name.as_str()) != donor.as_deref()))
        .collect();

    let mut claimed_branches: BTreeSet<String> = BTreeSet::new();
    let mut parked: Vec<String> = Vec::new();
    let mut summary: Vec<String> = Vec::new();
    let mut rendered_any = false;
    for c in ordered {
        let is_donor = Some(c.name.as_str()) == donor.as_deref();
        if !is_donor {
            sweep_clone(&sr, c, &mut parked)?;
        }
        // every branch tip, HEAD, and stash tip must be in the hub before the
        // clone's directory may be deleted
        for sha in migrate::shas_to_verify(c) {
            let probe = git_hub(&sr.hub, &["cat-file", "-e", &format!("{sha}^{{commit}}")])?;
            if !probe.ok() {
                return Err(format!(
                    "verification failed: {sha} (from {}) is not in the hub — nothing was \
                     deleted; temporary refs are kept under refs/migrate-tmp/ for inspection",
                    c.name
                ));
            }
        }
        if !is_donor {
            cleanup_tmp_refs(&sr, &c.name);
        }
        let target = if Some(&c.name) == found.primary.as_ref() {
            let n = layout::next_slot_number(&found.repo, &dir_names(&sr.root));
            layout::slot_dir_name(&found.repo, n)
        } else {
            c.name.clone()
        };
        let (line, rendered) =
            convert_clone(&sr, c, &target, &default_branch, &backup, &mut claimed_branches)?;
        rendered_any |= rendered;
        summary.push(line);
    }
    ensure_hub_excludes(&sr.hub)?;

    ui::success(&hub_note);
    for line in &summary {
        println!("  {line}");
    }
    if !parked.is_empty() {
        println!("parked refs (old tips preserved):");
        for r in &parked {
            println!("  {r}");
        }
    }
    println!(
        "backup kept at {} (patches + env copies) — delete it once satisfied",
        backup.display()
    );
    if !rendered_any {
        ui::info("render per-slot ports with `ttr slot env <name>` once a template is in place");
    }
    Ok(())
}

/// Resolve the migration root: `--root` as given, else walk up from cwd until
/// a directory's contents discover a layout.
fn discover_migrate_root(
    explicit: Option<&Path>,
    repo: Option<&str>,
) -> Result<(PathBuf, MigrationLayout), String> {
    if let Some(dir) = explicit {
        let found = migrate::discover_migration(&dir_names(dir), repo)
            .map_err(|e| format!("{}: {e}", dir.display()))?;
        return Ok((dir.to_path_buf(), found));
    }
    let start = std::env::current_dir().map_err(|e| format!("cannot read cwd: {e}"))?;
    for dir in start.ancestors() {
        if let Ok(found) = migrate::discover_migration(&dir_names(dir), repo) {
            return Ok((dir.to_path_buf(), found));
        }
    }
    Err(format!(
        "no clones to migrate found walking up from {} — pass --root (and --repo when the \
         directories are not named <repo>-slot-N)",
        start.display()
    ))
}

/// Gather one clone's state for the planner. Only filesystem checks and git
/// reads — no mutations.
fn gather_clone(root: &Path, name: &str) -> CloneInfo {
    let dir = root.join(name);
    let dot_git = dir.join(".git");
    let kind = match fs::symlink_metadata(&dot_git) {
        Ok(m) if m.file_type().is_dir() => CloneKind::FullClone,
        Ok(m) if m.file_type().is_file() => CloneKind::Worktree,
        _ => CloneKind::NotGit,
    };
    let mut info = CloneInfo {
        name: name.to_string(),
        kind,
        head: CloneHead::Unborn,
        branches: Vec::new(),
        dirty: false,
        stash: None,
        has_linked_worktrees: false,
        op_in_progress: None,
    };
    if kind != CloneKind::FullClone {
        return info;
    }

    let read = |args: &[&str]| {
        git_slot(&dir, args).ok().filter(|o| o.ok()).map(|o| o.stdout.trim().to_string())
    };
    let sha = read(&["rev-parse", "HEAD"]);
    let branch = read(&["symbolic-ref", "--quiet", "--short", "HEAD"]).filter(|b| !b.is_empty());
    info.head = match (branch, sha) {
        (_, None) => CloneHead::Unborn,
        (Some(branch), Some(sha)) => CloneHead::OnBranch { branch, sha },
        (None, Some(sha)) => CloneHead::Detached { sha },
    };
    info.branches = read(&[
        "for-each-ref",
        "refs/heads",
        "--format=%(objectname) %(refname:short)",
    ])
    .map(|out| {
        out.lines()
            .filter_map(|l| l.split_once(' '))
            .map(|(sha, name)| (name.to_string(), sha.to_string()))
            .collect()
    })
    .unwrap_or_default();
    info.dirty = read(&["status", "--porcelain"])
        .map(|out| guards::dirty_entry_count(&out) > 0)
        .unwrap_or(false);
    if let Some(sha) = read(&["rev-parse", "--quiet", "--verify", "refs/stash"]) {
        let count = read(&["stash", "list"]).map(|out| out.lines().count()).unwrap_or(1);
        info.stash = Some((count.max(1), sha));
    }
    info.has_linked_worktrees = fs::read_dir(dot_git.join("worktrees"))
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false);
    info.op_in_progress = OP_MARKERS
        .iter()
        .find(|(marker, _)| dot_git.join(marker).exists())
        .map(|(_, op)| op.to_string());
    info
}

/// Save a dirty clone's tree as a patch and copy its env files into the
/// backup dir. `git add -A` stages untracked files so one `diff --cached`
/// captures everything (binary-safe); the index is reset afterwards.
fn capture_clone(root: &Path, backup: &Path, c: &CloneInfo) -> Result<(), String> {
    let dir = root.join(&c.name);
    for env in ENV_FILES {
        let src = dir.join(env);
        if src.is_file() {
            fs::copy(&src, backup.join(format!("{}{env}", c.name)))
                .map_err(|e| format!("cannot back up {}/{env}: {e}", c.name))?;
        }
    }
    if !c.dirty {
        return Ok(());
    }
    let staged = git_slot(&dir, &["add", "-A"])?;
    if !staged.ok() {
        return Err(format!("git add -A failed in {}:\n{}", c.name, staged.stderr.trim()));
    }
    let diff = git_slot(&dir, &["diff", "--binary", "--cached"])?;
    if !diff.ok() {
        return Err(format!("git diff failed in {}:\n{}", c.name, diff.stderr.trim()));
    }
    let _ = git_slot(&dir, &["reset", "-q"]);
    let path = backup.join(migrate::patch_file_name(&c.name));
    fs::write(&path, &diff.stdout).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Move the donor's `.git` to `<root>/<repo>.git` and mark it bare —
/// `core.bare` goes into `config.worktree` when `extensions.worktreeConfig`
/// is enabled (a shared `true` would break every linked worktree).
fn create_hub(root: &Path, hub: &Path, donor_name: &str) -> Result<String, String> {
    let donor_git = root.join(donor_name).join(".git");
    fs::rename(&donor_git, hub)
        .map_err(|e| format!("cannot move {} to {}: {e}", donor_git.display(), hub.display()))?;
    let wtc = git_hub(hub, &["config", "--bool", "--get", "extensions.worktreeConfig"])
        .ok()
        .filter(|o| o.ok())
        .map(|o| o.stdout.trim() == "true")
        .unwrap_or(false);
    for edit in migrate::bare_config_edits(wtc) {
        let out = match edit.scope {
            ConfigScope::Shared => git_hub(hub, &["config", edit.key, edit.value])?,
            ConfigScope::WorktreePrivate => {
                let file = hub.join(migrate::WORKTREE_CONFIG_FILE);
                let file_s = file.to_string_lossy().to_string();
                git_hub(hub, &["config", "--file", &file_s, edit.key, edit.value])?
            }
        };
        if !out.ok() {
            return Err(format!("git config {} failed:\n{}", edit.key, out.stderr.trim()));
        }
    }
    let bare = git_hub(hub, &["rev-parse", "--is-bare-repository"])?;
    if bare.stdout.trim() != "true" {
        return Err(format!(
            "{} does not report itself bare after configuration — aborting before any \
             worktree is created",
            hub.display()
        ));
    }
    Ok(format!(
        "hub created from {donor_name}'s .git: {}{}",
        hub.display(),
        if wtc { " (worktreeConfig on — core.bare placed in config.worktree)" } else { "" }
    ))
}

/// The hub's default branch, with HEAD re-pointed there so future
/// `slot new` calls detach at the right base.
fn resolve_default_branch(hub: &Path) -> String {
    let read = |args: &[&str]| {
        git_hub(hub, args).ok().filter(|o| o.ok()).map(|o| o.stdout.trim().to_string())
    };
    let origin_head = read(&["rev-parse", "--abbrev-ref", "origin/HEAD"]);
    let locals: Vec<String> = read(&["for-each-ref", "refs/heads", "--format=%(refname:short)"])
        .map(|out| out.lines().map(str::to_string).collect())
        .unwrap_or_default();
    let head_branch = read(&["symbolic-ref", "--quiet", "--short", "HEAD"]);
    let default =
        migrate::pick_default_branch(origin_head.as_deref(), &locals, head_branch.as_deref());
    if head_branch.as_deref() != Some(default.as_str()) {
        let _ = git_hub(hub, &["symbolic-ref", "HEAD", &format!("refs/heads/{default}")]);
    }
    default
}

fn update_ref(sr: &SlotRoot, name: &str, sha: &str) -> Result<(), String> {
    let full = format!("refs/heads/{name}");
    let out = git_hub(&sr.hub, &["update-ref", &full, sha])?;
    if !out.ok() {
        return Err(format!("git update-ref {full} {sha} failed:\n{}", out.stderr.trim()));
    }
    Ok(())
}

/// Fetch one clone's refs into a temp namespace, then land every branch tip
/// in the hub (create / fast-forward / park), plus the stash tip and — when
/// unreachable from any branch — the detached HEAD.
fn sweep_clone(sr: &SlotRoot, c: &CloneInfo, parked: &mut Vec<String>) -> Result<(), String> {
    let dir_s = sr.root.join(&c.name).to_string_lossy().to_string();
    let mut refspecs = vec![format!("+refs/heads/*:refs/migrate-tmp/{}/*", c.name)];
    if c.stash.is_some() {
        refspecs.push(format!("+refs/stash:refs/migrate-tmp/{}/stash", c.name));
    }
    if matches!(c.head, CloneHead::Detached { .. }) {
        refspecs.push(format!("+HEAD:refs/migrate-tmp/{}/head", c.name));
    }
    let mut args = vec!["fetch", "--quiet", "--no-tags", &dir_s];
    args.extend(refspecs.iter().map(String::as_str));
    let fetched = git_hub(&sr.hub, &args)?;
    if !fetched.ok() {
        return Err(format!("git fetch from {} failed:\n{}", c.name, fetched.stderr.trim()));
    }

    let is_ancestor = |a: &str, b: &str| {
        git_hub(&sr.hub, &["merge-base", "--is-ancestor", a, b]).map(|o| o.ok()).unwrap_or(false)
    };
    for (branch, sha) in &c.branches {
        let hub_sha = git_hub(
            &sr.hub,
            &[
                "rev-parse",
                "--quiet",
                "--verify",
                &format!("refs/heads/{branch}"),
            ],
        )
        .ok()
        .filter(|o| o.ok())
        .map(|o| o.stdout.trim().to_string());
        match migrate::sweep_action(&c.name, branch, sha, hub_sha.as_deref(), is_ancestor) {
            SweepAction::Create | SweepAction::FastForward => update_ref(sr, branch, sha)?,
            SweepAction::AlreadyPresent => {}
            SweepAction::Park { ref_name } => {
                update_ref(sr, &ref_name, sha)?;
                parked.push(ref_name);
            }
        }
    }
    if let Some((_, sha)) = &c.stash {
        let ref_name = migrate::park_ref(&c.name, "stash");
        update_ref(sr, &ref_name, sha)?;
        parked.push(ref_name);
    }
    if let CloneHead::Detached { sha } = &c.head {
        // a detached commit may be on no branch at all; park it so it stays
        // reachable even if the new worktree later moves off it
        let contained = git_hub(&sr.hub, &["branch", "--contains", sha, "--format=%(refname)"])
            .ok()
            .filter(|o| o.ok())
            .map(|o| !o.stdout.trim().is_empty())
            .unwrap_or(false);
        if !contained {
            let ref_name = migrate::park_ref(&c.name, "detached-head");
            update_ref(sr, &ref_name, sha)?;
            parked.push(ref_name);
        }
    }
    Ok(())
}

/// Drop one clone's `refs/migrate-tmp/<name>/*` namespace (best-effort — the
/// data has been verified into real refs by now).
fn cleanup_tmp_refs(sr: &SlotRoot, clone_name: &str) {
    let ns = format!("refs/migrate-tmp/{clone_name}");
    if let Ok(out) = git_hub(&sr.hub, &["for-each-ref", "--format=%(refname)", &ns]) {
        for r in out.stdout.lines().map(str::trim).filter(|r| !r.is_empty()) {
            let _ = git_hub(&sr.hub, &["update-ref", "-d", r]);
        }
    }
}

/// Delete the (verified) clone and re-create it as a worktree per the plan,
/// then restore env files, re-apply the dirty patch, and write the marker.
fn convert_clone(
    sr: &SlotRoot,
    c: &CloneInfo,
    target: &str,
    default_branch: &str,
    backup: &Path,
    claimed_branches: &mut BTreeSet<String>,
) -> Result<(String, bool), String> {
    let hub_tip = match &c.head {
        CloneHead::OnBranch { branch, .. } => git_hub(
            &sr.hub,
            &[
                "rev-parse",
                "--quiet",
                "--verify",
                &format!("refs/heads/{branch}"),
            ],
        )
        .ok()
        .filter(|o| o.ok())
        .map(|o| o.stdout.trim().to_string()),
        _ => None,
    };
    let branch_taken =
        matches!(&c.head, CloneHead::OnBranch { branch, .. } if claimed_branches.contains(branch));
    let is_ancestor = |a: &str, b: &str| {
        git_hub(&sr.hub, &["merge-base", "--is-ancestor", a, b]).map(|o| o.ok()).unwrap_or(false)
    };
    let checkout = migrate::plan_checkout(
        &c.head,
        c.dirty,
        default_branch,
        hub_tip.as_deref(),
        branch_taken,
        is_ancestor,
    );

    let old_dir = sr.root.join(&c.name);
    fs::remove_dir_all(&old_dir)
        .map_err(|e| format!("cannot remove {}: {e}", old_dir.display()))?;
    let new_dir = sr.slot_dir(target);
    let new_s = new_dir.to_string_lossy().to_string();
    let (added, mut desc) = match &checkout {
        Checkout::Branch(b) => {
            (git_hub(&sr.hub, &["worktree", "add", &new_s, b])?, format!("on branch {b}"))
        }
        Checkout::Detach { at, reason } => (
            git_hub(&sr.hub, &["worktree", "add", "--detach", &new_s, at])?,
            format!("detached at {} — {}", short(at), reason.note()),
        ),
    };
    if !added.ok() {
        return Err(format!(
            "git worktree add for {target} failed (its data is already safe in the hub and \
             {}):\n{}",
            backup.display(),
            added.stderr.trim()
        ));
    }
    if let Checkout::Branch(b) = &checkout {
        claimed_branches.insert(b.clone());
    }

    for env in ENV_FILES {
        let saved = backup.join(format!("{}{env}", c.name));
        if saved.is_file() {
            fs::copy(&saved, new_dir.join(env))
                .map_err(|e| format!("cannot restore {target}/{env}: {e}"))?;
            desc.push_str(&format!(", {env} carried"));
        }
    }
    let patch = backup.join(migrate::patch_file_name(&c.name));
    if patch.is_file() {
        let patch_s = patch.to_string_lossy().to_string();
        let applied = git_slot(&new_dir, &["apply", "--whitespace=nowarn", &patch_s])?;
        if applied.ok() {
            desc.push_str(", dirty tree re-applied");
        } else {
            ui::warning(&format!(
                "{target}: could not re-apply the dirty tree — patch kept at {patch_s}\n{}",
                applied.stderr.trim()
            ));
            desc.push_str(", dirty tree NOT re-applied (see warning)");
        }
    }
    fs::write(
        new_dir.join(layout::MARKER_FILE),
        layout::marker_contents(target, default_branch, "main"),
    )
    .map_err(|e| format!("cannot write {target}/{}: {e}", layout::MARKER_FILE))?;

    // Render `.env` through the same op `slot new`/`slot env` use, reusing any
    // claims already in the carried `.env` and avoiding sibling ports. Skipped
    // (not failed) when the repo has no template yet — the data is already safe.
    let rendered = slot_env_template_available(sr, &new_dir);
    if rendered {
        match render_slot_env(sr, &new_dir) {
            Ok(summary) => desc.push_str(&format!(
                ", .env rendered ({} port(s): {} reused, {} fresh)",
                summary.ports.len(),
                summary.reused,
                summary.claimed
            )),
            Err(e) => {
                ui::warning(&format!("{target}: .env not rendered — {e}"));
                desc.push_str(", .env NOT rendered (see warning)");
            }
        }
    }

    let line = if target == c.name {
        format!("{target}: {desc}")
    } else {
        format!("{} → {target}: {desc}", c.name)
    };
    Ok((line, rendered))
}

fn short(rev: &str) -> &str {
    if rev.len() == 40 && rev.bytes().all(|b| b.is_ascii_hexdigit()) { &rev[..12] } else { rev }
}

fn print_plan(
    root: &Path,
    hub: &Path,
    found: &MigrationLayout,
    full: &[&CloneInfo],
    donor: Option<&str>,
) {
    ui::info(&format!("dry run — nothing will be touched under {}", root.display()));
    if found.hub_exists {
        println!("hub exists: {}", hub.display());
    } else if let Some(d) = donor {
        println!("would create hub {} by moving {d}'s .git", hub.display());
    }
    for c in full {
        let head = match &c.head {
            CloneHead::OnBranch { branch, .. } => format!("on {branch}"),
            CloneHead::Detached { sha } => format!("detached at {}", short(sha)),
            CloneHead::Unborn => "unborn".to_string(),
        };
        let mut notes = vec![format!(
            "{} branch tip(s) to sweep + verify",
            c.branches.len()
        )];
        if c.dirty {
            notes.push("dirty — tree saved as a patch and re-applied".to_string());
        }
        if c.stash.is_some() {
            notes.push(if Some(c.name.as_str()) == donor {
                "stash survives with the donor's .git".to_string()
            } else {
                format!("stash parked at {}", migrate::park_ref(&c.name, "stash"))
            });
        }
        for env in ENV_FILES {
            if root.join(&c.name).join(env).is_file() {
                notes.push(format!("{env} carried"));
            }
        }
        println!("{} ({head}): {}", c.name, notes.join(", "));
    }
    if let Some(p) = &found.primary
        && full.iter().any(|c| c.name == *p)
    {
        let n = layout::next_slot_number(&found.repo, &dir_names(root));
        println!("{p} (primary) would become {}", layout::slot_dir_name(&found.repo, n));
    }
}
