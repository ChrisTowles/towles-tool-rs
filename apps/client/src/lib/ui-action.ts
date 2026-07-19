import { invoke } from "@/lib/tauri";

/** Record a user gesture in the OTel event log (the root CLAUDE.md's
 * "every user action must be logged" rule). One shared seam: crosses IPC to
 * the `ui_action` Tauri command, which emits a `ui.action` tracing event with
 * a stable action id and the screen it happened on. Fire-and-forget by
 * design — telemetry must never change the gesture's own behavior, and an
 * ignored `Result` can't produce an unhandled rejection (in browser dev the
 * call is a `NotInTauri` no-op). Discrete intents only: no content, no
 * continuous input. */
export function uiAction(action: string, screen: string): void {
  void invoke("ui_action", { action, screen });
}
