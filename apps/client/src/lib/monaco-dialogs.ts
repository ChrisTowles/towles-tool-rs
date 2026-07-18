/**
 * Dialog service override for the VS Code layer.
 *
 * Without this, `IDialogService` resolves to VS Code's `StandaloneDialogService`,
 * whose `confirm` is a literal `window.confirm()`. That is a blocking native
 * script dialog: inside the Tauri WebView it spins a nested GTK main loop while
 * the app is mid-JS and dispatching sync IPC on that same thread, and the
 * window wedges. Nothing else in this app has ever called one.
 *
 * `lib/monaco-prune.ts` shadows the two commands known to reach it. This is the
 * backstop for the ones we haven't found: every prompt auto-declines, so an
 * unexpected caller loses its dialog instead of the user losing the window.
 */

import { IDialogService } from "@codingame/monaco-vscode-api";
import { Event } from "@codingame/monaco-vscode-api/vscode/vs/base/common/event";

/** Declines everything. Mirrors StandaloneDialogService's shape minus the
 * native calls — `prompt` runs no button, so callers take their cancel path. */
class DecliningDialogService {
  readonly onWillShowDialog = Event.None;
  readonly onDidShowDialog = Event.None;

  private declined(what: string, message: unknown): void {
    console.warn(`[monaco] auto-declined a ${what} dialog:`, message);
  }

  async confirm(confirmation: { message?: string }) {
    this.declined("confirm", confirmation?.message);
    return { confirmed: false, checkboxChecked: false };
  }

  async prompt(prompt: { message?: string }) {
    this.declined("prompt", prompt?.message);
    return { result: undefined };
  }

  async info(message: string) {
    this.declined("info", message);
  }

  async warn(message: string) {
    this.declined("warn", message);
  }

  async error(message: string) {
    this.declined("error", message);
  }

  async input() {
    this.declined("input", undefined);
    return { confirmed: false, values: undefined };
  }

  async about() {}
}

/** Service map entry for `api.initialize`. */
export default function getServiceOverride(): Record<string, unknown> {
  return { [IDialogService.toString()]: new DecliningDialogService() };
}
