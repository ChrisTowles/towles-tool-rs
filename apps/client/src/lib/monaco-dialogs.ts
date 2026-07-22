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

import { toast } from "sonner";
import { IDialogService } from "@codingame/monaco-vscode-api";
import { Event } from "@codingame/monaco-vscode-api/vscode/vs/base/common/event";
import { deleteCopyForTrash, isDangerous, stripMnemonic } from "@/lib/monaco-dialog-copy";
import { dialogStore } from "@/lib/monaco-dialog-store";

class AppDialogService {
  readonly onWillShowDialog = Event.None;
  readonly onDidShowDialog = Event.None;

  async confirm(confirmation: { message?: string; detail?: string; primaryButton?: string }) {
    const { message, detail } = deleteCopyForTrash(
      confirmation?.message ?? "Are you sure?",
      confirmation?.detail,
    );
    const primary = stripMnemonic(confirmation?.primaryButton ?? "OK");
    const confirmed = await dialogStore.ask({
      message,
      detail,
      primary,
      danger: isDangerous(primary, message),
    });
    return { confirmed, checkboxChecked: false };
  }

  /**
   * A prompt is a confirm plus a set of buttons, where VS Code runs the
   * chosen button's `run()` to produce the result. Only the first (primary)
   * button is offered, so this is "do it / cancel" — enough for what the
   * workbench raises here, and it keeps one ask-protocol.
   */
  async prompt(prompt: {
    message?: string;
    detail?: string;
    buttons?: { label?: string; run?: (ctx: { checkboxChecked: boolean }) => unknown }[];
    cancelButton?: unknown;
  }) {
    const first = prompt?.buttons?.[0];
    const { confirmed } = await this.confirm({
      message: prompt?.message,
      detail: prompt?.detail,
      primaryButton: first?.label,
    });
    return { result: confirmed ? await first?.run?.({ checkboxChecked: false }) : undefined };
  }

  // Not questions — these report an outcome. They reach the user as toasts
  // because this is the workbench's only channel for them: a failed Explorer
  // rename or delete surfaces here, and console-only meant the user saw a
  // silent no-op after confirming.
  async info(message: string, detail?: string) {
    this.notify("info", message, detail);
  }

  async warn(message: string, detail?: string) {
    this.notify("warning", message, detail);
  }

  async error(message: string, detail?: string) {
    this.notify("error", message, detail);
  }

  private notify(level: "info" | "warning" | "error", message: string, detail?: string) {
    toast[level](detail ? `${message} — ${detail}` : message);
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
