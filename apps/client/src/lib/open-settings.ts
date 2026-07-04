import { isTauri } from "@tauri-apps/api/core";

const SETTINGS_LABEL = "settings";

// Open Settings as its own OS window rather than an in-app modal. In the Tauri
// shell this is a real WebviewWindow (focus it if already open); in plain-Vite
// browser dev it falls back to a native popup window pointed at the same entry.
export async function openSettings() {
  if (isTauri()) {
    const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
    const existing = await WebviewWindow.getByLabel(SETTINGS_LABEL);
    if (existing) {
      await existing.setFocus();
      return;
    }
    const win = new WebviewWindow(SETTINGS_LABEL, {
      url: "settings.html",
      title: "Settings — Towles Tool",
      width: 720,
      height: 560,
      minWidth: 600,
      minHeight: 440,
      resizable: true,
    });
    win.once("tauri://error", (e) => console.error("failed to open settings window", e));
    return;
  }

  window.open("/settings.html", SETTINGS_LABEL, "width=720,height=560");
}

// Close the current Settings window from within it (native OS chrome also works).
export async function closeCurrentWindow() {
  if (isTauri()) {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    await getCurrentWindow().close();
    return;
  }
  window.close();
}
