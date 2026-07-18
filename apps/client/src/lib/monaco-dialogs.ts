/**
 * Dialog service for the VS Code layer, backed by the app's own UI.
 *
 * The hazard this replaces: without an override, `IDialogService` resolves to
 * VS Code's `StandaloneDialogService`, whose `confirm` is a literal
 * `window.confirm()`. That is a blocking native script dialog — inside the
 * Tauri WebView it spins a nested GTK main loop while the app is mid-JS and
 * dispatching sync IPC on that same thread, and the window wedges. **Nothing
 * in this file may ever call `window.confirm`/`alert`/`prompt`**, however
 * convenient; that is the whole point of it existing.
 *
 * The service is constructed by the VS Code service layer, well outside React,
 * so it publishes pending requests through a store (the same `get`/`subscribe`
 * shape as `lib/focus-target.ts`) and `<MonacoDialogHost>` renders them. Each
 * request carries the `resolve` of the promise the workbench is awaiting, so a
 * button click is what answers VS Code.
 */

import { IDialogService } from "@codingame/monaco-vscode-api";
import { Event } from "@codingame/monaco-vscode-api/vscode/vs/base/common/event";
import { deleteCopyForTrash, isDangerous, stripMnemonic } from "@/lib/monaco-dialog-copy";
import { dialogStore } from "@/lib/monaco-dialog-store";

class AppDialogService {
  readonly onWillShowDialog = Event.None;
  readonly onDidShowDialog = Event.None;

  async confirm(confirmation: { message?: string; detail?: string; primaryButton?: string }) {
    const copy = deleteCopyForTrash(
      confirmation?.message ?? "Are you sure?",
      confirmation?.detail,
    );
    const primary = stripMnemonic(confirmation?.primaryButton ?? "OK");
    const confirmed = await dialogStore.ask({
      message: copy.message,
      detail: copy.detail,
      primary,
      danger: isDangerous(primary, copy.message),
    });
    return { confirmed, checkboxChecked: false };
  }

  /**
   * A prompt is a confirm plus a set of buttons; VS Code runs the chosen
   * button's `run()` to produce the result. We offer only the first
   * (primary) button, so this is "do it / cancel" — enough for the
   * confirmations the workbench actually raises here.
   */
  async prompt(prompt: {
    message?: string;
    detail?: string;
    buttons?: { label?: string; run?: (ctx: { checkboxChecked: boolean }) => unknown }[];
    cancelButton?: unknown;
  }) {
    const first = prompt?.buttons?.[0];
    const message = prompt?.message ?? "Are you sure?";
    const primary = stripMnemonic(first?.label ?? "OK");
    const confirmed = await dialogStore.ask({
      message,
      detail: prompt?.detail,
      primary,
      danger: isDangerous(primary, message),
    });
    if (!confirmed) return { result: undefined };
    return { result: await first?.run?.({ checkboxChecked: false }) };
  }

  // Notifications, not questions — the workbench has nowhere to render them
  // here, so they go to the console rather than interrupting.
  async info(message: string, detail?: string) {
    console.info("[monaco]", message, detail ?? "");
  }

  async warn(message: string, detail?: string) {
    console.warn("[monaco]", message, detail ?? "");
  }

  async error(message: string, detail?: string) {
    console.error("[monaco]", message, detail ?? "");
  }

  /** Text input (e.g. a rename prompt outside the tree's inline editor) has
   * no host yet; decline rather than invent a value. */
  async input() {
    console.warn("[monaco] declined an input dialog — no host for it");
    return { confirmed: false, values: undefined };
  }

  async about() {}
}

/** Service map entry for `api.initialize`. */
export default function getServiceOverride(): Record<string, unknown> {
  return { [IDialogService.toString()]: new AppDialogService() };
}
