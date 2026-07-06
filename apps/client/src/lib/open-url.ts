import { isTauri } from "@tauri-apps/api/core";

// Open a URL (GitHub PRs/issues, etc.) in the OS default browser. In the Tauri
// shell a bare `window.open`/`<a target="_blank">` either no-ops or opens an
// in-app webview window rather than the system browser, so this routes
// through the opener plugin; in plain-Vite browser dev it falls back to
// window.open, which already does the right thing.
export async function openExternalUrl(url: string) {
  if (isTauri()) {
    const { openUrl } = await import("@tauri-apps/plugin-opener");
    await openUrl(url);
    return;
  }
  window.open(url, "_blank", "noopener");
}
