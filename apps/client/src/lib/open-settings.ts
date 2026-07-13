import { isTauri } from "@tauri-apps/api/core";

const SETTINGS_LABEL = "settings";

/** Deep-link the Settings window opens onto: a tab id and/or a prefilled filter
 * (e.g. `{ tab: "collectors", filter: "slack" }` lands on the Slack rows). */
export type SettingsTarget = { tab?: string; filter?: string };

function targetQuery(target?: SettingsTarget): string {
  if (!target) return "";
  const params = new URLSearchParams();
  if (target.tab) params.set("tab", target.tab);
  if (target.filter) params.set("filter", target.filter);
  const query = params.toString();
  return query ? `?${query}` : "";
}

// Open Settings as its own OS window rather than an in-app modal. In the Tauri
// shell this is a real WebviewWindow (focus it if already open); in plain-Vite
// browser dev it falls back to a native popup window pointed at the same entry.
// An optional `target` deep-links onto a tab / prefilled filter, but only when
// the window is opened fresh — an already-open window is just refocused.
export async function openSettings(target?: SettingsTarget) {
  const query = targetQuery(target);
  if (isTauri()) {
    const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
    const existing = await WebviewWindow.getByLabel(SETTINGS_LABEL);
    if (existing) {
      await existing.setFocus();
      return;
    }
    const win = new WebviewWindow(SETTINGS_LABEL, {
      url: `settings.html${query}`,
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

  window.open(`/settings.html${query}`, SETTINGS_LABEL, "width=720,height=560");
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
