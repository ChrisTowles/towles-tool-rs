//! Self-registers a `.desktop` entry + icon so GNOME/COSMIC (Wayland) can
//! resolve the running window's app-id to the real icon instead of falling
//! back to a generic placeholder. `tauri build` normally writes both as part
//! of its Linux packaging step (deb/rpm/AppImage), but the daily-driver flow
//! (`npm run run`) runs `tauri build --no-bundle` and execs the raw binary —
//! packaging never happens, so nothing installs them. Idempotent: only
//! touches disk when the content actually differs, so every slot's binary
//! can call this on startup without fighting over the file.

use std::path::Path;

const ICON_BYTES: &[u8] = include_bytes!("../icons/icon.png");

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
         Comment=Personal dev tools: agentboard, PRs, journal\n\
         Exec={}\n\
         Icon={app_id}\n\
         Terminal=false\n\
         Categories=Development;Utility;\n\
         StartupWMClass={app_id}\n",
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
