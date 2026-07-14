//! Towles Tool desktop app (Tauri 2). Hosts the agentboard bridge: an engine
//! (tracker/metadata/order/git/watcher) driven by tokio tasks that emits state
//! snapshots as the `agentboard://state` event and exposes client commands.
//! Also owns the embedded terminals (`terminal`): PTYs the app spawns and
//! kills on window close, rendered by xterm.js in the agentboard screen.

mod agentboard;
mod claude_sessions;
mod doctor;
mod gh_actions;
mod instance_lock;
mod journal;
mod resources;
mod scheduler;
mod settings;
mod slack;
mod slack_socket;
mod slots;
mod store;
mod terminal;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::{Emitter, Manager, WindowEvent};
use tokio::sync::Notify;

use agentboard::{Ab, Engine, STATE_EVENT, now_ms};
use tt_agentboard::fs_notify::DirNotifier;

/// Human-readable name of the checkout this binary was built from — the repo-root
/// directory (e.g. `slot-migrate`, `towles-tool-rs-primary`). Baked in at compile time from
/// `CARGO_MANIFEST_DIR` (`<root>/crates-tauri/tt-app`), so each slot's binary
/// knows its own slot without any runtime cwd/env plumbing. Lets several slots'
/// windows be told apart in the title bar, taskbar, and app header.
pub(crate) fn slot_label() -> String {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("towles-tool")
        .to_string()
}

/// Slot name for the frontend header badge (see `slot_label`).
#[tauri::command]
fn app_slot() -> String {
    slot_label()
}

pub fn run() {
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

            // Distinguish concurrent slot windows in the title bar / taskbar.
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.set_title(&format!("Towles Tool — {}", slot_label()));
            }

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

            // fs-notify accelerant: any journal change signals an eager scan.
            let projects_dir = engine.lock().unwrap().projects_dir();
            let scan_for_notify = scan.clone();
            let notifier =
                DirNotifier::watch(&projects_dir, move || scan_for_notify.notify_one()).ok();

            app.manage(Ab {
                engine: engine.clone(),
                emit: emit.clone(),
                scan: scan.clone(),
                _notifier: Mutex::new(notifier),
                needs_since: Mutex::new(tt_agentboard::bridge::NeedsSince::new()),
            });

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
            // signal. Warms any stale git-cache entries (e.g. a worktree slot
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
                tauri::async_runtime::spawn(async move {
                    let mut interval = tokio::time::interval(Duration::from_millis(2000));
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
                                    .map(|(dir, base_branch)| {
                                        let info = tt_agentboard::git_info::compute_git_info(
                                            &dir,
                                            base_branch.as_deref(),
                                        );
                                        (dir, info)
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .await
                            .unwrap_or_default();
                            engine.lock().unwrap().warm_git_cache(warmed, now);
                        }
                        {
                            let mut e = engine.lock().unwrap();
                            e.scan_once(now);
                        }
                        emit.notify_one();
                    }
                });
            }

            // Git-stat poll: recompute working-tree stats every 10s with the
            // subprocesses OUTSIDE the engine lock — a slow or hung git (stale
            // network mount, cold cache) must never wedge the ab_* commands
            // that share the lock, and lock-held git spawns every 1.5s were
            // the dominant idle-CPU cost. Signals a re-emit only when some
            // repo's stats actually changed.
            {
                let engine = engine.clone();
                let emit = emit.clone();
                tauri::async_runtime::spawn(async move {
                    let mut interval = tokio::time::interval(Duration::from_secs(10));
                    loop {
                        interval.tick().await;
                        let poll_engine = engine.clone();
                        let changed = tauri::async_runtime::spawn_blocking(move || {
                            let targets = poll_engine.lock().unwrap().git_targets();
                            let mut changed = false;
                            for (dir, base_branch) in targets {
                                let info = tt_agentboard::git_info::compute_git_info(
                                    &dir,
                                    base_branch.as_deref(),
                                );
                                let stored = poll_engine.lock().unwrap().store_git_info(
                                    &dir,
                                    info,
                                    now_ms(),
                                );
                                changed |= stored;
                            }
                            changed
                        })
                        .await
                        .unwrap_or(false);
                        if changed {
                            emit.notify_one();
                        }
                    }
                });
            }

            // Background fetch: `git fetch origin` every 3 minutes per tracked
            // repo (deduped across worktrees/slots), outside the engine lock
            // like the stat poll above. The 10s git-stat poll only reads
            // already-cached remote-tracking refs, so without this,
            // "commits behind main" never updates until the user happens to
            // fetch some other way (opening a terminal, `tt slot create`,
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
                                .map(|(dir, _)| dir)
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
            app.manage(store_state);
            // Overlap guard for the manual "refresh now" command.
            app.manage(store::CollectNowState::default());

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

            // Kick an initial scan so the first snapshot has data.
            scan.notify_one();
            Ok(())
        })
        .manage(terminal::TermState::default())
        .manage(resources::ResourceState::default())
        .manage(claude_sessions::ClaudeSessionsCache::default())
        .on_window_event(|window, event| {
            if let WindowEvent::Destroyed = event {
                terminal::on_window_destroyed(window.app_handle(), window.label());
            }
        })
        .invoke_handler(tauri::generate_handler![
            app_slot,
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
            agentboard::ab_rename_session,
            agentboard::ab_close_session,
            agentboard::ab_refresh,
            agentboard::ab_set_folder_purpose,
            agentboard::ab_set_folder_base_branch,
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
            slots::slot_base_branches,
            slots::slot_init_template,
            slots::slot_check_branch,
            slots::slot_create,
            slots::slot_remove,
            slots::slot_run_setup,
            slots::slot_suggest,
            store::store_snapshot,
            store::store_add_task,
            store::store_set_task_status,
            store::store_set_task_position,
            store::store_update_task,
            store::store_delete_task,
            store::store_clear_done,
            store::store_promote_task_to_issue,
            store::store_create_issue,
            store::store_gh_tracked_repos,
            store::store_gh_issues_list,
            store::store_gh_milestones_list,
            store::store_import_issues,
            store::store_collect_now,
            gh_actions::cockpit_assign_issue,
            gh_actions::cockpit_create_issue_branch,
            store::store_dm_dismiss,
            slack::slack_dm_history,
            slack::slack_dm_send,
            slack::slack_dm_file,
            slack::slack_list_users,
            store::journal_log,
            journal::journal_get_today,
            journal::journal_save,
            journal::journal_list,
            journal::journal_search,
            journal::journal_create,
            journal::journal_open,
            claude_sessions::claude_sessions_summary,
            claude_sessions::claude_sessions_search,
            agentboard::ab_open_session_for_cwd,
            doctor::doctor_run,
            settings::settings_get,
            settings::settings_set,
            terminal::term_start,
            terminal::term_write,
            terminal::term_resize,
            terminal::term_scroll,
            terminal::term_wheel,
            terminal::term_request_full,
            terminal::term_visibility,
            terminal::term_select,
            terminal::term_copy,
            terminal::term_search,
            terminal::term_scroll_to,
            terminal::term_clear,
            terminal::term_focus,
            terminal::term_open_path,
            terminal::term_kill,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Towles Tool application");
}
