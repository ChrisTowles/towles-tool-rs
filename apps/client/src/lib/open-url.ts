import { Result } from "better-result";
import { IpcFailed, type IpcError } from "@/lib/errors";
import { isTauri } from "@/lib/tauri";

/**
 * Open a URL (GitHub PRs/issues, etc.) in the OS default browser. In the Tauri
 * shell a bare `window.open`/`<a target="_blank">` either no-ops or opens an
 * in-app webview window rather than the system browser, so this routes through
 * the opener plugin; in plain-Vite browser dev it falls back to `window.open`,
 * which already does the right thing and so always succeeds.
 */
export async function openExternalUrl(url: string): Promise<Result<void, IpcError>> {
  if (!isTauri()) {
    window.open(url, "_blank", "noopener");
    return Result.ok(undefined);
  }
  return Result.tryPromise({
    try: async () => {
      const { openUrl } = await import("@tauri-apps/plugin-opener");
      await openUrl(url);
    },
    catch: (cause): IpcError => new IpcFailed({ command: "opener.openUrl", cause }),
  });
}
