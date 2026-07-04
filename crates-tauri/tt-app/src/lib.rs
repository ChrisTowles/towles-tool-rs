//! Towles Tool desktop app (Tauri 2). Hosts the agentboard bridge: an engine
//! (tracker/metadata/order/git/watcher) driven by tokio tasks that emits state
//! snapshots as the `agentboard://state` event and exposes client commands.
//! Also owns the embedded terminals (`terminal`): PTYs the app spawns directly
//! (not tmux), rendered by xterm.js in the agentboard screen.

mod agentboard;
mod scheduler;
mod store;
mod terminal;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::{Emitter, Manager, WindowEvent};
use tokio::sync::Notify;

use agentboard::{Ab, Engine, STATE_EVENT, now_ms};
use tt_agentboard::fs_notify::DirNotifier;

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
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
                let engine = engine.clone();
                let emit = emit.clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        emit.notified().await;
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        let payload = {
                            let mut e = engine.lock().unwrap();
                            e.compute_payload(now_ms())
                        };
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

            // Localhost metadata HTTP ingest (external agents/scripts POST here).
            {
                let engine = engine.clone();
                let emit = emit.clone();
                let (host, port) = agentboard::ingest_addr();
                tauri::async_runtime::spawn(agentboard::serve_metadata(engine, emit, host, port));
            }

            // Personal-dashboard store + journal logging. Open the store once; if it
            // fails, the app still runs and store commands return an error.
            let store_state = store::StoreState::open();
            // Emit the initial store snapshot for the dashboard's first mount.
            store::emit_snapshot(&app.handle().clone(), &store_state);
            app.manage(store_state);

            // Collector scheduler: fills tt.db (PRs via gh, calendar/email/tasks
            // via claude -p per settings.assistant) and re-emits the snapshot.
            scheduler::spawn(app.handle().clone());

            // Kick an initial scan so the first snapshot has data.
            scan.notify_one();
            Ok(())
        })
        .manage(terminal::TermState::default())
        .on_window_event(|window, event| {
            if let WindowEvent::Destroyed = event {
                terminal::on_window_destroyed(window.app_handle(), window.label());
            }
        })
        .invoke_handler(tauri::generate_handler![
            agentboard::ab_get_state,
            agentboard::ab_mark_seen,
            agentboard::ab_dismiss_agent,
            agentboard::ab_reorder_session,
            agentboard::ab_set_theme,
            agentboard::ab_add_repo,
            agentboard::ab_remove_repo,
            agentboard::ab_refresh,
            agentboard::ab_set_status,
            agentboard::ab_set_progress,
            agentboard::ab_log,
            agentboard::ab_clear_log,
            agentboard::ab_open_in_editor,
            store::store_snapshot,
            store::store_add_task,
            store::store_set_task_done,
            store::store_archive_email,
            store::journal_log,
            terminal::term_start,
            terminal::term_write,
            terminal::term_resize,
            terminal::term_kill,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Towles Tool application");
}
