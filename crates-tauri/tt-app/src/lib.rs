//! Towles Tool desktop app (Tauri 2). Hosts the agentboard bridge: an engine
//! (tracker/metadata/order/git/watcher) driven by tokio tasks that emits state
//! snapshots as the `agentboard://state` event and exposes client commands.
//! Also owns the embedded terminals (`terminal`): PTYs the app spawns,
//! persisted across app restarts by a shpool daemon when available
//! (`shpool`), rendered by ghostty-web in the agentboard screen.

mod agentboard;
mod graph;
mod journal;
mod scheduler;
mod settings;
mod shpool;
mod store;
mod terminal;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::{Emitter, Manager, WindowEvent};
use tokio::sync::Notify;

use agentboard::{Ab, Engine, STATE_EVENT, now_ms};
use tt_agentboard::fs_notify::DirNotifier;

/// Human-readable name of the checkout this binary was built from — the repo-root
/// directory (e.g. `towles-tool-rs-slot-2`). Baked in at compile time from
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
    let builder = tauri::Builder::default().plugin(tauri_plugin_opener::init());

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

            let engine = Arc::new(Mutex::new(Engine::new()));
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
            });

            let handle = app.handle().clone();

            // Debounced emitter: coalesce a burst of triggers into one rebuild + emit.
            {
                let emit = emit.clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        emit.notified().await;
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        let payload = agentboard::stamped_payload(&handle);
                        let _ = handle.emit(STATE_EVENT, payload);
                    }
                });
            }

            // Watcher scan: every 2s, or eagerly on an fs-notify signal.
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
                        {
                            let mut e = engine.lock().unwrap();
                            e.scan_once(now_ms());
                        }
                        emit.notify_one();
                    }
                });
            }

            // Git-stat poll: refresh working-tree stats every 1.5s.
            {
                let engine = engine.clone();
                let emit = emit.clone();
                tauri::async_runtime::spawn(async move {
                    let mut interval = tokio::time::interval(Duration::from_millis(1500));
                    loop {
                        interval.tick().await;
                        {
                            let mut e = engine.lock().unwrap();
                            e.refresh_git(now_ms());
                        }
                        emit.notify_one();
                    }
                });
            }

            // Personal-dashboard store + journal logging. Open the store once; if it
            // fails, the app still runs and store commands return an error.
            let store_state = store::StoreState::open();
            // Emit the initial store snapshot for the dashboard's first mount.
            store::emit_snapshot(&app.handle().clone(), &store_state);
            app.manage(store_state);

            // Collector scheduler: fills tt.db (PRs + issues via gh, calendar via
            // claude -p per settings.collectors) and re-emits the snapshot. The
            // shared signal lets `settings_set` make cadence edits take effect live.
            let settings_reload = Arc::new(Notify::new());
            app.manage(settings::SettingsSignal(settings_reload.clone()));
            scheduler::spawn(app.handle().clone(), settings_reload);

            // Kick an initial scan so the first snapshot has data.
            scan.notify_one();
            Ok(())
        })
        .manage(terminal::TermState::default())
        .on_window_event(|window, event| match event {
            // With live shells that shpool can keep alive, closing needs an
            // answer first (keep detached vs kill); the frontend dialog
            // resolves via `app_close`, which destroys the window for real.
            WindowEvent::CloseRequested { api, .. } => {
                if terminal::ask_before_close(window.app_handle(), window.label()) {
                    api.prevent_close();
                }
            }
            WindowEvent::Destroyed => {
                terminal::on_window_destroyed(window.app_handle(), window.label());
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            app_slot,
            agentboard::ab_get_state,
            agentboard::ab_mark_seen,
            agentboard::ab_dismiss_agent,
            agentboard::ab_reorder_session,
            agentboard::ab_set_theme,
            agentboard::ab_add_repo,
            agentboard::ab_remove_repo,
            agentboard::ab_discover_repos,
            agentboard::ab_get_scan_roots,
            agentboard::ab_set_scan_roots,
            agentboard::ab_add_session,
            agentboard::ab_rename_session,
            agentboard::ab_close_session,
            agentboard::ab_refresh,
            agentboard::ab_set_folder_purpose,
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
            store::store_snapshot,
            store::store_add_task,
            store::store_set_task_status,
            store::store_promote_task_to_issue,
            store::store_create_issue,
            store::journal_log,
            journal::journal_get_today,
            journal::journal_list,
            journal::journal_search,
            journal::journal_create,
            journal::journal_open,
            graph::graph_spend_summary,
            settings::settings_get,
            settings::settings_set,
            terminal::term_start,
            terminal::term_write,
            terminal::term_resize,
            terminal::term_kill,
            terminal::app_close,
            shpool::shpool_status,
            shpool::shpool_install,
            shpool::shpool_sessions,
            shpool::shpool_kill_session,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Towles Tool application");
}
