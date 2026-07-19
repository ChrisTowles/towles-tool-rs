//! Self-registers a `.desktop` entry + icon per slot so each checkout shows up
//! as its own entry in the GNOME/COSMIC app launcher/search, with the right
//! icon and an `Exec` pointing at that slot's own binary. `tauri build`
//! normally writes both as part of its Linux packaging step (deb/rpm/
//! AppImage), but the daily-driver flow (`npm start`) runs `tauri build
//! --no-bundle` and execs the raw binary — packaging never happens, so
//! nothing installs them. Idempotent: only touches disk when the content
//! actually differs, so every slot's binary can call this on startup without
//! fighting over the file.
//!
//! `StartupWMClass` is deliberately the constant binary name (`tt-app`), not
//! `app_id`: `enableGTKAppId` is off (see `tauri.conf.json`'s history and
//! `lib.rs`'s `app_identifier` doc — a real GTK/D-Bus app-id made every
//! worktree slot's window a D-Bus-activatable singleton, and any activation
//! — a dock click, `gio launch`, systemd — crashed the already-running
//! process re-entering Tauri's internal setup()), so every running window's
//! actual WM_CLASS is GTK's default (the binary's prgname). Matching
//! `app_id` here would never resolve; this way the dock/taskbar can still
//! find *an* icon for the running window, just not a per-slot one.

use std::path::Path;

const ICON_BYTES: &[u8] = include_bytes!("../icons/icon.png");
const WM_CLASS: &str = "tt-app";

pub fn ensure_installed(app_id: &str) {
    let Some(data_home) = dirs::data_local_dir() else {
        return;
    };

    let icon_path = data_home.join("icons/hicolor/512x512/apps").join(format!("{app_id}.png"));
    write_if_changed(&icon_path, ICON_BYTES);

    let desktop_path = data_home.join("applications").join(format!("{app_id}.desktop"));
    let entry = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Towles Tool\n\
         Comment=Personal dev tools: agentboard, PRs\n\
         Exec={}\n\
         Icon={app_id}\n\
         Terminal=false\n\
         Categories=Development;Utility;\n\
         StartupWMClass={WM_CLASS}\n",
        std::env::current_exe().unwrap_or_default().display(),
    );
    write_if_changed(&desktop_path, entry.as_bytes());
}

fn write_if_changed(path: &Path, contents: &[u8]) {
    if std::fs::read(path).is_ok_and(|existing| existing == contents) {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, contents);
}
