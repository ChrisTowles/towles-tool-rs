//! Towles Tool desktop app (Tauri 2). Hosts the agentboard bridge: an engine
//! (tracker/metadata/order/git/watcher) driven by tokio tasks that emits state
//! snapshots as the `agentboard://state` event and exposes client commands.
//! Also owns the embedded terminals (`terminal`): PTYs the app spawns and
//! kills on window close, rendered by xterm.js in the agentboard screen.

mod agentboard;
mod claude_sessions;
mod diagnostics;
mod doctor;
mod gh_actions;
mod ide;
mod instance_lock;
mod launch;
#[cfg(target_os = "linux")]
mod linux_desktop;
mod lsp;
mod mcp;
mod mcp_http;
mod preview;
mod resources;
mod resume;
mod scheduler;
mod settings;
mod slack;
mod slack_socket;
mod store;
mod task;
mod telemetry;
mod terminal;
mod update;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::{Emitter, Manager, WindowEvent};
use tokio::sync::Notify;

use agentboard::{Ab, Engine, STATE_EVENT, now_ms};
use tt_agentboard::fs_notify::{MultiFileNotifier, ScopedDirNotifier};

/// Human-readable name of the checkout this binary was built from — the repo-root
/// directory (e.g. `task-migrate`, `towles-tool-rs-primary`). Baked in at compile time from
/// `CARGO_MANIFEST_DIR` (`<root>/crates-tauri/tt-app`), so each task's binary
/// knows its own task without any runtime cwd/env plumbing. Lets several tasks'
/// windows be told apart in the title bar, taskbar, and app header.
pub(crate) fn task_label() -> String {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("towles-tool")
        .to_string()
}

/// Task name for the frontend header badge (see `task_label`).
#[tauri::command]
fn app_task() -> String {
    task_label()
}

/// The shared IPC seam for frontend `ui.action` telemetry (see the root
/// CLAUDE.md's `tt-telemetry` bullet): the webview can't reach `tracing`, so every
/// user gesture worth recording crosses here with a stable action id, the
/// screen it happened on, and an optional word of `detail` (an outcome, a
/// count — never content or continuous input).
#[tauri::command]
fn ui_action(action: String, screen: String, detail: Option<String>) {
    tracing::info!(%action, %screen, detail = %detail.as_deref().unwrap_or(""), "ui.action");
}

/// Per-task Tauri app identifier, so each worktree's self-installed
/// `.desktop` entry + icon (`linux_desktop::ensure_installed`) gets its own
/// filename instead of every task's binary overwriting the same one on
/// startup. The main checkout (not under a `.claude/worktrees/` parent)
/// keeps the base identifier unscoped, matching how it's the one instance
/// meant to daily-drive.
///
/// This used to also be load-bearing for `enableGTKAppId` (tauri.conf.json),
/// which made every same-identifier binary register as the *same*
/// D-Bus-activatable GTK application — any activation of it (a second
/// launch, but *also* a dock/taskbar icon click, `gio launch`, systemd, with
/// no new process involved at all) re-entered Tauri's internal `setup()` and
/// panicked rebuilding the config's `"main"` webview a second time.
/// `enableGTKAppId` is off now (that whole class of activation can't be
/// scoped away, only eliminated — see `linux_desktop`'s module doc), so this
/// identifier no longer affects GTK/D-Bus at all.
fn app_identifier(base: &str) -> String {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    // `<repo>/.claude/worktrees/<task>/crates-tauri/tt-app` — ancestors 3/4
    // are the worktrees/.claude segments exactly when this is a task build.
    let under_worktrees = manifest_dir.ancestors().nth(3).and_then(|p| p.file_name())
        == Some(std::ffi::OsStr::new("worktrees"))
        && manifest_dir.ancestors().nth(4).and_then(|p| p.file_name())
            == Some(std::ffi::OsStr::new(".claude"));
    if !under_worktrees {
        return base.to_string();
    }
    let suffix: String = task_label()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect();
    format!("{base}.task-{suffix}")
}

pub fn run() {
    // Errors always print to stderr, more with RUST_LOG. Independently, every
    // span/event streams to this task's on-disk event log at debug — the app
    // runs unattended for hours, so its telemetry has to already be captured
    // when a question comes up. A failure here must never block startup.
    let _ = tt_telemetry::init("tt-app", "error");

    // WebKitGTK's DMABUF renderer glitches on the NVIDIA proprietary driver:
    // small damage regions (e.g. a terminal cursor blink) flash as
    // window-sized artifacts (tauri-apps/tauri#9304). Opt out before any
    // webview exists, but only where NVIDIA is actually driving the screen,
    // and never override an explicit user setting.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none()
        && std::path::Path::new("/proc/driver/nvidia/version").exists()
    {
        // SAFETY: called before Tauri/GTK spawn any threads.
        unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
    }

    let mut context = tauri::generate_context!();
    let identifier = app_identifier(&context.config().identifier);
    context.config_mut().identifier = identifier.clone();

    // Set by scripts/dev-drive.mjs and scripts/e2e.mjs: both are
    // test/verification launches, never the user actually sitting down to
    // use the app, so the window shouldn't grab OS focus and yank them away
    // from whatever they were doing. A runtime signal, not `#[cfg(feature =
    // "wdio")]` — that feature means "wdio plugins are compiled in," a
    // different concern that only happens to correlate with these two
    // scripts today.
    if std::env::var_os("TT_NO_FOCUS_STEAL").is_some() {
        for window in &mut context.config_mut().app.windows {
            window.focus = false;
        }
    }

    // Guard against a second launch of the *same* checkout (same resolved
    // identifier — the primary, or one specific task) running concurrently:
    // with no GTK/D-Bus single-instance registration (`enableGTKAppId` is
    // off — see `linux_desktop`'s module doc for why), nothing else stops
    // two processes both opening a window, duplicating PTYs and scheduler
    // polling. Held for the whole process lifetime; a lock left by a killed
    // process is detected and stolen (see `InstanceLock`).
    let Some(_instance_lock) =
        instance_lock::InstanceLock::try_acquire(&format!("app-{identifier}"))
    else {
        eprintln!("Towles Tool ({identifier}) is already running — focus its existing window.");
        return;
    };

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init());

    // WebdriverIO E2E plugins, only under `--features wdio` (see e2e/):
    // tauri-plugin-wdio exposes the execute/mock IPC surface, and
    // tauri-plugin-wdio-webdriver runs the in-app WebDriver server the
    // @wdio/tauri-service embedded provider connects to.
    #[cfg(feature = "wdio")]
    let builder =
        builder.plugin(tauri_plugin_wdio::init()).plugin(tauri_plugin_wdio_webdriver::init());

    builder
        .setup(|app| {
            // Register the wdio capability at runtime (feature-gated) so normal
            // builds never reference the plugins' ACL and stay clean.
            #[cfg(feature = "wdio")]
            app.handle().add_capability(include_str!("../wdio-capability.json"))?;

            // Register (or refresh) the desktop entry + icon this app-id
            // resolves to — see linux_desktop's module doc for why this is
            // needed even outside a packaged build.
            #[cfg(target_os = "linux")]
            linux_desktop::ensure_installed(&app.config().identifier);

            // Distinguish concurrent task windows in the title bar / taskbar.
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.set_title(&format!("Towles Tool — {}", task_label()));
            }

            // Fire-and-forget: check GitHub for a newer release and, if one
            // exists, push the update banner event + an OS notification.
            update::check_on_startup(app.handle().clone());

            // A Debug-mode Zig parser saturates a core at ~130 KB/s of PTY
            // output, so busy terminals peg engine threads and the app reads
            // as laggy with no obvious cause. The Cargo.toml dev-profile
            // override should make this unreachable; be loud if it regresses
            // (the Doctor screen shows the same check).
            if tt_vt::parser_optimize_mode() == "Debug" {
                eprintln!(
                    "warning: libghostty-vt compiled in Zig Debug mode (~1000x slower parsing; \
                     busy terminals will peg a core) — restore the \
                     [profile.dev.package.libghostty-vt-sys] override in Cargo.toml"
                );
            }

            // Scope to this app instance: sessions.json is shared across
            // instances, so another running app's PTY can carry the same
            // session id — its agents are that window's to report, not ours.
            let engine = Arc::new(Mutex::new(Engine::new(
                tt_agentboard::procenv::InstanceScope::this_app(),
            )));
            let emit = Arc::new(Notify::new());
            let scan = Arc::new(Notify::new());

            // fs-notify accelerant: a tracked repo/worktree's journal change
            // signals an eager scan. Scoped to those checkouts' own
            // `~/.claude/projects` subdirectories — not the whole tree, which
            // every Claude Code session on the machine writes into (including
            // whatever session is editing this repo right now) — see
            // `ScopedDirNotifier`'s doc comment. The initial target set is
            // empty; the scan loop below calls `set_targets` on its first
            // tick and every one after, so this starts narrowing within 2s.
            let projects_dir = engine.lock().unwrap().projects_dir();
            let scan_for_notify = scan.clone();
            let notifier = Arc::new(Mutex::new(
                ScopedDirNotifier::new(move || scan_for_notify.notify_one()).ok(),
            ));

            // Event-driven git-info refresh: watches each tracked repo's own
            // `.git` internals (HEAD, index, refs, packed-refs — see
            // `Engine::control_watch_files`) so a real commit/fetch/branch-
            // switch/`git add` invalidates just that repo immediately instead
            // of waiting on `GIT_CACHE_TTL_MS`'s now-long backup ceiling. The
            // reverse-index (which dir owns which watched file) is rebuilt by
            // the scan loop below alongside its `MultiFileNotifier`
            // registration diff; the callback only resolves paths → dirs and
            // invalidates. Doesn't cover unstaged working-tree edits — see
            // `control_files_for`'s doc for why that still needs the poll.
            let git_watch_index: Arc<Mutex<HashMap<PathBuf, String>>> = Arc::default();
            let git_watcher = {
                let engine = engine.clone();
                let scan = scan.clone();
                let index = git_watch_index.clone();
                Arc::new(Mutex::new(
                    MultiFileNotifier::new(move |changed: Vec<PathBuf>| {
                        let dirs: HashSet<String> = {
                            let idx = index.lock().unwrap();
                            changed.iter().filter_map(|p| idx.get(p).cloned()).collect()
                        };
                        if dirs.is_empty() {
                            return;
                        }
                        {
                            let mut e = engine.lock().unwrap();
                            for dir in &dirs {
                                e.invalidate_git(dir);
                            }
                        }
                        scan.notify_one();
                    })
                    .ok(),
                ))
            };

            app.manage(Ab {
                engine: engine.clone(),
                emit: emit.clone(),
                scan: scan.clone(),
                needs_since: Mutex::new(tt_agentboard::bridge::NeedsSince::new()),
            });

            // Compiler-diagnostics hub for the Claude Code IDE bridge: fed by
            // CLI connects, working-tree changes (git-stat poll below), and
            // the manual refresh command; consumed by the per-terminal IDE
            // servers' getDiagnostics.
            let diag_hub = diagnostics::DiagHub::spawn(app.handle().clone());
            app.manage(diag_hub.clone());

            let handle = app.handle().clone();

            // Debounced emitter: coalesce a burst of triggers into one rebuild,
            // and broadcast only when the payload actually changed — every emit
            // costs the webview a deserialize + React re-render, and the scan
            // and git tickers signal unconditionally. The rebuild itself runs
            // on a blocking worker (it can shell out on cache misses).
            {
                let emit = emit.clone();
                tauri::async_runtime::spawn(async move {
                    // Compared with `ts` zeroed: the stamp changes every
                    // rebuild, the rest only when state does.
                    let mut last: Option<tt_agentboard::StatePayload> = None;
                    // Edge detector for needs-you desktop notifications: fires
                    // once per flip into needs-you, never on the level.
                    let mut needs_watch = tt_agentboard::NeedsYouWatch::new();
                    loop {
                        emit.notified().await;
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        let rebuild_handle = handle.clone();
                        let Ok(payload) = tauri::async_runtime::spawn_blocking(move || {
                            agentboard::stamped_payload(&rebuild_handle)
                        })
                        .await
                        else {
                            continue;
                        };
                        let mut probe = payload.clone();
                        probe.ts = 0;
                        if last.as_ref() != Some(&probe) {
                            last = Some(probe);
                            let edges = needs_watch.observe(&payload);
                            agentboard::notify_needs_you(&handle, &edges);
                            let _ = handle.emit(STATE_EVENT, payload);
                        }
                    }
                });
            }

            // Watcher scan: every 2s, or eagerly on a (debounced) fs-notify
            // signal. Warms any stale git-cache entries (e.g. a worktree
            // just created — always a cache miss) OUTSIDE the engine lock
            // first, same reasoning as the stat-poll below: `scan_once` and
            // every `ab_*` command share this lock and (on Linux) sync
            // commands dispatch inline on the GTK main thread, so a lock-held
            // git subprocess chain would freeze the whole app, not just this
            // loop. See `Engine::expand_with_worktrees`'s doc comment.
            {
                let engine = engine.clone();
                let emit = emit.clone();
                let scan = scan.clone();
                let notifier = notifier.clone();
                let projects_dir = projects_dir.clone();
                let git_watcher = git_watcher.clone();
                let git_watch_index = git_watch_index.clone();
                tauri::async_runtime::spawn(async move {
                    let mut interval = tokio::time::interval(Duration::from_millis(2000));
                    let mut git_watched: HashSet<PathBuf> = HashSet::new();
                    loop {
                        tokio::select! {
                            _ = interval.tick() => {}
                            _ = scan.notified() => {}
                        }
                        let now = now_ms();
                        let warm_engine = engine.clone();
                        let stale = tauri::async_runtime::spawn_blocking(move || {
                            warm_engine.lock().unwrap().stale_git_targets(now)
                        })
                        .await
                        .unwrap_or_default();
                        if !stale.is_empty() {
                            let warmed = tauri::async_runtime::spawn_blocking(move || {
                                stale
                                    .into_iter()
                                    .map(|(dir, base_branch, previous)| {
                                        let info = tt_agentboard::git_info::compute_git_info(
                                            &dir,
                                            base_branch.as_deref(),
                                            Some(&previous),
                                        );
                                        (dir, info)
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .await
                            .unwrap_or_default();
                            // Stamp with the time the batch actually *finished*,
                            // not the `now` from before it ran. Reusing the
                            // pre-batch timestamp made every entry born older
                            // than `GIT_CACHE_TTL_MS` whenever the batch outran
                            // the TTL, so the next tick found them stale again
                            // and recomputed immediately, forever.
                            let warmed_at = now_ms();
                            engine.lock().unwrap().warm_git_cache(warmed, warmed_at);
                        }
                        {
                            let mut e = engine.lock().unwrap();
                            e.scan_once(now);
                        }
                        // Narrow the fs-notify accelerant to the repos/worktrees
                        // actually being polled. Cheap (cache-only, see
                        // `Engine::watch_targets`'s doc) and a no-op unless the
                        // tracked set changed since last tick, so doing this
                        // every tick rather than only on repo add/remove is
                        // simpler and costs nothing extra.
                        let targets = engine.lock().unwrap().watch_targets();
                        if let Some(n) = notifier.lock().unwrap().as_mut() {
                            n.set_targets(&projects_dir, &targets);
                        }
                        // Same idea for the git-control-file watch: diff the
                        // desired set against what's registered, add/remove
                        // only the delta, and rebuild the path→dir reverse
                        // index the watcher callback resolves against. A dir
                        // with no cached info yet contributes no files (see
                        // `control_files_for`) — it starts being watched from
                        // whichever tick follows its first compute.
                        let desired = engine.lock().unwrap().control_watch_files();
                        if let Some(w) = git_watcher.lock().unwrap().as_mut() {
                            let desired_keys: HashSet<PathBuf> = desired.keys().cloned().collect();
                            for stale in
                                git_watched.difference(&desired_keys).cloned().collect::<Vec<_>>()
                            {
                                w.remove(&stale);
                            }
                            for fresh in
                                desired_keys.difference(&git_watched).cloned().collect::<Vec<_>>()
                            {
                                let _ = w.add(&fresh);
                            }
                            git_watched = desired_keys;
                        }
                        *git_watch_index.lock().unwrap() = desired;
                        emit.notify_one();
                    }
                });
            }

            // Git-stat poll: the diagnostics-hub half of git-info refresh,
            // outside the engine lock like the scan loop above (a slow/hung
            // git must never wedge the `ab_*` commands sharing the lock).
            // Staleness-gated via `stale_git_targets` — it used to
            // unconditionally recompute every tracked repo every tick
            // regardless of `GIT_CACHE_TTL_MS`, which meant this loop alone
            // kept every repo on a hard 10s recompute cadence no matter how
            // long the TTL or how precise the control-file invalidation
            // above got; nothing downstream of *this* loop ever benefited
            // from either. Now it shares the exact same staleness signal the
            // scan loop's `warm_git_cache` uses, so in steady state (nothing
            // invalidated) this tick finds nothing to do.
            {
                let engine = engine.clone();
                let emit = emit.clone();
                let diag = diag_hub.clone();
                tauri::async_runtime::spawn(async move {
                    let mut interval = tokio::time::interval(Duration::from_secs(10));
                    loop {
                        interval.tick().await;
                        let poll_engine = engine.clone();
                        let now = now_ms();
                        let changed_dirs = tauri::async_runtime::spawn_blocking(move || {
                            let targets = poll_engine.lock().unwrap().stale_git_targets(now);
                            let mut changed_dirs = Vec::new();
                            for (dir, base_branch, previous) in targets {
                                let info = tt_agentboard::git_info::compute_git_info(
                                    &dir,
                                    base_branch.as_deref(),
                                    Some(&previous),
                                );
                                let stored = poll_engine.lock().unwrap().store_git_info(
                                    &dir,
                                    info,
                                    now_ms(),
                                );
                                if stored {
                                    changed_dirs.push(dir);
                                }
                            }
                            changed_dirs
                        })
                        .await
                        .unwrap_or_default();
                        if !changed_dirs.is_empty() {
                            emit.notify_one();
                            // A folder whose working tree moved has stale
                            // diagnostics; the hub skips folders without a
                            // connected Claude session.
                            for dir in &changed_dirs {
                                diag.request(std::path::Path::new(dir));
                            }
                        }
                    }
                });
            }

            // Background fetch: `git fetch origin` every 3 minutes per tracked
            // repo (deduped across worktrees/tasks), outside the engine lock
            // like the stat poll above. The 10s git-stat poll only reads
            // already-cached remote-tracking refs, so without this,
            // "commits behind main" never updates until the user happens to
            // fetch some other way (opening a terminal, `tt task create`,
            // `tt gh` commands). No re-emit here — the next stat-poll tick
            // picks up whatever the fetch changed.
            {
                let engine = engine.clone();
                tauri::async_runtime::spawn(async move {
                    let mut interval = tokio::time::interval(Duration::from_secs(180));
                    loop {
                        interval.tick().await;
                        let fetch_engine = engine.clone();
                        let _ = tauri::async_runtime::spawn_blocking(move || {
                            let targets: Vec<String> = fetch_engine
                                .lock()
                                .unwrap()
                                .git_targets()
                                .into_iter()
                                .map(|(dir, _, _)| dir)
                                .collect();
                            tt_agentboard::git_info::fetch_all(&targets);
                        })
                        .await;
                    }
                });
            }

            // Personal-dashboard store + journal logging. Open the store once; if it
            // fails, the app still runs and store commands return an error.
            let store_state = store::StoreState::open();
            // Emit the initial store snapshot for the dashboard's first mount.
            store::emit_snapshot(&app.handle().clone(), &store_state);
            let repo_cache_store = store_state.clone();
            app.manage(store_state);

            // Tracked-repo identity cache: every 10s, reconcile `tt-store`'s
            // `repos` table (repo root -> `owner/repo`) from the currently
            // tracked repos' cache-only git origin (no subprocess — reads
            // whatever the scan/stat-poll loops above already computed). This
            // is what lets `tt-mcp`'s `task_create` validate its `repo`
            // argument against a real GitHub slug instead of a dir/basename
            // match; `repos.json` (via the engine's tracked-repo list) stays
            // the sole source of truth for which repos exist; a repo that's
            // untracked or whose origin becomes unparseable just drops out of
            // the next reconcile, with no separate untrack path to keep in
            // sync. See `tt_store::Store::reconcile_repos`.
            {
                let engine = engine.clone();
                tauri::async_runtime::spawn(async move {
                    let mut interval = tokio::time::interval(Duration::from_secs(10));
                    loop {
                        interval.tick().await;
                        let now = now_ms();
                        let origins = engine.lock().unwrap().tracked_repo_origins();
                        let slugs: Vec<(String, String)> = origins
                            .into_iter()
                            .filter_map(|(dir, origin_url)| {
                                let url = origin_url?;
                                let slug = tt_git::task_assign::repo_slug_from_remote(&url)?;
                                Some((dir, slug))
                            })
                            .collect();
                        repo_cache_store.reconcile_repos(&slugs, now);
                    }
                });
            }

            // Serve MCP over loopback HTTP. Bind-or-skip: whichever instance
            // takes the port serves every Claude Code session on the machine,
            // and the rest serve none — the OS bind is the mutex. Deliberately
            // after `manage(store_state)`, since a mutating call re-emits the
            // snapshot through that state.
            let mcp_port =
                tt_config::load().map(|s| s.mcp.port).unwrap_or(tt_config::DEFAULT_MCP_PORT);
            mcp_http::spawn(app.handle().clone(), mcp_port);

            // Overlap guard for the manual "refresh now" command.
            app.manage(store::CollectNowState::default());
            // Per-dir overlap guard for the rail's manual "Sync now" command.
            app.manage(store::RepoSyncState::default());

            // Collector scheduler: fills tt.db (PRs + issues via gh, calendar via
            // claude -p per settings.collectors) and re-emits the snapshot. The
            // shared signal lets `settings_set` make cadence edits take effect live.
            let scheduler_reload = Arc::new(Notify::new());
            // Slack Socket Mode: real-time DM delivery when an app-level token is
            // configured (no-op otherwise). Its own reload signal so a settings
            // write reliably reaches it alongside the scheduler.
            let slack_socket_reload = Arc::new(Notify::new());
            app.manage(settings::SettingsSignal {
                scheduler: scheduler_reload.clone(),
                slack_socket: slack_socket_reload.clone(),
            });
            scheduler::spawn(app.handle().clone(), scheduler_reload);
            slack_socket::spawn(app.handle().clone(), slack_socket_reload);

            // Clear IDE lockfiles left by towles-tool processes that died
            // without cleanup, so Claude Code never dials a dead server.
            ide::sweep_stale_lockfiles();

            // Keep the run marker fresh so a crash's estimated time stays
            // close to the real one (see `resume`).
            resume::spawn_heartbeat(app.handle().clone());

            // Kick an initial scan so the first snapshot has data.
            scan.notify_one();
            Ok(())
        })
        .manage(resume::ResumeState::begin())
        .manage(terminal::TermState::default())
        .manage(launch::LaunchState::default())
        .manage(lsp::Lsp::default())
        .manage(ide::DiffRequests::default())
        .manage(ide::ViewerWatches::default())
        .manage(resources::ResourceState::default())
        .manage(claude_sessions::ClaudeSessionsCache::default())
        .on_window_event(|window, event| match event {
            // Logged like focus_changed below, and for the same reason: an
            // orderly close otherwise leaves *no* record, making it
            // indistinguishable in the event log from a kill or crash (a
            // real triage dead-end: a dev-drive window that "vanished"
            // turned out to be a manual close, provable only by this gap).
            // CloseRequested says someone asked; Destroyed says the window
            // went down the orderly path.
            WindowEvent::CloseRequested { .. } => {
                tracing::info!(window = window.label(), "window.close_requested");
            }
            WindowEvent::Destroyed => {
                tracing::info!(window = window.label(), "window.destroyed");
                terminal::on_window_destroyed(window.app_handle(), window.label());
                // Reaching here at all means an orderly shutdown — a crash or
                // reboot never fires this, which is exactly what the next
                // launch reads the marker to find out.
                resume::on_window_destroyed(window.app_handle());
            }
            // The only record of the window's OS-level focus history — nothing
            // else logs this. Answers "did the app steal focus, and when?"
            // after the fact instead of needing to catch it live under a
            // debugger (see the worktree-delete-focus investigation).
            WindowEvent::Focused(focused) => {
                tracing::info!(focused = *focused, window = window.label(), "window.focus_changed");
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            app_task,
            ui_action,
            update::check_for_update,
            resources::app_resource_usage,
            agentboard::ab_get_state,
            agentboard::ab_mark_seen,
            agentboard::ab_dismiss_agent,
            agentboard::ab_reorder_session,
            agentboard::ab_set_theme,
            agentboard::ab_add_repo,
            agentboard::ab_remove_repo,
            agentboard::ab_untrack_missing,
            agentboard::ab_discover_repos,
            agentboard::ab_get_scan_roots,
            agentboard::ab_set_scan_roots,
            agentboard::ab_add_session,
            resume::ab_resume_candidates,
            agentboard::ab_rename_session,
            agentboard::ab_close_session,
            agentboard::ab_refresh,
            agentboard::ab_set_repo_meta,
            agentboard::ab_set_repo_order,
            agentboard::ab_set_folder_base_branch,
            agentboard::ab_set_folder_quiet,
            agentboard::ab_set_session_purpose,
            agentboard::ab_set_compact_percent,
            agentboard::ab_save_windows,
            agentboard::ab_save_collapsed,
            agentboard::ab_set_status,
            agentboard::ab_set_progress,
            agentboard::ab_log,
            agentboard::ab_clear_log,
            agentboard::ab_open_in_editor,
            agentboard::ab_get_diff,
            agentboard::ab_get_diff_files,
            agentboard::ab_get_base_file,
            agentboard::ab_get_commit_stats,
            launch::launch_configs,
            launch::launch_register,
            preview::preview_capture,
            preview::preview_write_feedback,
            task::task_base_branches,
            task::task_check_branch,
            task::task_create,
            task::task_delete,
            task::task_stop_port,
            task::task_run_setup,
            task::task_suggest,
            task::task_write_pasted_images,
            task::read_clipboard_image,
            store::store_snapshot,
            store::store_add_task,
            store::store_attach_task_issue,
            store::store_detach_task_issue,
            store::store_attach_task_pr,
            store::store_detach_task_pr,
            store::store_task_set_worktree,
            store::store_set_task_status,
            store::store_set_task_position,
            store::store_update_task,
            store::store_clear_done,
            store::store_promote_task_to_issue,
            store::store_create_issue,
            store::store_gh_issues_list,
            store::store_collect_now,
            store::store_sync_repo,
            gh_actions::cockpit_assign_issue,
            gh_actions::cockpit_create_issue_branch,
            store::store_dm_dismiss,
            mcp::mcp_tool_docs,
            mcp_http::mcp_status,
            mcp_http::mcp_test_call,
            slack::slack_dm_history,
            slack::slack_dm_send,
            slack::slack_dm_file,
            slack::slack_list_users,
            store::journal_log,
            claude_sessions::claude_sessions_summary,
            claude_sessions::claude_sessions_search,
            claude_sessions::claude_sessions_insights,
            claude_sessions::claude_sessions_breakdown,
            telemetry::telemetry_days,
            telemetry::telemetry_events,
            agentboard::ab_open_session_for_cwd,
            doctor::doctor_run,
            settings::settings_get,
            settings::settings_set,
            terminal::term_start,
            terminal::term_write,
            terminal::term_key,
            terminal::term_resize,
            terminal::term_scroll,
            terminal::term_wheel,
            terminal::term_mouse,
            terminal::term_request_full,
            terminal::term_visibility,
            terminal::term_select,
            terminal::term_copy,
            terminal::term_paste,
            terminal::term_paste_clipboard,
            terminal::term_search,
            terminal::term_scroll_to,
            terminal::term_clear,
            terminal::term_theme,
            terminal::term_focus,
            terminal::term_open_path,
            terminal::term_kill,
            ide::ide_set_selection,
            ide::ide_clear_selection,
            ide::ide_at_mention,
            ide::ide_status,
            ide::ide_list_files,
            ide::ide_set_open_file,
            ide::ide_set_diff_dirty,
            ide::ide_read_file,
            ide::ide_stat,
            ide::ide_read_dir,
            ide::ide_create_dir,
            ide::ide_delete,
            ide::ide_rename,
            lsp::lsp_start,
            lsp::lsp_send,
            lsp::lsp_stop,
            lsp::lsp_stop_all,
            ide::ide_write_file,
            ide::ide_watch_files,
            ide::ide_unwatch_files,
            ide::ide_diff_resolve,
            diagnostics::ide_diagnostics_refresh,
        ])
        .run(context)
        .expect("error while running Towles Tool application");
}
