//! Data-hub collectors for the towles-tool personal dashboard.
//!
//! Each collector gathers one slice of state — calendar events, cross-repo
//! issues, and pull-request status — and writes it into the shared
//! [`tt_store::Store`]. The calendar collector shells out to `claude -p` (via
//! [`tt_exec`]) once per configured [`tt_config::CalendarSource`], each source
//! writing into its own store lane; the issue and PR collectors shell out to
//! `gh`.
//!
//! Tauri-free (the shared-crate rule): both the CLI (`tt collect`) and the
//! desktop app's scheduler drive this crate against the same [`CollectSummary`]
//! contract.
//!
//! ## Robustness contract
//!
//! The public `collect_*` functions **never panic and never return `Err`**.
//! Every failure mode — a missing `claude`/`gh` binary, a non-zero exit,
//! unparseable output — is captured as a [`CollectSummary`] with `ok = false`
//! and a `message`, and is also recorded via [`tt_store::Store::record_run`]
//! under a stable collector key: `claude:calendar`, `issues`, or `prs`.

mod gh;
pub mod issues;
mod prs;
mod quiet_hours;
mod slack;
mod slack_socket;

pub use issues::fetch_importable_issues;
pub use quiet_hours::{should_run_at, should_run_calendar};
pub use slack::{
    DmFile, DmMessage, SlackDmConfig, SlackFile, SlackUser, dm_channel_id, fetch_dm_history,
    fetch_file, list_users, send_dm,
};
pub use slack_socket::{
    Backoff, Envelope, MessageEvent, ack_json, is_watched_message, open_socket_connection,
    parse_connection_url, parse_envelope,
};

use std::path::{Path, PathBuf};
use std::time::Duration;

use tt_config::CalendarSource;
use tt_store::{EventInput, Store};

/// Hard cap on a `claude -p` calendar run. Generous for MCP tool calls; without
/// it a wedged claude (auth prompt, dead MCP server) blocks its caller forever —
/// in the app that stalls every collector, since the scheduler awaits batches
/// serially.
const CLAUDE_TIMEOUT: Duration = Duration::from_secs(180);

/// The outcome of a single collector run.
#[derive(Debug, Clone, PartialEq)]
pub struct CollectSummary {
    /// Stable collector key (also the `record_run` key).
    pub collector: String,
    /// Whether the run succeeded end-to-end.
    pub ok: bool,
    /// Number of items written to the store (0 on failure).
    pub count: usize,
    /// A human-readable note: the error on failure, or context on success
    /// (e.g. `"no repos configured"`).
    pub message: Option<String>,
}

// ---------------------------------------------------------------------------
// Public collectors
// ---------------------------------------------------------------------------

/// The stable `record_run` key for the calendar collector.
const CALENDAR_KEY: &str = "claude:calendar";

/// Collect today's calendar events, running one `claude -p` per **enabled**
/// [`CalendarSource`] and writing each into its own store lane. Records
/// `claude:calendar`.
///
/// One run key covers every source deliberately: `claude:calendar` is what the
/// frontend's collector-health list, the stale-collector watch, and the store's
/// own run-pruning all match on, and the user-facing question ("is my calendar
/// data fresh?") is answered per-collector, not per-calendar. Sources are still
/// fully independent *underneath* it — each pulls and writes on its own, and a
/// failing source contributes a `<id>: <error>` note and flips `ok` without
/// costing the others their rows.
pub fn collect_calendar(store: &Store, sources: &[CalendarSource], now_ms: i64) -> CollectSummary {
    let enabled: Vec<&CalendarSource> = sources.iter().filter(|s| s.enabled).collect();
    if enabled.is_empty() {
        // Still age out stale rows. Retention otherwise only runs as a side
        // effect of a write, so turning the last calendar off would freeze
        // whatever was in the table — leaving the countdown counting down from
        // a meeting that happened weeks ago.
        let _ = store.sweep_old_events(now_ms);
        let msg = "no calendar sources enabled".to_string();
        return finish(store, CALENDAR_KEY, true, 0, Some(msg), now_ms);
    }

    let (day_start_ms, day_end_ms) = Store::local_day_bounds(now_ms);
    let mut count = 0usize;
    let mut notes: Vec<String> = Vec::new();
    let mut ok = true;
    let mut wrote = false;
    // Ids name store lanes, so a repeat isn't a harmless duplicate: the second
    // pull's scoped DELETE removes the rows the first just wrote, and `count`
    // would still claim both. Settings are hand-editable, so refuse the repeat
    // rather than silently losing a calendar.
    let mut seen_ids: Vec<&str> = Vec::new();
    for source in enabled {
        let id = source.id.trim();
        if !id.is_empty() && seen_ids.contains(&id) {
            ok = false;
            notes.push(format!("{id}: duplicate source id; only the first is pulled"));
            continue;
        }
        seen_ids.push(id);
        match collect_calendar_source(store, source, day_start_ms, day_end_ms, now_ms) {
            Ok(n) => {
                count += n;
                wrote = true;
            }
            Err(msg) => {
                ok = false;
                notes.push(format!("{}: {msg}", source.id));
            }
        }
    }

    // Retention normally rides along with a successful write. If every source
    // failed there was no write, and without this a permanently-broken calendar
    // would keep yesterday's meetings in the countdown indefinitely — the same
    // failure the no-sources branch above sweeps for.
    if !wrote {
        let _ = store.sweep_old_events(now_ms);
    }

    let message = if notes.is_empty() { None } else { Some(notes.join("; ")) };
    finish(store, CALENDAR_KEY, ok, count, message, now_ms)
}

/// Pull one calendar source and write it into its own `(source, day)` lane.
///
/// Returns the number of events stored, or a human-readable error — the caller
/// aggregates both into the single `claude:calendar` run record, so this never
/// records a run of its own.
fn collect_calendar_source(
    store: &Store,
    source: &CalendarSource,
    day_start_ms: i64,
    day_end_ms: i64,
    now_ms: i64,
) -> Result<usize, String> {
    // Trimmed, and the *trimmed* value is what reaches the store. `calendar_set`
    // trims the `source` it validates and writes, so writing the raw id here
    // would put a whitespace-padded settings entry into a second lane that the
    // MCP push path can never target — one calendar, two lanes, neither
    // sweeping the other, every meeting listed twice.
    let id = source.id.trim();
    if id.is_empty() {
        return Err("source has no id".to_string());
    }
    if source.prompt.trim().is_empty() {
        return Err("source has no prompt".to_string());
    }
    let value = run_claude(&source.prompt)?;
    let events = serde_json::from_value::<Vec<EventInput>>(value)
        .map_err(|e| format!("invalid calendar JSON: {e}"))?;
    store_calendar_events(store, id, day_start_ms, day_end_ms, &events, now_ms)
}

/// Apply one source's parsed calendar result to the store, guarding against a
/// suspicious empty sweep.
///
/// A source normally replaces its whole `(source, today)` lane, but a
/// `claude -p` run can return a syntactically-valid empty `[]` when the model
/// hedges or the calendar MCP is momentarily down. Replacing on that would wipe
/// today's events and blank the Cockpit next-meeting countdown until the next
/// tick. So when the result is empty *and* **this source** still holds events
/// later today, treat the run as suspect: keep the existing rows and report an
/// error. A genuinely empty day (no future rows for this source either) still
/// clears normally and succeeds.
///
/// The guard is scoped to `source` on both sides: one calendar returning empty
/// neither consults nor protects another calendar's rows, so a flaky work
/// calendar can't be masked by a healthy personal one (or vice versa).
fn store_calendar_events(
    store: &Store,
    source: &str,
    day_start_ms: i64,
    day_end_ms: i64,
    events: &[EventInput],
    now_ms: i64,
) -> Result<usize, String> {
    if events.is_empty() && has_future_events_today(store, source, now_ms, day_end_ms)? {
        return Err("returned no events but future events remain for today; \
                    kept existing events"
            .to_string());
    }
    store
        .replace_events_for_source(source, day_start_ms, day_end_ms, events, now_ms)
        .map_err(|e| e.to_string())
}

/// Whether `source` holds any event still upcoming today (local time).
///
/// "Today" is the local calendar day containing `now_ms`; the window is
/// `[now_ms, day_end_ms)`, so only still-to-start events count. Rows from other
/// sources are filtered out — the guard is per-lane by design.
fn has_future_events_today(
    store: &Store,
    source: &str,
    now_ms: i64,
    day_end_ms: i64,
) -> Result<bool, String> {
    store
        .events_between(now_ms, day_end_ms)
        .map(|events| events.iter().any(|e| e.source == source))
        .map_err(|e| e.to_string())
}

/// Collect open issues assigned to me across `repo_dirs` via `gh` and update the
/// stored issue set. Records `issues`. With no repo dirs this is a clean no-op.
///
/// Failure containment: rows are only replaced for repos whose `gh` calls
/// succeeded. A repo that errors (rate limit, network, auth) keeps its
/// last-known-good rows — a transient outage must not blank the dashboard —
/// and the run is recorded `ok = false` so staleness is visible. Only a fully
/// clean sweep does a full-table replace (which also purges rows of repos no
/// longer tracked).
pub fn collect_issues(store: &Store, repo_dirs: &[PathBuf], now_ms: i64) -> CollectSummary {
    let repo_dirs = dedupe_repo_dirs(repo_dirs);
    let outcome = sweep_repos(&repo_dirs, issues::collect_repo_issues);
    let write = |all: &[tt_store::IssueInput], repos: Option<&[String]>| match repos {
        None => store.replace_issues(all),
        Some(repos) => store.replace_issues_for_repos(repos, all),
    };
    let summary =
        finish_sweep(store, "issues", outcome, write, |i| (i.repo.clone(), i.number), now_ms);
    sync_task_links(store, &repo_dirs, now_ms);
    summary
}

/// The Board↔GitHub read half for tasks (#339), run after every issues/PRs
/// collect pass:
///
/// 1. copy snapshot state onto every issue/PR link row whose ref the sweep
///    just refreshed;
/// 2. targeted `gh <issue|pr> view` for still-`open` links *missing* from the
///    snapshot — absence is ambiguous (closed? merged? merely reassigned away
///    or aged out of the merged list?), so state is only ever learned from an
///    actual fetch, never inferred. A failed fetch keeps the last state;
/// 3. auto-attach collected PRs whose head branch matches a task's worktree;
/// 4. roll task statuses up ([`rollup_task_statuses`]);
/// 5. archive finished tasks older than [`tt_store::ARCHIVE_AFTER_MS`].
///
/// Never panics and never fails the collect pass — each stage logs and moves
/// on (the never-panic contract). The write half (closing/reopening linked
/// issues when a task crosses the done boundary on the Board) lives in the
/// Tauri app's `store_set_task_status` command.
fn sync_task_links(store: &Store, repo_dirs: &[PathBuf], now_ms: i64) {
    if let Err(e) = store.refresh_link_states_from_cache(now_ms) {
        log::warn!("refresh_link_states_from_cache failed: {e}");
    }
    let dir_by_repo = repo_dir_index(repo_dirs);
    match store.open_issue_refs_missing_from_cache() {
        Ok(refs) => {
            for (repo, number) in refs {
                let Some(dir) = dir_by_repo.get(&repo) else {
                    continue;
                };
                match issues::fetch_issue_state(dir, number) {
                    Ok(state) => {
                        if let Err(e) = store.set_issue_link_state(&repo, number, &state, now_ms) {
                            log::warn!("set_issue_link_state {repo}#{number} failed: {e}");
                        }
                    }
                    Err(e) => log::warn!("issue state fetch {repo}#{number} failed: {e}"),
                }
            }
        }
        Err(e) => log::warn!("open_issue_refs_missing_from_cache failed: {e}"),
    }
    match store.open_pr_refs_missing_from_cache() {
        Ok(refs) => {
            for (repo, number) in refs {
                let Some(dir) = dir_by_repo.get(&repo) else {
                    continue;
                };
                match prs::fetch_pr_state(dir, number) {
                    Ok(state) => {
                        if let Err(e) = store.set_pr_link_state(&repo, number, &state, None, now_ms)
                        {
                            log::warn!("set_pr_link_state {repo}#{number} failed: {e}");
                        }
                    }
                    Err(e) => log::warn!("pr state fetch {repo}#{number} failed: {e}"),
                }
            }
        }
        Err(e) => log::warn!("open_pr_refs_missing_from_cache failed: {e}"),
    }
    if let Err(e) = store.auto_attach_worktree_prs(now_ms) {
        log::warn!("auto_attach_worktree_prs failed: {e}");
    }
    if let Err(e) = rollup_task_statuses(store, now_ms) {
        log::warn!("rollup_task_statuses failed: {e}");
    }
    // 5. age finished tasks off the active board. Riding this path (rather
    // than a timer of its own) keeps the sweep on the same cadence that
    // creates done tasks in the first place.
    if let Err(e) = store.archive_closed_tasks(now_ms - tt_store::ARCHIVE_AFTER_MS, now_ms) {
        log::warn!("archive_closed_tasks failed: {e}");
    }
}

/// Map `owner/name` → repo dir for the swept dirs, so targeted fetches know
/// where to run `gh`. A dir whose name lookup fails is skipped (its refs keep
/// their cached state until a later pass).
fn repo_dir_index(repo_dirs: &[PathBuf]) -> std::collections::HashMap<String, PathBuf> {
    let mut index = std::collections::HashMap::new();
    for dir in repo_dirs {
        match gh::repo_name_with_owner(dir) {
            Ok(name) => {
                index.entry(name).or_insert_with(|| dir.clone());
            }
            Err(e) => log::debug!("repo name lookup failed for {}: {e}", dir.display()),
        }
    }
    index
}

/// Roll linked tasks across the done boundary from their cached link states:
/// a task with at least one link rolls to `done` once every linked issue is
/// `closed` and every linked PR is `merged` or `closed`; a `done` task with a
/// reopened ref falls back to `backlog`. Tasks with no links never auto-move,
/// and statuses that already match are left untouched — safe to run on every
/// poll without fighting manual board moves that aren't a done/not-done
/// crossing. Closed and archived tasks are skipped entirely: an explicit
/// outcome is a user decision this rollup must never overturn (a reopened ref
/// must not resurrect an abandoned task, and `set_task_status` would clear
/// its outcome as a side effect).
pub fn rollup_task_statuses(store: &Store, now_ms: i64) -> tt_store::Result<usize> {
    let mut changed = 0;
    for task in store.all_tasks()? {
        if task.issues.is_empty() && task.prs.is_empty() {
            continue;
        }
        if task.outcome.is_some() || task.archived_at.is_some() {
            continue;
        }
        let resolved = task.issues.iter().all(|l| l.state == "closed")
            && task.prs.iter().all(|l| l.state == "merged" || l.state == "closed");
        let target = match (resolved, task.status.as_str()) {
            (true, status) if status != "done" => Some("done"),
            (false, "done") => Some("backlog"),
            _ => None,
        };
        if let Some(status) = target {
            store.set_task_status(task.id, status, now_ms)?;
            changed += 1;
        }
    }
    Ok(changed)
}

/// Collect open + review-requested + recently-merged PRs across `repo_dirs`
/// via `gh` and update the stored PR set. Records `prs`. Failure containment
/// matches [`collect_issues`]: failed repos keep their last-known-good rows.
///
/// This is the "full" sweep — used for on-demand refreshes (a manual
/// `tt collect prs`, `collect_all`, and the post-mutation nudge) where paying
/// for all three `gh` calls once is worth it for full freshness. The periodic
/// scheduler tick instead splits this into [`collect_prs_open`] (fast cadence)
/// and [`collect_prs_merged`] (slow cadence) so it isn't re-fetching the
/// rarely-needed merged list on every fast tick.
pub fn collect_prs(store: &Store, repo_dirs: &[PathBuf], now_ms: i64) -> CollectSummary {
    let repo_dirs = dedupe_repo_dirs(repo_dirs);
    let outcome = sweep_repos(&repo_dirs, prs::collect_repo_prs);
    let write = |all: &[tt_store::PrInput], repos: Option<&[String]>| match repos {
        None => store.replace_prs(all),
        Some(repos) => store.replace_prs_for_repos(repos, all),
    };
    let summary =
        finish_sweep(store, "prs", outcome, write, |p| (p.repo.clone(), p.number), now_ms);
    sync_task_links(store, &repo_dirs, now_ms);
    summary
}

/// Collect just the authored + review-requested open PRs across `repo_dirs`
/// — the fast half of [`collect_prs`], meant for the scheduler's frequent
/// tick. Records `prs` (same key as [`collect_prs`]/[`collect_prs_merged`]:
/// they're cadence splits of one logical collector, not separate ones).
/// Also runs [`sync_task_links`], so task-linked PR merge detection stays on
/// this fast cadence rather than the slower merged-list one.
pub fn collect_prs_open(store: &Store, repo_dirs: &[PathBuf], now_ms: i64) -> CollectSummary {
    let repo_dirs = dedupe_repo_dirs(repo_dirs);
    let outcome = sweep_repos(&repo_dirs, prs::collect_repo_prs_open);
    let write = |all: &[tt_store::PrInput], repos: Option<&[String]>| match repos {
        None => store.replace_open_prs(all),
        Some(repos) => store.replace_open_prs_for_repos(repos, all),
    };
    let summary =
        finish_sweep(store, "prs", outcome, write, |p| (p.repo.clone(), p.number), now_ms);
    sync_task_links(store, &repo_dirs, now_ms);
    summary
}

/// Collect just the recently-merged authored PRs across `repo_dirs` — the
/// slow half of [`collect_prs`]. This list exists only to catch a
/// just-merged branch before its worktree is removed; it isn't the mechanism
/// that detects a task-linked PR merging (that's [`sync_task_links`], run
/// from [`collect_prs_open`]'s faster cadence), so this doesn't call it.
pub fn collect_prs_merged(store: &Store, repo_dirs: &[PathBuf], now_ms: i64) -> CollectSummary {
    let repo_dirs = dedupe_repo_dirs(repo_dirs);
    let outcome = sweep_repos(&repo_dirs, prs::collect_repo_merged_prs);
    let write = |all: &[tt_store::PrInput], repos: Option<&[String]>| match repos {
        None => store.replace_merged_prs(all),
        Some(repos) => store.replace_merged_prs_for_repos(repos, all),
    };
    finish_sweep(store, "prs", outcome, write, |p| (p.repo.clone(), p.number), now_ms)
}

/// Collect the watched Slack DM's latest state via the Slack Web API and
/// upsert it into the store. Records `slack:dm`. Missing token/user-id is a
/// recorded failure (the caller gates on `enabled`, so reaching here without
/// credentials is a misconfiguration worth surfacing, not a silent no-op).
pub fn collect_slack_dm(store: &Store, config: &SlackDmConfig, now_ms: i64) -> CollectSummary {
    const KEY: &str = "slack:dm";
    if config.token.trim().is_empty() || config.watch_user_id.trim().is_empty() {
        let msg = "slack collector needs both a token and a watch user id".to_string();
        return finish(store, KEY, false, 0, Some(msg), now_ms);
    }
    match slack::fetch_dm(config) {
        Ok(Some(dm)) => match store.upsert_dm(&dm, now_ms) {
            Ok(()) => finish(store, KEY, true, 1, None, now_ms),
            Err(e) => finish(store, KEY, false, 0, Some(e.to_string()), now_ms),
        },
        Ok(None) => finish(store, KEY, true, 0, Some("no messages in DM yet".to_string()), now_ms),
        Err(msg) => finish(store, KEY, false, 0, Some(msg), now_ms),
    }
}

/// Per-repo results of one collector sweep.
struct Sweep<T> {
    /// `(owner/name, items)` for every repo whose `gh` calls succeeded —
    /// including repos with zero items, which still need their rows cleared.
    successes: Vec<(String, Vec<T>)>,
    errors: Vec<String>,
    skipped: Vec<String>,
}

/// Max repos swept concurrently. Each PR repo costs ~2 `gh` subprocesses, so a
/// serial sweep of N repos is ~2N sequential network round-trips; fanning the
/// per-repo work across a small pool cuts wall time without hammering the `gh`
/// API. Kept modest deliberately — the win is overlapping network latency, not
/// saturating CPU.
const SWEEP_CONCURRENCY: usize = 4;

/// One repo's place in the sweep, tagged so results can be re-sorted into input
/// order after the parallel workers finish.
enum RepoOutcome<T> {
    Skipped(String),
    Ok((String, Vec<T>)),
    Err(String),
}

/// Run `collect_repo` over every existing repo dir, partitioning outcomes.
///
/// The per-repo `gh` calls are fanned across a bounded pool of scoped threads
/// (see [`SWEEP_CONCURRENCY`]) so their network latency overlaps. Each repo's
/// outcome is independent — one repo's error never sinks another's rows — and
/// the returned [`Sweep`] preserves input order (results are re-sorted by
/// position), so downstream dedup and full-vs-partial replace behave exactly as
/// they did serially.
fn sweep_repos<T: Send>(
    repo_dirs: &[PathBuf],
    collect_repo: impl Fn(&std::path::Path) -> Result<(String, Vec<T>), String> + Sync,
) -> Sweep<T> {
    let outcomes = parallel_map(repo_dirs, SWEEP_CONCURRENCY, |dir| {
        // Tracked repos can go stale (moved/deleted dirs); a missing cwd makes
        // `Command` fail with a misleading "gh not found" error, so skip them
        // here and surface the skip in the run message instead.
        if !dir.is_dir() {
            return RepoOutcome::Skipped(format!("skipped missing repo dir {}", dir.display()));
        }
        match collect_repo(dir) {
            Ok(result) => RepoOutcome::Ok(result),
            Err(e) => RepoOutcome::Err(e),
        }
    });

    let mut sweep = Sweep { successes: Vec::new(), errors: Vec::new(), skipped: Vec::new() };
    for outcome in outcomes {
        match outcome {
            RepoOutcome::Skipped(msg) => sweep.skipped.push(msg),
            RepoOutcome::Ok(result) => sweep.successes.push(result),
            RepoOutcome::Err(e) => sweep.errors.push(e),
        }
    }
    sweep
}

/// Dedupe tracked repo dirs by their resolved GitHub `owner/repo`, keeping the
/// first dir seen for each.
///
/// Every worktree of a repo is its own tracked directory but shares one
/// GitHub identity; sweeping all of them fires byte-identical `gh` queries
/// once per worktree straight into the GraphQL budget, which is per-token, not
/// per-directory (#322). Resolution reuses [`gh::repo_name_with_owner`]'s
/// process-lifetime cache, so after a dir's first tick this costs nothing —
/// the win is in the sweep this feeds skipping the expensive per-repo `gh`
/// calls (PR/issue lists) for every duplicate dir, on every subsequent tick.
fn dedupe_repo_dirs(repo_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let resolved = parallel_map(repo_dirs, SWEEP_CONCURRENCY, |dir| {
        (dir.clone(), gh::repo_name_with_owner(dir))
    });
    dedupe_resolved(resolved)
}

/// Pure half of [`dedupe_repo_dirs`]: keep the first dir per successfully
/// resolved name, plus every dir whose resolution failed. A failed resolution
/// can't prove two dirs are the same repo, so it's kept as-is — its error
/// still surfaces from `collect_repo_{issues,prs}` exactly as it would have
/// without this dedup pass, rather than being silently dropped.
fn dedupe_resolved(resolved: Vec<(PathBuf, Result<String, String>)>) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    resolved
        .into_iter()
        .filter_map(|(dir, name)| match name {
            Ok(name) => seen.insert(name).then_some(dir),
            Err(_) => Some(dir),
        })
        .collect()
}

/// Apply `f` to every item across up to `max_workers` scoped threads, returning
/// the results in the input order regardless of completion order.
///
/// A simple shared atomic cursor hands each idle worker the next index (a bounded
/// work queue), so a slow repo doesn't stall the others. Each worker keeps its
/// own `(index, output)` pairs; after the scope joins they are merged and sorted
/// by index, making the output deterministic. Panics in `f` propagate on join —
/// but the collectors never panic (their contract), so in practice they don't.
fn parallel_map<In, Out>(
    items: &[In],
    max_workers: usize,
    f: impl Fn(&In) -> Out + Sync,
) -> Vec<Out>
where
    In: Sync,
    Out: Send,
{
    use std::sync::atomic::{AtomicUsize, Ordering};

    let len = items.len();
    if len == 0 {
        return Vec::new();
    }

    let next = AtomicUsize::new(0);
    let workers = max_workers.clamp(1, len);
    let f = &f;

    let mut collected: Vec<(usize, Out)> = Vec::with_capacity(len);
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut local: Vec<(usize, Out)> = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= len {
                            break;
                        }
                        local.push((i, f(&items[i])));
                    }
                    local
                })
            })
            .collect();
        for handle in handles {
            collected.extend(handle.join().expect("sweep worker thread panicked"));
        }
    });

    collected.sort_by_key(|(i, _)| *i);
    collected.into_iter().map(|(_, out)| out).collect()
}

/// Apply a sweep's results to the store and record the run.
///
/// `write(all, None)` performs a full-table replace; `write(all, Some(repos))`
/// replaces only the named repos' rows. `key_of` yields the `(repo, number)`
/// identity used to dedup items collected from two checkouts of one repo
/// (parallel worktrees).
fn finish_sweep<T>(
    store: &Store,
    key: &str,
    sweep: Sweep<T>,
    write: impl Fn(&[T], Option<&[String]>) -> tt_store::Result<usize>,
    key_of: impl Fn(&T) -> (String, i64),
    now_ms: i64,
) -> CollectSummary {
    let Sweep { successes, errors, skipped } = sweep;

    if successes.is_empty() {
        // Nothing succeeded: never touch existing rows. All-skipped (or an
        // empty tracked list) is a clean no-op; any error marks the run failed.
        let ok = errors.is_empty();
        let mut notes: Vec<String> = errors.into_iter().chain(skipped).collect();
        if notes.is_empty() {
            notes.push("no repos configured".to_string());
        }
        return finish(store, key, ok, 0, Some(notes.join("; ")), now_ms);
    }

    let repos: Vec<String> = successes.iter().map(|(repo, _)| repo.clone()).collect();
    let mut by_key: std::collections::HashMap<(String, i64), T> = std::collections::HashMap::new();
    for (_, items) in successes {
        for item in items {
            by_key.insert(key_of(&item), item);
        }
    }
    let all: Vec<T> = by_key.into_values().collect();
    let count = all.len();

    let clean_sweep = errors.is_empty() && skipped.is_empty();
    let scope = if clean_sweep { None } else { Some(repos.as_slice()) };
    if let Err(e) = write(&all, scope) {
        return finish(store, key, false, count, Some(e.to_string()), now_ms);
    }

    let notes: Vec<String> = errors.iter().cloned().chain(skipped).collect();
    let message = if notes.is_empty() { None } else { Some(notes.join("; ")) };
    finish(store, key, errors.is_empty(), count, message, now_ms)
}

/// Run every collector: calendar, issues, then PRs.
pub fn collect_all(
    store: &Store,
    calendar_sources: &[CalendarSource],
    repo_dirs: &[PathBuf],
    now_ms: i64,
) -> Vec<CollectSummary> {
    vec![
        collect_calendar(store, calendar_sources, now_ms),
        collect_issues(store, repo_dirs, now_ms),
        collect_prs(store, repo_dirs, now_ms),
    ]
}

/// Run the collectors a manual "refresh now" fires: issues, then PRs, then —
/// only when a Slack config is supplied — the watched DM. Calendar is
/// deliberately excluded: every calendar run spends `claude` tokens, so it
/// stays on its scheduled cadence and is never triggered by a button press.
/// `slack` is `Some` only when the collector is enabled and configured (the
/// caller decides), so passing `None` cleanly skips it rather than recording a
/// misconfiguration failure on every manual refresh.
pub fn collect_manual(
    store: &Store,
    repo_dirs: &[PathBuf],
    slack: Option<&SlackDmConfig>,
    now_ms: i64,
) -> Vec<CollectSummary> {
    let mut summaries = vec![
        collect_issues(store, repo_dirs, now_ms),
        collect_prs(store, repo_dirs, now_ms),
    ];
    if let Some(config) = slack {
        summaries.push(collect_slack_dm(store, config, now_ms));
    }
    summaries
}

/// Sync one repo's issues + PRs immediately — the Agentboard rail's manual
/// "Sync now" action, for pulling in GitHub updates the poll cadence hasn't
/// picked up yet. Unlike [`collect_issues`]/[`collect_prs`], this never takes
/// their full-table-replace path: even a totally clean run only touches
/// `dir`'s own rows (via the `_for_repos` scoped write), so syncing one repo
/// can never wipe another tracked repo's cached issues/PRs. A missing `dir`
/// is recorded as a failure (unlike a sweep's silent skip) — this is a
/// targeted action on a specific repo, not a passive background pass.
///
/// Still records the shared `issues`/`prs` `record_run` rows like the
/// sweep-based collectors: that freshness timestamp tracks whether the
/// collector engine itself is healthy, not per-repo coverage, so a scoped run
/// updating it is consistent with what it already means.
pub fn collect_repo_now(store: &Store, dir: &Path, now_ms: i64) -> Vec<CollectSummary> {
    if !dir.is_dir() {
        let msg = format!("repo directory not found: {}", dir.display());
        return vec![
            finish(store, "issues", false, 0, Some(msg.clone()), now_ms),
            finish(store, "prs", false, 0, Some(msg), now_ms),
        ];
    }
    let issues_summary = collect_repo_issues_now(store, dir, now_ms);
    let prs_summary = collect_repo_prs_now(store, dir, now_ms);
    sync_task_links(store, std::slice::from_ref(&dir.to_path_buf()), now_ms);
    vec![issues_summary, prs_summary]
}

fn collect_repo_issues_now(store: &Store, dir: &Path, now_ms: i64) -> CollectSummary {
    write_repo_issues_now(store, issues::collect_repo_issues(dir), now_ms)
}

/// Apply one repo's freshly-collected issues via the scoped
/// [`Store::replace_issues_for_repos`] write — factored out from
/// [`collect_repo_issues_now`] so the scoping behavior is unit-testable
/// without shelling out to `gh`.
fn write_repo_issues_now(
    store: &Store,
    result: Result<(String, Vec<tt_store::IssueInput>), String>,
    now_ms: i64,
) -> CollectSummary {
    match result {
        Ok((repo, items)) => match store.replace_issues_for_repos(&[repo], &items) {
            Ok(count) => finish(store, "issues", true, count, None, now_ms),
            Err(e) => finish(store, "issues", false, 0, Some(e.to_string()), now_ms),
        },
        Err(e) => finish(store, "issues", false, 0, Some(e), now_ms),
    }
}

fn collect_repo_prs_now(store: &Store, dir: &Path, now_ms: i64) -> CollectSummary {
    write_repo_prs_now(store, prs::collect_repo_prs(dir), now_ms)
}

/// Apply one repo's freshly-collected PRs via the scoped
/// [`Store::replace_prs_for_repos`] write — factored out from
/// [`collect_repo_prs_now`] so the scoping behavior is unit-testable without
/// shelling out to `gh`.
fn write_repo_prs_now(
    store: &Store,
    result: Result<(String, Vec<tt_store::PrInput>), String>,
    now_ms: i64,
) -> CollectSummary {
    match result {
        Ok((repo, items)) => match store.replace_prs_for_repos(&[repo], &items) {
            Ok(count) => finish(store, "prs", true, count, None, now_ms),
            Err(e) => finish(store, "prs", false, 0, Some(e.to_string()), now_ms),
        },
        Err(e) => finish(store, "prs", false, 0, Some(e), now_ms),
    }
}

/// The tracked repo directories from the agentboard repos config, or an empty
/// vec if the config is missing/empty.
pub fn tracked_repo_dirs() -> Vec<PathBuf> {
    let path = tt_agentboard::repos::default_repos_path();
    tt_agentboard::repos::load_repos(&path).into_iter().map(PathBuf::from).collect()
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Record the run and build the matching summary. A failed `record_run` write is
/// ignored (the collector contract forbids surfacing it as an error/panic).
fn finish(
    store: &Store,
    collector: &str,
    ok: bool,
    count: usize,
    message: Option<String>,
    now_ms: i64,
) -> CollectSummary {
    let _ = store.record_run(collector, ok, message.as_deref(), now_ms);
    CollectSummary { collector: collector.to_string(), ok, count, message }
}

/// Run `claude -p <prompt>` (capped at [`CLAUDE_TIMEOUT`]) and extract a JSON
/// value from its stdout. Returns a human-readable error string on spawn
/// failure, timeout, non-zero exit, or no parseable JSON.
fn run_claude(prompt: &str) -> Result<serde_json::Value, String> {
    log::debug!("claude -p ({} byte prompt)", prompt.len());
    let output = tt_exec::run_with_timeout("claude", &["-p", prompt], CLAUDE_TIMEOUT)
        .map_err(|e| e.to_string())?;
    if !output.ok() {
        let stderr = output.stderr.trim();
        return Err(if stderr.is_empty() {
            format!("claude exited with code {}", output.exit_code)
        } else {
            format!("claude failed: {stderr}")
        });
    }
    extract_json(&output.stdout).ok_or_else(|| "no parseable JSON in claude output".to_string())
}

/// Leniently extract the first parseable balanced JSON array or object from
/// `raw`.
///
/// Bracket-scans (respecting strings and escapes) from each `[`/`{` in turn; a
/// candidate that is unbalanced or fails to parse — prose like `[3 total]`
/// ahead of the real payload — moves the scan to the next opener instead of
/// giving up. The raw text is never rewritten (a fence marker inside a JSON
/// string must survive), and fences don't need stripping: the scan simply
/// starts at the first opener. Returns `None` when nothing in `raw` parses.
pub fn extract_json(raw: &str) -> Option<serde_json::Value> {
    let mut from = 0;
    while let Some(offset) = raw[from..].find(['[', '{']) {
        let start = from + offset;
        if let Some(value) = parse_balanced_at(raw, start) {
            return Some(value);
        }
        // This opener didn't yield JSON; resume after it.
        from = start + 1;
    }
    None
}

/// Parse the balanced bracket run starting at byte `start` (which must be `[`
/// or `{`), or `None` if it never closes or isn't valid JSON.
fn parse_balanced_at(raw: &str, start: usize) -> Option<serde_json::Value> {
    let (open, close) = if raw.as_bytes()[start] == b'[' { ('[', ']') } else { ('{', '}') };

    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in raw[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            c if c == open => depth += 1,
            c if c == close => {
                depth -= 1;
                if depth == 0 {
                    let end = start + offset + ch.len_utf8();
                    return serde_json::from_str(&raw[start..end]).ok();
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tt_store::TaskOutcome;

    #[test]
    fn extract_clean_array() {
        let v = extract_json(r#"[{"a":1},{"a":2}]"#).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 2);
    }

    #[test]
    fn extract_fenced_array() {
        let raw = "```json\n[1, 2, 3]\n```";
        assert_eq!(extract_json(raw).unwrap().as_array().unwrap().len(), 3);
    }

    #[test]
    fn extract_prose_wrapped_object() {
        let raw = "Sure! Here is the data you asked for:\n{\"events\": []}\nHope that helps.";
        let v = extract_json(raw).unwrap();
        assert!(v.get("events").is_some());
    }

    #[test]
    fn extract_object_with_nested_arrays_and_braces_in_strings() {
        let raw = r#"{"title": "a } weird ] title", "attendees": ["x", "y"]}"#;
        let v = extract_json(raw).unwrap();
        assert_eq!(v.get("title").unwrap(), "a } weird ] title");
        assert_eq!(v.get("attendees").unwrap().as_array().unwrap().len(), 2);
    }

    #[test]
    fn extract_unbalanced_array_salvages_inner_object() {
        // The array never closes, but the scan moves to the next opener and
        // rescues the complete object inside it.
        let v = extract_json(r#"[{"a": 1}"#).unwrap();
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn extract_fully_unbalanced_is_none() {
        assert!(extract_json(r#"[{"a": 1"#).is_none());
    }

    #[test]
    fn extract_skips_prose_brackets_before_the_payload() {
        // claude routinely narrates before the JSON; a bracketed fragment in
        // that prose must not abort extraction.
        let raw = r#"Here are today's events [3 total]:
[{"externalId":"e1","title":"standup","startTs":1}]"#;
        let v = extract_json(raw).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 1);
        assert_eq!(v[0]["title"], "standup");
    }

    #[test]
    fn extract_skips_unparseable_brace_fragment() {
        let raw = r#"I'll check {your} calendar: [{"title":"standup"}]"#;
        let v = extract_json(raw).unwrap();
        assert_eq!(v[0]["title"], "standup");
    }

    #[test]
    fn extract_preserves_fence_marker_inside_string_values() {
        // The old implementation rewrote the raw text to strip fences, which
        // corrupted fence markers inside JSON strings.
        let raw = "```json\n{\"title\": \"use ```json blocks\"}\n```";
        let v = extract_json(raw).unwrap();
        assert_eq!(v["title"], "use ```json blocks");
    }

    #[test]
    fn extract_error_sentence_is_none() {
        assert!(extract_json("I could not access your calendar tools.").is_none());
    }

    #[test]
    fn collect_prs_no_repos_is_clean_noop() {
        let store = Store::open_in_memory().unwrap();
        let summary = collect_prs(&store, &[], 1);
        assert!(summary.ok);
        assert_eq!(summary.count, 0);
        assert_eq!(summary.message.as_deref(), Some("no repos configured"));
        // The run is recorded under the `prs` key.
        let runs = store.runs().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].collector, "prs");
        assert!(runs[0].ok);
    }

    #[test]
    fn collect_prs_open_and_collect_prs_merged_are_clean_noops_and_share_the_prs_key() {
        let store = Store::open_in_memory().unwrap();
        let open = collect_prs_open(&store, &[], 1);
        assert!(open.ok);
        assert_eq!(open.count, 0);
        let merged = collect_prs_merged(&store, &[], 2);
        assert!(merged.ok);
        assert_eq!(merged.count, 0);
        // Both cadences are splits of the same logical collector, so they
        // record under the same `prs` key rather than two separate ones.
        let runs = store.runs().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].collector, "prs");
    }

    #[test]
    fn collect_manual_runs_issues_and_prs_but_never_calendar() {
        let store = Store::open_in_memory().unwrap();
        let summaries = collect_manual(&store, &[], None, 1);
        let keys: Vec<&str> = summaries.iter().map(|s| s.collector.as_str()).collect();
        assert_eq!(keys, ["issues", "prs"], "manual refresh runs issues + PRs, skips calendar");
        assert!(!keys.contains(&"claude:calendar"), "calendar is never manually triggered");
    }

    #[test]
    fn collect_manual_includes_slack_only_when_configured() {
        let store = Store::open_in_memory().unwrap();
        // `watch_user_id` left empty so `collect_slack_dm` records its
        // misconfiguration failure and returns before any network call — enough
        // to prove the slack summary is appended when a config is supplied.
        let config = SlackDmConfig {
            token: "xoxp-test".to_string(),
            watch_user_id: String::new(),
            watch_name: String::new(),
        };
        let keys: Vec<String> = collect_manual(&store, &[], Some(&config), 1)
            .into_iter()
            .map(|s| s.collector)
            .collect();
        assert_eq!(keys, ["issues", "prs", "slack:dm"]);
    }

    #[test]
    fn collect_issues_no_repos_is_clean_noop() {
        let store = Store::open_in_memory().unwrap();
        let summary = collect_issues(&store, &[], 1);
        assert!(summary.ok);
        assert_eq!(summary.count, 0);
        assert_eq!(summary.message.as_deref(), Some("no repos configured"));
        let runs = store.runs().unwrap();
        assert_eq!(runs[0].collector, "issues");
    }

    #[test]
    fn rollup_moves_task_to_done_when_all_links_resolve() {
        let store = Store::open_in_memory().unwrap();
        store.replace_issues(&[issue("o/r", 1)]).unwrap();
        let task = store.add_task("linked", "backlog", None, 1).unwrap();
        store.attach_task_issue(task.id, "o/r", 1, "https://github.com/o/r/issues/1").unwrap();
        store.attach_task_pr(task.id, "o/r", 10, "https://github.com/o/r/pull/10").unwrap();

        // Everything open: no-op.
        store.refresh_link_states_from_cache(2).unwrap();
        assert_eq!(rollup_task_statuses(&store, 2).unwrap(), 0);
        assert_eq!(store.get_task(task.id).unwrap().unwrap().status, "backlog");

        // Issue closes but the PR is still open → card stays put.
        let mut closed = issue("o/r", 1);
        closed.state = "closed".to_string();
        store.replace_issues(&[closed.clone()]).unwrap();
        store.refresh_link_states_from_cache(3).unwrap();
        assert_eq!(rollup_task_statuses(&store, 3).unwrap(), 0);
        assert_eq!(store.get_task(task.id).unwrap().unwrap().status, "backlog");

        // PR merges too (learned via targeted fetch) → all resolved → done.
        store.set_pr_link_state("o/r", 10, "merged", None, 4).unwrap();
        assert_eq!(rollup_task_statuses(&store, 4).unwrap(), 1);
        assert_eq!(store.get_task(task.id).unwrap().unwrap().status, "done");

        // Issue reopens on GitHub → done task falls back to backlog.
        store.replace_issues(&[issue("o/r", 1)]).unwrap();
        store.refresh_link_states_from_cache(5).unwrap();
        assert_eq!(rollup_task_statuses(&store, 5).unwrap(), 1);
        assert_eq!(store.get_task(task.id).unwrap().unwrap().status, "backlog");
    }

    #[test]
    fn rollup_never_overturns_an_explicit_outcome() {
        // A task closed as abandoned whose linked PR later reads merged: the
        // rollup must not resurrect-and-complete it (set_task_status would
        // clear the outcome as a side effect), and a reopened ref must not
        // drag an archived done task back to backlog.
        let store = Store::open_in_memory().unwrap();
        let task = store.add_task("closed", "doing", None, 1).unwrap();
        store.attach_task_pr(task.id, "o/r", 10, "u").unwrap();
        store.set_pr_link_state("o/r", 10, "merged", None, 2).unwrap();
        store.close_task(task.id, TaskOutcome::Abandoned, 3).unwrap();

        assert_eq!(rollup_task_statuses(&store, 4).unwrap(), 0);
        let after = store.get_task(task.id).unwrap().unwrap();
        assert_eq!(after.status, "doing", "frozen where it was closed");
        assert_eq!(after.outcome.as_deref(), Some("abandoned"));
    }

    #[test]
    fn sync_sweep_archives_old_finished_tasks() {
        // The sweep rides sync_task_links' cadence: anything finished more
        // than ARCHIVE_AFTER_MS ago leaves the active board.
        let store = Store::open_in_memory().unwrap();
        let old = store.add_task("old", "doing", None, 1).unwrap();
        store.close_task(old.id, TaskOutcome::Done, 10).unwrap();
        let fresh = store.add_task("fresh", "doing", None, 2).unwrap();
        store.close_task(fresh.id, TaskOutcome::Done, 500).unwrap();

        let now = tt_store::ARCHIVE_AFTER_MS + 100;
        store.archive_closed_tasks(now - tt_store::ARCHIVE_AFTER_MS, now).unwrap();
        assert!(store.get_task(old.id).unwrap().unwrap().archived_at.is_some());
        assert!(store.get_task(fresh.id).unwrap().unwrap().archived_at.is_none());
    }

    #[test]
    fn rollup_ignores_linkless_tasks_and_manual_non_done_moves() {
        let store = Store::open_in_memory().unwrap();
        let plain = store.add_task("plain", "backlog", None, 1).unwrap();
        store.set_task_status(plain.id, "doing", 1).unwrap();

        assert_eq!(rollup_task_statuses(&store, 2).unwrap(), 0);
        assert_eq!(store.get_task(plain.id).unwrap().unwrap().status, "doing");
    }

    #[test]
    fn absent_refs_are_never_inferred_closed() {
        // The regression the old reconcile had: an issue that merely left the
        // snapshot (reassigned away, collector failure) must not roll its task
        // to done. State only changes via the snapshot or a targeted fetch.
        let store = Store::open_in_memory().unwrap();
        store.replace_issues(&[issue("o/r", 1)]).unwrap();
        let task = store.add_task("linked", "doing", None, 1).unwrap();
        store.attach_task_issue(task.id, "o/r", 1, "u").unwrap();
        store.refresh_link_states_from_cache(2).unwrap();

        // The ref vanishes from the snapshot entirely.
        store.replace_issues(&[]).unwrap();
        store.refresh_link_states_from_cache(3).unwrap();
        assert_eq!(rollup_task_statuses(&store, 3).unwrap(), 0);
        assert_eq!(store.get_task(task.id).unwrap().unwrap().status, "doing");
        // …but it is reported for a targeted fetch.
        assert_eq!(
            store.open_issue_refs_missing_from_cache().unwrap(),
            vec![("o/r".to_string(), 1)]
        );
    }

    fn cal_event(ext: &str, start_ts: i64) -> EventInput {
        EventInput {
            external_id: ext.to_string(),
            title: format!("event {ext}"),
            start: chrono::DateTime::from_timestamp_millis(start_ts).unwrap().fixed_offset(),
            end: chrono::DateTime::from_timestamp_millis(start_ts + 30 * 60 * 1000)
                .map(|e| e.fixed_offset()),
            attendees: vec![],
            location: None,
            join_url: None,
        }
    }

    /// Local noon on a fixed day, so a `now + 1h` event stays inside the same
    /// local calendar day on any machine's time zone (avoids midnight flakiness).
    fn local_noon_ms() -> i64 {
        use chrono::{Local, TimeZone};
        Local.with_ymd_and_hms(2026, 7, 12, 12, 0, 0).unwrap().timestamp_millis()
    }

    /// Write `events` into `source`'s lane for the local day containing `now`,
    /// the same way [`collect_calendar`] does.
    fn seed(store: &Store, source: &str, events: &[EventInput], now: i64) {
        let (start, end) = Store::local_day_bounds(now);
        store.replace_events_for_source(source, start, end, events, now).unwrap();
    }

    /// Run the store half of one source's pull for the local day containing
    /// `now` — what `collect_calendar_source` does after `claude -p` returns.
    fn apply(
        store: &Store,
        source: &str,
        events: &[EventInput],
        now: i64,
    ) -> Result<usize, String> {
        let (start, end) = Store::local_day_bounds(now);
        store_calendar_events(store, source, start, end, events, now)
    }

    #[test]
    fn local_day_bounds_bracket_now() {
        let now = local_noon_ms();
        let (start, end) = Store::local_day_bounds(now);
        assert!(start <= now && now < end, "now falls inside its own local day");
        // A day is 23–25h wide depending on DST; noon is always well inside it.
        assert!(end - start >= 23 * 60 * 60 * 1000);
        assert!(end - start <= 25 * 60 * 60 * 1000);
    }

    #[test]
    fn calendar_non_empty_result_replaces_that_sources_day() {
        let store = Store::open_in_memory().unwrap();
        let now = local_noon_ms();
        seed(&store, "google", &[cal_event("old", now + 60 * 60 * 1000)], now);

        let fresh = cal_event("new", now + 2 * 60 * 60 * 1000);
        assert_eq!(apply(&store, "google", std::slice::from_ref(&fresh), now).unwrap(), 1);

        let stored = store.events_between(now, now + 24 * 60 * 60 * 1000).unwrap();
        assert_eq!(stored.len(), 1, "the day's replace swapped the old row for the new one");
        assert_eq!(stored[0].external_id, "new");
    }

    #[test]
    fn calendar_empty_result_with_future_events_preserves_and_fails() {
        let store = Store::open_in_memory().unwrap();
        let now = local_noon_ms();
        seed(&store, "google", &[cal_event("keep", now + 60 * 60 * 1000)], now);

        let err = apply(&store, "google", &[], now).unwrap_err();

        assert!(err.contains("kept existing events"));
        let stored = store.events_between(now, now + 24 * 60 * 60 * 1000).unwrap();
        assert_eq!(stored.len(), 1, "existing future events survive the empty sweep");
        assert_eq!(stored[0].external_id, "keep");
    }

    #[test]
    fn calendar_empty_result_with_no_future_events_clears_ok() {
        let store = Store::open_in_memory().unwrap();
        let now = local_noon_ms();
        // Only a past event today; nothing still upcoming.
        seed(&store, "google", &[cal_event("past", now - 60 * 60 * 1000)], now);

        assert_eq!(apply(&store, "google", &[], now).unwrap(), 0, "a genuinely empty day is fine");
        assert!(
            store
                .events_between(now - 24 * 60 * 60 * 1000, now + 24 * 60 * 60 * 1000)
                .unwrap()
                .is_empty(),
            "the empty result clears the stale past row"
        );
    }

    #[test]
    fn calendar_empty_result_on_empty_store_is_clean_noop() {
        let store = Store::open_in_memory().unwrap();
        let now = local_noon_ms();
        assert_eq!(apply(&store, "google", &[], now).unwrap(), 0);
    }

    #[test]
    fn one_sources_empty_sweep_neither_consults_nor_clears_another() {
        // The whole point of per-source scoping: outlook coming back empty must
        // not be excused by google's rows, and must not delete them either.
        let store = Store::open_in_memory().unwrap();
        let now = local_noon_ms();
        seed(&store, "google", &[cal_event("personal", now + 60 * 60 * 1000)], now);

        // Outlook has no rows of its own → its empty result is genuinely empty,
        // even though google holds a future event.
        assert_eq!(apply(&store, "outlook", &[], now).unwrap(), 0);

        let stored = store.events_between(now, now + 24 * 60 * 60 * 1000).unwrap();
        assert_eq!(stored.len(), 1, "google's row is untouched by outlook's pull");
        assert_eq!(stored[0].external_id, "personal");
    }

    #[test]
    fn calendar_with_no_enabled_sources_is_a_clean_noop() {
        let store = Store::open_in_memory().unwrap();
        let sources: Vec<CalendarSource> = CalendarSource::defaults()
            .into_iter()
            .map(|s| CalendarSource { enabled: false, ..s })
            .collect();

        let summary = collect_calendar(&store, &sources, local_noon_ms());

        assert!(summary.ok);
        assert_eq!(summary.count, 0);
        assert_eq!(summary.message.as_deref(), Some("no calendar sources enabled"));
        // Still recorded under the single aggregate key.
        let runs = store.runs().unwrap();
        assert_eq!(runs[0].collector, "claude:calendar");
    }

    #[test]
    fn calendar_source_without_a_prompt_fails_without_spawning_claude() {
        let store = Store::open_in_memory().unwrap();
        let now = local_noon_ms();
        let sources = vec![CalendarSource {
            id: "google".to_string(),
            label: "Google".to_string(),
            enabled: true,
            prompt: "   ".to_string(),
        }];

        let summary = collect_calendar(&store, &sources, now);

        assert!(!summary.ok);
        assert_eq!(summary.message.as_deref(), Some("google: source has no prompt"));
    }

    fn issue(repo: &str, number: i64) -> tt_store::IssueInput {
        tt_store::IssueInput {
            repo: repo.to_string(),
            number,
            title: format!("issue {number}"),
            labels: vec![],
            state: "open".to_string(),
            url: format!("https://github.com/{repo}/issues/{number}"),
            updated_ts: 1,
        }
    }

    fn issue_write(
        store: &Store,
    ) -> impl Fn(&[tt_store::IssueInput], Option<&[String]>) -> tt_store::Result<usize> + '_ {
        |all, repos| match repos {
            None => store.replace_issues(all),
            Some(repos) => store.replace_issues_for_repos(repos, all),
        }
    }

    fn pr(repo: &str, number: i64) -> tt_store::PrInput {
        tt_store::PrInput {
            repo: repo.to_string(),
            number,
            title: format!("pr {number}"),
            branch: format!("branch-{number}"),
            state: "open".to_string(),
            checks: "none".to_string(),
            review_state: String::new(),
            url: format!("https://github.com/{repo}/pull/{number}"),
            updated_ts: 1,
        }
    }

    #[test]
    fn write_repo_issues_now_only_replaces_the_synced_repo() {
        // The bug this exists to prevent: `collect_issues` with a 1-element
        // repo_dirs slice would see a "clean" sweep and do a full-table
        // replace, wiping every other tracked repo's rows. The scoped write
        // must never do that.
        let store = Store::open_in_memory().unwrap();
        store.replace_issues(&[issue("o/a", 1), issue("o/b", 2)]).unwrap();

        let summary =
            write_repo_issues_now(&store, Ok(("o/a".to_string(), vec![issue("o/a", 9)])), 5);

        assert!(summary.ok);
        assert_eq!(summary.count, 1);
        let issues = store.issues().unwrap();
        assert!(issues.iter().any(|i| i.repo == "o/a" && i.number == 9), "synced repo's fresh row");
        assert!(issues.iter().any(|i| i.repo == "o/b" && i.number == 2), "other repo untouched");
        assert!(!issues.iter().any(|i| i.number == 1), "synced repo's stale row is gone");
    }

    #[test]
    fn write_repo_issues_now_records_the_error_and_keeps_existing_rows_on_failure() {
        let store = Store::open_in_memory().unwrap();
        store.replace_issues(&[issue("o/a", 1)]).unwrap();

        let summary = write_repo_issues_now(&store, Err("gh: rate limited".to_string()), 5);

        assert!(!summary.ok);
        assert_eq!(summary.message.as_deref(), Some("gh: rate limited"));
        assert_eq!(store.issues().unwrap().len(), 1, "existing rows survive a failed sync");
    }

    #[test]
    fn write_repo_prs_now_only_replaces_the_synced_repo() {
        let store = Store::open_in_memory().unwrap();
        store.replace_prs(&[pr("o/a", 1), pr("o/b", 2)]).unwrap();

        let summary = write_repo_prs_now(&store, Ok(("o/a".to_string(), vec![pr("o/a", 9)])), 5);

        assert!(summary.ok);
        assert_eq!(summary.count, 1);
        let prs = store.prs().unwrap();
        assert!(prs.iter().any(|p| p.repo == "o/a" && p.number == 9), "synced repo's fresh row");
        assert!(prs.iter().any(|p| p.repo == "o/b" && p.number == 2), "other repo untouched");
        assert!(!prs.iter().any(|p| p.number == 1), "synced repo's stale row is gone");
    }

    #[test]
    fn collect_repo_now_records_a_failure_for_a_missing_dir_without_touching_other_rows() {
        let store = Store::open_in_memory().unwrap();
        store.replace_issues(&[issue("o/a", 1)]).unwrap();
        store.replace_prs(&[pr("o/a", 1)]).unwrap();

        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("gone");
        let summaries = collect_repo_now(&store, &missing, 9);

        assert_eq!(summaries.len(), 2);
        assert!(summaries.iter().all(|s| !s.ok));
        assert!(
            summaries.iter().all(|s| s.message.as_deref().unwrap().contains("not found")),
            "missing dir is surfaced as a clear failure, not a silent skip"
        );
        assert_eq!(store.issues().unwrap().len(), 1, "no repo touched on a missing dir");
        assert_eq!(store.prs().unwrap().len(), 1, "no repo touched on a missing dir");
    }

    #[test]
    fn all_failed_sweep_preserves_existing_rows() {
        let store = Store::open_in_memory().unwrap();
        store.replace_issues(&[issue("o/a", 1)]).unwrap();

        let sweep = Sweep {
            successes: vec![],
            errors: vec!["gh: rate limited".to_string()],
            skipped: vec![],
        };
        let summary = finish_sweep(
            &store,
            "issues",
            sweep,
            issue_write(&store),
            |i| (i.repo.clone(), i.number),
            9,
        );

        assert!(!summary.ok);
        assert_eq!(store.issues().unwrap().len(), 1, "last-known-good rows survive a dead sweep");
        assert!(summary.message.unwrap().contains("rate limited"));
    }

    #[test]
    fn partial_sweep_replaces_only_succeeded_repos() {
        let store = Store::open_in_memory().unwrap();
        store.replace_issues(&[issue("o/a", 1), issue("o/b", 2)]).unwrap();

        // o/a re-collected (fresh row 3); o/b errored.
        let sweep = Sweep {
            successes: vec![("o/a".to_string(), vec![issue("o/a", 3)])],
            errors: vec!["gh failed in /repos/b: boom".to_string()],
            skipped: vec![],
        };
        let summary = finish_sweep(
            &store,
            "issues",
            sweep,
            issue_write(&store),
            |i| (i.repo.clone(), i.number),
            9,
        );

        assert!(!summary.ok, "a failed repo marks the run failed even though data was written");
        let issues = store.issues().unwrap();
        assert!(issues.iter().any(|i| i.repo == "o/a" && i.number == 3));
        assert!(issues.iter().any(|i| i.repo == "o/b" && i.number == 2), "failed repo keeps rows");
        assert!(!issues.iter().any(|i| i.number == 1), "succeeded repo's stale rows are gone");
    }

    #[test]
    fn clean_sweep_purges_untracked_repos() {
        let store = Store::open_in_memory().unwrap();
        store.replace_issues(&[issue("o/gone", 1)]).unwrap();

        let sweep = Sweep {
            successes: vec![("o/a".to_string(), vec![issue("o/a", 2)])],
            errors: vec![],
            skipped: vec![],
        };
        let summary = finish_sweep(
            &store,
            "issues",
            sweep,
            issue_write(&store),
            |i| (i.repo.clone(), i.number),
            9,
        );

        assert!(summary.ok);
        let issues = store.issues().unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].repo, "o/a", "full replace drops repos no longer tracked");
    }

    #[test]
    fn dedupe_resolved_keeps_first_dir_per_resolved_repo() {
        let deduped = dedupe_resolved(vec![
            (PathBuf::from("/worktrees/repo-a"), Ok("o/repo".to_string())),
            (PathBuf::from("/worktrees/repo-b"), Ok("o/repo".to_string())),
            (PathBuf::from("/worktrees/other"), Ok("o/other".to_string())),
        ]);
        assert_eq!(
            deduped,
            vec![
                PathBuf::from("/worktrees/repo-a"),
                PathBuf::from("/worktrees/other")
            ]
        );
    }

    #[test]
    fn dedupe_resolved_keeps_every_dir_that_fails_to_resolve() {
        // A dir whose resolution errors (offline, no gh auth, moved) can't be
        // proven a duplicate of anything, so it must survive the dedup pass —
        // its error still needs to surface from the real collect call.
        let deduped = dedupe_resolved(vec![
            (PathBuf::from("/worktrees/a"), Err("gh: not a git repo".to_string())),
            (PathBuf::from("/worktrees/b"), Err("gh: not a git repo".to_string())),
        ]);
        assert_eq!(deduped, vec![PathBuf::from("/worktrees/a"), PathBuf::from("/worktrees/b")]);
    }

    #[test]
    fn dedupe_resolved_is_empty_for_empty_input() {
        assert!(dedupe_resolved(vec![]).is_empty());
    }

    #[test]
    fn parallel_map_preserves_input_order() {
        // Reversed sleeps so completion order differs from input order; the
        // result must still come back sorted by input position.
        let inputs: Vec<usize> = (0..12).collect();
        let out = parallel_map(&inputs, 4, |&n| {
            std::thread::sleep(std::time::Duration::from_millis(((12 - n) % 5) as u64));
            n * 10
        });
        assert_eq!(out, (0..12).map(|n| n * 10).collect::<Vec<_>>());
    }

    #[test]
    fn parallel_sweep_contains_failures_and_keeps_order() {
        // Five tracked dirs; the middle one is missing (skipped) and one existing
        // repo errors. The surviving repos' results must come back in input order
        // and the failure must not sink them.
        let root = tempfile::tempdir().unwrap();
        let make = |name: &str| {
            let p = root.path().join(name);
            std::fs::create_dir(&p).unwrap();
            p
        };
        let repo_dirs = vec![
            make("repo0"),
            make("repo1"),
            root.path().join("gone"), // never created → skipped
            make("repo3"),            // stubbed to error below
            make("repo4"),
        ];

        // Stub collector: repo3 fails; every other existing repo yields one issue
        // whose number encodes its input position, so ordering is checkable.
        let collect =
            |dir: &std::path::Path| -> Result<(String, Vec<tt_store::IssueInput>), String> {
                let name = dir.file_name().unwrap().to_string_lossy().to_string();
                if name == "repo3" {
                    return Err("gh failed in repo3: boom".to_string());
                }
                let n: i64 = name.trim_start_matches("repo").parse().unwrap();
                Ok((name.clone(), vec![issue(&name, n)]))
            };

        let sweep = sweep_repos(&repo_dirs, collect);

        // Order preserved: successes follow input order, minus the skip and error.
        let repos: Vec<&str> = sweep.successes.iter().map(|(r, _)| r.as_str()).collect();
        assert_eq!(repos, ["repo0", "repo1", "repo4"], "successes keep input order");
        let numbers: Vec<i64> = sweep.successes.iter().map(|(_, v)| v[0].number).collect();
        assert_eq!(numbers, [0, 1, 4], "each surviving repo's rows are intact");

        assert_eq!(sweep.errors.len(), 1);
        assert!(sweep.errors[0].contains("repo3"), "the failing repo is reported");
        assert_eq!(sweep.skipped.len(), 1);
        assert!(sweep.skipped[0].contains("gone"), "the missing dir is skipped, not errored");
    }

    #[test]
    fn sweep_dedups_same_repo_from_two_checkouts() {
        let store = Store::open_in_memory().unwrap();
        // Two worktrees of one repo both succeed and report the same issue.
        let sweep = Sweep {
            successes: vec![
                ("o/a".to_string(), vec![issue("o/a", 1)]),
                ("o/a".to_string(), vec![issue("o/a", 1)]),
            ],
            errors: vec![],
            skipped: vec![],
        };
        let summary = finish_sweep(
            &store,
            "issues",
            sweep,
            issue_write(&store),
            |i| (i.repo.clone(), i.number),
            9,
        );
        assert!(summary.ok);
        assert_eq!(summary.count, 1);
        assert_eq!(store.issues().unwrap().len(), 1);
    }
}
