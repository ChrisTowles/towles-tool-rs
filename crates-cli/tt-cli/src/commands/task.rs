//! `tt task` — worktree-task lifecycle over any git checkout.
//!
//! Thin CLI shell: creation/rendering/removal all live in `tt_tasks::ops`
//! (shared with the app's `task_create`/`task_delete` commands). See the
//! tt-tasks crate docs for the convention and the `${tt:...}` template
//! grammar. `hook-create`/`hook-remove` are the Claude Code
//! WorktreeCreate/WorktreeRemove hook shells — stdin is the hook JSON and
//! (for create) stdout is *only* the worktree path, per the hook contract.

use std::fs;
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};

use tt_agentboard::task_removal;
use tt_tasks::envfile;
use tt_tasks::ops::{self, CleanOpts, CreateOpts, OpsError, RemoveOpts, TaskRoot};

use crate::cli::TaskCommands;
use crate::ui;

pub fn run(command: TaskCommands) -> i32 {
    let result = match command {
        TaskCommands::New { title, repo, status, notes, branch, base, json } => cmd_new(
            &title,
            &repo,
            &status,
            notes.as_deref(),
            branch.as_deref(),
            base.as_deref(),
            json,
        ),
        TaskCommands::Ls { json, root } => cmd_ls(json, root.as_deref()),
        TaskCommands::Rm { name, force, root } => cmd_rm(&name, force, root.as_deref()),
        TaskCommands::Init { root } => cmd_init(root.as_deref()),
        TaskCommands::Env { name, root } => cmd_env(&name, root.as_deref()),
        TaskCommands::Clean { dry_run, json, root } => cmd_clean(dry_run, json, root.as_deref()),
        TaskCommands::HookCreate => cmd_hook_create(),
        TaskCommands::HookRemove => cmd_hook_remove(),
    };
    match result {
        Ok(()) => 0,
        Err(message) => {
            ui::error(&message);
            1
        }
    }
}

/// The hook JSON Claude Code writes to the hook's stdin. TTY-guarded so a
/// hand-run `tt task hook-create` fails fast instead of hanging on a read.
fn read_hook_input() -> Result<serde_json::Value, String> {
    let mut stdin = std::io::stdin();
    if stdin.is_terminal() {
        return Err("hook-create/hook-remove read Claude Code's hook JSON on stdin — \
                    they are not meant to be run by hand"
            .to_string());
    }
    let mut raw = String::new();
    stdin.read_to_string(&mut raw).map_err(|e| format!("cannot read hook stdin: {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("hook stdin is not valid JSON: {e}"))
}

fn hook_str<'a>(input: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| input.get(k).and_then(|v| v.as_str())).filter(|s| !s.is_empty())
}

/// WorktreeCreate hook: create (or reuse) the task for the requested name and
/// print its path — the one line of stdout Claude Code parses. The requested
/// name IS the branch, verbatim (`claude -w feat/thing` → branch
/// `feat/thing`, task folder `feat-thing` — the folder is a one-way slug of
/// the branch, never parsed back) — and never the native `worktree-<name>`
/// scheme or a guessed prefix. Claude Code observed (2.1.210) sends
/// `{session_id, transcript_path, cwd, hook_event_name, name}` with `cwd`
/// already the main checkout root; `worktree_name`/`source_ref` are accepted
/// too for the documented shape.
fn cmd_hook_create() -> Result<(), String> {
    let input = read_hook_input()?;
    let name = hook_str(&input, &["name", "worktree_name"])
        .ok_or("hook input has no worktree name (`name`/`worktree_name`)")?;
    let root = hook_str(&input, &["cwd"]).map(PathBuf::from);
    let branch = name.to_string();

    let opts = CreateOpts {
        root: root.clone(),
        branch: branch.clone(),
        base: hook_str(&input, &["source_ref"]).map(str::to_string),
        run_setup: true,
    };
    let dir = match ops::create_task(&opts) {
        Ok(created) => {
            for warning in &created.warnings {
                eprintln!("tt task: {warning}");
            }
            created.dir
        }
        // A task whose folder already exists is a resume ONLY if it's still
        // on the requested branch — Claude Code re-enters worktrees by name.
        // Distinct branches can slug to the same folder (`feat/thing` and a
        // literal `feat-thing` both land on `feat-thing`), so this must be
        // checked, not assumed: silently handing back an unrelated branch's
        // worktree would be worse than failing loudly.
        Err(OpsError::TaskExists { dir, .. }) => {
            let existing_branch = ops::git_task(Path::new(&dir), &["branch", "--show-current"])
                .ok()
                .filter(|out| out.ok())
                .map(|out| out.stdout.trim().to_string());
            if existing_branch.as_deref() != Some(branch.as_str()) {
                return Err(format!(
                    "task folder {dir} already exists on branch {:?}, not the requested {branch:?} \
                     — its name collides with a different branch's slug",
                    existing_branch.unwrap_or_else(|| "<unreadable>".to_string())
                ));
            }
            PathBuf::from(dir)
        }
        Err(e) => return Err(e.to_string()),
    };
    println!("{}", dir.display());
    Ok(())
}

/// WorktreeRemove hook: the same guarded removal as `tt task rm` (never
/// forced — a task with unpushed work stays on disk and the refusal lands in
/// Claude Code's hook log on stderr), plus the agentboard untracking every
/// removal path owes.
fn cmd_hook_remove() -> Result<(), String> {
    let input = read_hook_input()?;
    let path = hook_str(&input, &["worktree_path", "path"])
        .map(PathBuf::from)
        .ok_or("hook input has no worktree path (`worktree_path`/`path`)")?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("bad worktree path {}", path.display()))?
        .to_string();
    if !path.exists() {
        eprintln!("tt task: {} is already gone — nothing to remove", path.display());
        return Ok(());
    }
    let checkout = ops::discover_root(Some(&path)).map_err(|e| e.to_string())?.checkout;
    let opts = RemoveOpts { root: Some(path.clone()), name, force: false };
    match remove_task_fully(&opts, &path, &checkout)? {
        task_removal::Outcome::Removed { messages, .. } => {
            for message in messages {
                eprintln!("tt task: {message}");
            }
            Ok(())
        }
        task_removal::Outcome::Blocked { name, blocked, messages } => {
            Err(refusal(&name, &blocked, &messages))
        }
    }
}

/// Remove a worktree task and everything bound to it, through the one shared
/// sequence in [`tt_agentboard::task_removal`]. The CLI has no panes and no
/// in-memory rail, so it passes no hooks; the app passes its own and gets the
/// identical ordering.
fn remove_task_fully(
    opts: &RemoveOpts,
    dir: &Path,
    checkout: &Path,
) -> Result<task_removal::Outcome, String> {
    let store = board_store_for(checkout);
    let removal = task_removal::TaskRemoval {
        opts,
        dir,
        repos_path: &tt_agentboard::repos::default_repos_path(),
        rows: store.as_ref().map(|s| s as &dyn task_removal::BoardRows),
        // The name came from the command line, so a missing worktree is a typo
        // to report, not a no-op to celebrate.
        on_missing: task_removal::MissingDir::Fail,
    };
    task_removal::remove_task_and_bindings(removal, &mut task_removal::NoHooks)
        .map_err(|e| e.to_string())
}

/// Open the board store that owns a checkout's rows.
///
/// The scope comes from the *checkout*, not the ambient cwd: the row was
/// written by the app running from there, so a `tt task rm` invoked from inside
/// any worktree would otherwise open a different, empty database, delete
/// nothing, and leave the orphaned row this path exists to prevent.
///
/// `None` on a store that won't open — best-effort by design, since the
/// worktree removal has already happened by the time this matters and failing
/// the command now would report a removal that really occurred as a failure.
fn board_store_for(checkout: &Path) -> Option<tt_store::Store> {
    let scope = tt_config::task_scope_from_dir(checkout);
    let path = tt_config::store_db_path_for_scope(scope.as_deref()).ok()?;
    tt_store::Store::open(&path).ok()
}

/// Steps 4 and 5 of the removal sequence, for `tt task clean` — which removes
/// in bulk through `ops::clean_tasks` and so has already taken each worktree
/// off disk by the time the bindings need clearing. Same shared code as every
/// other path.
///
/// **Returns** its notes rather than printing them: `tt task clean --json`
/// writes a machine-read document to stdout and `ui::warning` (like
/// `ui::success`) prints to *stdout*, so a helper that reported for itself
/// would corrupt that document. Each caller renders in its own idiom; `--json`
/// drops them.
fn after_removal(checkout: &Path, dir: &Path) -> Vec<String> {
    let store = board_store_for(checkout);
    task_removal::remove_bindings(
        &tt_agentboard::repos::default_repos_path(),
        store.as_ref().map(|s| s as &dyn task_removal::BoardRows),
        dir,
    )
}

/// Create a task (the unit of work): a board-task row PLUS its worktree, in one
/// shot. Params mirror the MCP `task_create` tool (title/repo/status/notes) and
/// add the worktree's branch/base, defaulted so `tt task new "Fix login" --repo
/// myrepo` is enough. The board row is the same store path the app's `+` flow and
/// MCP `task_create` write; the worktree is the same `ops::create_task` the app's
/// `task_create` command uses — this is those two flows unified behind one verb.
fn cmd_new(
    title: &str,
    repo: &str,
    status: &str,
    notes: Option<&str>,
    branch: Option<&str>,
    base: Option<&str>,
    json: bool,
) -> Result<(), String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("a task needs a title".to_string());
    }

    // Resolve the tracked repo (by name or dir), exactly as MCP `task_create`
    // does — the worktree is created inside this checkout, and the board row
    // lands in its swimlane.
    let repos = tt_agentboard::repos::load_repos(&tt_agentboard::repos::default_repos_path());
    let entries = tt_agentboard::repos::repo_entries(&repos);
    let repo_dir = match entries.iter().find(|e| e.dir == repo || e.name == repo) {
        Some(e) => e.dir.clone(),
        // A repo not on the rail is still usable when it names a real checkout on
        // disk — the worktree is created there and the board row records its path.
        None if Path::new(repo).is_dir() => repo.to_string(),
        None => {
            return Err(format!("unknown repo {repo:?} — track it on the Agentboard rail first"));
        }
    };

    // The branch defaults to a slug of the title; the task folder slugs it again.
    let branch = match branch.map(str::trim).filter(|b| !b.is_empty()) {
        Some(b) => b.to_string(),
        None => {
            let slug = tt_git::branch_name::slug(title);
            if slug.is_empty() {
                return Err("cannot derive a branch from the title — pass --branch".to_string());
            }
            slug
        }
    };

    // `--repo` may name any dir inside the checkout (including one of its own
    // worktrees); `ops::create_task` anchors to the main checkout, so resolve it
    // here too and bind the board row to *that* — a nested path recorded as
    // `worktree_repo_root` would key the card to a Board swimlane matching no repo.
    let sr = ops::discover_root(Some(Path::new(&repo_dir))).map_err(|e| e.to_string())?;
    let repo_root = sr.checkout.to_string_lossy().to_string();

    let opts = CreateOpts {
        root: Some(sr.checkout.clone()),
        branch,
        base: base.map(str::to_string),
        run_setup: true,
    };
    let created = ops::create_task(&opts).map_err(|e| e.to_string())?;
    for warning in &created.warnings {
        ui::warning(warning);
    }
    let dir_s = created.dir.to_string_lossy().to_string();

    // Record the board task and bind it to the repo + the new worktree. A store
    // that can't open (or a rejected status) is a soft failure: the worktree
    // exists and is usable, so warn rather than abort.
    let task_id = match record_board_task(title, status, notes, &repo_root, &created.branch, &dir_s)
    {
        Ok(id) => Some(id),
        Err(e) => {
            ui::warning(&format!("worktree created, but the board task was not recorded: {e}"));
            None
        }
    };

    if json {
        let ports: serde_json::Map<String, serde_json::Value> =
            created.ports.iter().map(|(k, p)| (k.clone(), (*p).into())).collect();
        let value = serde_json::json!({
            "taskId": task_id,
            "title": title,
            "status": status,
            "repo": repo_root,
            "name": created.name,
            "dir": dir_s,
            "branch": created.branch,
            "base": created.base,
            "baseLabel": created.base_label,
            "ports": ports,
            "inheritedKeys": created.inherited,
        });
        println!("{}", serde_json::to_string_pretty(&value).unwrap_or_default());
    } else {
        match task_id {
            Some(id) => ui::success(&format!(
                "created task #{id} \"{title}\" ({status}) on branch {}",
                created.branch
            )),
            None => ui::success(&format!("created worktree on branch {}", created.branch)),
        }
        for (key, port) in &created.ports {
            println!("  {key}={port}");
        }
        if created.inherited > 0 {
            println!("  inherited {} key(s) from a sibling checkout", created.inherited);
        }
        println!("task: {dir_s}");
    }
    Ok(())
}

/// Write the #339 board-task row and bind it to `repo_root` + the new worktree
/// (`branch`/`dir`). Same store path as the app's `store_add_task` +
/// `store_task_set_worktree` and MCP `task_create`. Returns the new task id.
fn record_board_task(
    title: &str,
    status: &str,
    notes: Option<&str>,
    repo_root: &str,
    branch: &str,
    dir: &str,
) -> Result<i64, String> {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let store = tt_store::Store::open_default().map_err(|e| e.to_string())?;
    let task = store.add_task(title, status, notes, now_ms).map_err(|e| e.to_string())?;
    store
        .set_task_worktree(task.id, repo_root, None, Some(branch), Some(dir))
        .map_err(|e| e.to_string())?;
    Ok(task.id)
}

/// Resolve `name` to a checkout dir: `primary` or a dir under `.claude/worktrees/`.
fn checkout_dir(sr: &TaskRoot, name: &str) -> Result<std::path::PathBuf, String> {
    if name == "primary" || name == sr.checkout.file_name().and_then(|n| n.to_str()).unwrap_or("") {
        return Ok(sr.checkout.clone());
    }
    let dir = sr.task_dir(name);
    if dir.is_dir() {
        Ok(dir)
    } else {
        Err(format!("no task {name} in {}", sr.tasks_dir().display()))
    }
}

fn cmd_init(root: Option<&Path>) -> Result<(), String> {
    let sr = ops::discover_root(root).map_err(|e| e.to_string())?;
    let report = ops::init_repo(&sr).map_err(|e| e.to_string())?;

    println!("template: {}", report.template.display());
    if report.sidecar_created {
        println!(
            "  created (empty) — add ${{tt:port A-B}} tokens there, or commit a tokenized \
             .env.example instead"
        );
    }
    if report.gitignore_added {
        println!("gitignore: added .env");
    }
    if report.hooks_wired {
        println!(
            "hooks: wired WorktreeCreate/WorktreeRemove into {}",
            report.settings_path.display()
        );
        println!("  commit it — hooks run from the committed copy, new worktrees only");
    } else {
        println!("hooks: already wired in {}", report.settings_path.display());
    }
    for warning in &report.render.warnings {
        ui::warning(warning);
    }
    for (key, port) in &report.render.ports {
        println!("  {key}={port}");
    }
    ui::success(&format!(
        "rendered primary/.env ({} reused, {} fresh claim(s))",
        report.render.reused, report.render.claimed
    ));
    Ok(())
}

fn cmd_env(name: &str, root: Option<&Path>) -> Result<(), String> {
    let sr = ops::discover_root(root).map_err(|e| e.to_string())?;
    let dir = checkout_dir(&sr, name)?;
    let summary = ops::render_task_env(&sr, &dir, None).map_err(|e| e.to_string())?;
    for warning in &summary.warnings {
        ui::warning(warning);
    }
    ui::success(&format!(
        "rendered {name}/.env ({} reused, {} fresh claim(s), {} extra key(s) preserved)",
        summary.reused, summary.claimed, summary.preserved
    ));
    Ok(())
}

fn cmd_ls(json: bool, root: Option<&Path>) -> Result<(), String> {
    let sr = ops::discover_root(root).map_err(|e| e.to_string())?;
    let _ = ops::git_checkout(&sr.checkout, &["worktree", "prune"]);
    let mut checkouts: Vec<(String, std::path::PathBuf, bool)> =
        vec![("primary".to_string(), sr.checkout.clone(), true)];
    checkouts.extend(sr.tasks().into_iter().map(|(name, dir)| (name, dir, false)));

    let refs = ops::base_refs(&sr.checkout);

    struct Row {
        name: String,
        branch: String,
        detached: bool,
        broken: bool,
        work: tt_tasks::landed::WorkState,
        /// Rendered STATE cell — each arm below knows which vocabulary applies
        /// to it, so nothing downstream has to re-derive that.
        state: String,
        ports: Vec<(String, String)>,
        primary: bool,
    }

    let mut rows = Vec::new();
    for (name, dir, is_primary) in checkouts {
        let broken = !ops::git_task(&dir, &["rev-parse", "--is-inside-work-tree"])
            .map(|o| o.ok())
            .unwrap_or(false);
        let (branch, detached, work, state) = if broken {
            ("BROKEN".to_string(), false, Default::default(), "broken".to_string())
        } else {
            let current = ops::git_task(&dir, &["branch", "--show-current"])
                .map(|o| o.stdout.trim().to_string())
                .unwrap_or_default();
            let uncommitted = ops::uncommitted_count(&dir);
            if current.is_empty() {
                // A detached HEAD has no branch to compare against a base, so
                // the landed axes are unanswerable — but the orphan count is
                // base-independent and is exactly the work removal destroys,
                // so it is measured rather than left at a default 0 that would
                // report `holdsWork: false` for a task holding unreachable
                // commits.
                let sha = ops::git_task(&dir, &["rev-parse", "--short", "HEAD"])
                    .map(|o| o.stdout.trim().to_string())
                    .unwrap_or_else(|_| "?".to_string());
                let work = tt_tasks::landed::WorkState {
                    uncommitted,
                    orphaned: ops::orphaned_count(&dir),
                    ..Default::default()
                };
                let state = work.headline();
                (format!("detached:{sha}"), true, work, state)
            } else if current == refs.base {
                // A checkout sitting on the base branch has no line of work to
                // judge; running it through the landed vocabulary would label
                // the main checkout "no commits".
                let work = tt_tasks::landed::WorkState { uncommitted, ..Default::default() };
                let state = match uncommitted {
                    0 => "clean".to_string(),
                    n => format!("{n} uncommitted"),
                };
                (current, false, work, state)
            } else {
                let work = ops::work_state(
                    &refs,
                    &dir,
                    &format!("refs/heads/{current}"),
                    uncommitted,
                    ops::orphaned_count(&dir),
                );
                let state = work.headline();
                (current, false, work, state)
            }
        };
        let env_text = fs::read_to_string(dir.join(".env")).unwrap_or_default();
        let ports: Vec<(String, String)> = envfile::parse(&env_text)
            .into_iter()
            .filter(|(k, v)| {
                k.ends_with("PORT") && v.bytes().all(|b| b.is_ascii_digit()) && !v.is_empty()
            })
            .collect();
        rows.push(Row { name, branch, detached, broken, work, state, ports, primary: is_primary });
    }

    if json {
        let items: Vec<serde_json::Value> = rows
            .iter()
            .map(|r| {
                let port_map: serde_json::Map<String, serde_json::Value> =
                    r.ports.iter().map(|(k, v)| (k.clone(), v.clone().into())).collect();
                serde_json::json!({
                    "name": r.name,
                    "branch": r.branch,
                    "detached": r.detached,
                    "broken": r.broken,
                    // The two axes, separately: work that exists only here and
                    // dies with the task, vs commits the base has never seen.
                    "uncommitted": r.work.uncommitted,
                    "unlanded": r.work.unlanded,
                    "orphaned": r.work.orphaned,
                    "landed": r.work.landed.map(|v| v.label()),
                    "holdsWork": r.work.holds_work(),
                    "state": r.state,
                    "ports": port_map,
                    "primary": r.primary,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items).unwrap_or_default());
    } else {
        // Task names are branch slugs and run long, so the columns are sized to
        // the actual rows — a fixed width silently shifted every later column
        // out of alignment as soon as one name overflowed it.
        const HEADERS: [&str; 4] = ["CHECKOUT", "BRANCH", "STATE", "PORTS"];
        let cells: Vec<[String; 4]> = rows
            .iter()
            .map(|r| {
                let ports: Vec<String> = r.ports.iter().map(|(k, v)| format!("{k}={v}")).collect();
                [
                    r.name.clone(),
                    r.branch.clone(),
                    r.state.clone(),
                    ports.join(" "),
                ]
            })
            .collect();
        // Width counts chars, not bytes, so a multi-byte branch name doesn't
        // over-pad its column.
        let width = |i: usize| {
            cells
                .iter()
                .map(|c| c[i].chars().count())
                .chain([HEADERS[i].chars().count()])
                .max()
                .unwrap_or(0)
        };
        let (wn, wb, ws) = (width(0), width(1), width(2));
        println!("{:<wn$}  {:<wb$}  {:<ws$}  {}", HEADERS[0], HEADERS[1], HEADERS[2], HEADERS[3]);
        for c in &cells {
            println!("{:<wn$}  {:<wb$}  {:<ws$}  {}", c[0], c[1], c[2], c[3]);
        }
    }
    Ok(())
}

fn cmd_rm(name: &str, force: bool, root: Option<&Path>) -> Result<(), String> {
    let sr = ops::discover_root(root).map_err(|e| e.to_string())?;
    let dir = sr.task_dir(name);
    let opts = RemoveOpts { root: root.map(Path::to_path_buf), name: name.to_string(), force };
    match remove_task_fully(&opts, &dir, &sr.checkout)? {
        task_removal::Outcome::Removed { name, messages } => {
            for message in messages {
                ui::warning(&message);
            }
            ui::success(&format!("removed {name} (ports released with its .env)"));
            Ok(())
        }
        task_removal::Outcome::Blocked { name, blocked, messages } => {
            Err(refusal(&name, &blocked, &messages))
        }
    }
}

/// Render a guard refusal for the terminal: each reason paired with its
/// remedy, then a closing note about `--force`. The reason alone says what's
/// wrong but not what to do next, which is the difference between a dead end
/// and a decision. Sole owner of this text — `RemoveOutcome::Blocked` carries
/// typed reasons precisely so each shell can format them its own way.
/// `messages` carries caveats gathered before the verdict — chiefly a failed
/// `fetch --prune`, which means the guards judged against stale `origin/*`
/// refs. Printed above the reasons rather than dropped: a refusal that might
/// be an artifact of being offline reads exactly like a real one otherwise.
fn refusal(name: &str, blocked: &[tt_tasks::RmBlocked], messages: &[String]) -> String {
    let mut out = format!("refused to remove {name}:");
    for note in messages {
        out.push_str(&format!("\n  note: {note}"));
    }
    for reason in blocked {
        out.push_str(&format!("\n  {reason}\n    → {}", reason.remedy()));
    }
    let loses_work = blocked.iter().any(tt_tasks::RmBlocked::loses_work);
    out.push_str(&format!(
        "\n  Re-run with --force to remove anyway{}.",
        if loses_work { " — this discards the work above for good" } else { "" }
    ));
    out
}

fn cmd_clean(dry_run: bool, json: bool, root: Option<&Path>) -> Result<(), String> {
    let bases = tt_config::instance_state_bases().map_err(|e| e.to_string())?;
    let opts = CleanOpts {
        root: root.map(Path::to_path_buf),
        dry_run,
        scope_parents: bases.scope_parents().to_vec(),
    };
    let report =
        ops::clean_tasks(&opts, tt_config::task_scope_from_dir).map_err(|e| e.to_string())?;

    // Each removed task may be tracked on the agentboard rail (same rationale
    // as `tt task rm`'s untracking below) — drop its now-dangling repos.json
    // entry so collectors (`prs`/`issues`) don't keep retrying a gone dir.
    if !dry_run {
        for task in &report.removed {
            let notes = after_removal(&task.checkout, &task.dir);
            if !json {
                for note in notes {
                    ui::warning(&format!("{}: {note}", task.name));
                }
            }
        }
    }

    // Agentboard stores that survive the sweep: the unscoped daily driver's
    // plus every remaining checkout's scope. Removed scopes' stores just got
    // deleted wholesale with their state dir.
    let mut store_dirs = vec![bases.agentboard_dir(None)];
    store_dirs.extend(report.live_scopes.iter().map(|s| bases.agentboard_dir(Some(s))));
    let mut prunes = Vec::new();
    for dir in store_dirs {
        match tt_agentboard::cleanup::prune_store(&dir, dry_run) {
            Ok(Some(prune)) => prunes.push(prune),
            Ok(None) => {}
            Err(e) => {
                ui::warning(&format!("agentboard prune failed for {}: {e}", dir.display()));
            }
        }
    }

    if json {
        let value = serde_json::json!({
            "dryRun": report.dry_run,
            "removed": report.removed.iter().map(|s| serde_json::json!({
                "name": s.name,
                "branch": s.branch,
                "reason": s.reason,
                "messages": s.messages,
            })).collect::<Vec<_>>(),
            "kept": report.kept.iter().map(|s| serde_json::json!({
                "name": s.name,
                "branch": s.branch,
                "why": s.why,
            })).collect::<Vec<_>>(),
            "sweptStateDirs": report.swept_state_dirs.iter()
                .map(|p| p.display().to_string()).collect::<Vec<_>>(),
            "agentboard": prunes.iter().map(|p| serde_json::json!({
                "dir": p.dir.display().to_string(),
                "sessionFoldersDropped": p.session_folders_dropped,
                "windowsDropped": p.windows_dropped,
                "panesDropped": p.panes_dropped,
            })).collect::<Vec<_>>(),
            "warnings": report.warnings,
        });
        println!("{}", serde_json::to_string_pretty(&value).unwrap_or_default());
        return Ok(());
    }

    for warning in &report.warnings {
        ui::warning(warning);
    }
    let verb = if dry_run { "would remove" } else { "removed" };
    for task in &report.removed {
        ui::success(&format!("{verb} {} ({} — {})", task.name, task.branch, task.reason));
        for message in &task.messages {
            println!("  {message}");
        }
    }
    for task in &report.kept {
        println!("kept {} ({}): {}", task.name, task.branch, task.why.join("; "));
    }
    if !report.swept_state_dirs.is_empty() {
        let verb = if dry_run { "would sweep" } else { "swept" };
        println!("{verb} stale instance state:");
        for dir in &report.swept_state_dirs {
            println!("  {}", dir.display());
        }
    }
    for prune in &prunes {
        let verb = if dry_run { "would prune" } else { "pruned" };
        println!(
            "{verb} agentboard store {}: {} window(s), {} pane(s), {} session folder(s)",
            prune.dir.display(),
            prune.windows_dropped,
            prune.panes_dropped,
            prune.session_folders_dropped.len()
        );
    }
    if report.removed.is_empty() && report.swept_state_dirs.is_empty() && prunes.is_empty() {
        println!("nothing to clean");
    }
    Ok(())
}
