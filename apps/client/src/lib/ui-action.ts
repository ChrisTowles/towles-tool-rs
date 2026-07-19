import { invoke } from "@/lib/tauri";
import type { ScreenId } from "@/lib/screens";

/** Record one user gesture in the on-disk event log — the frontend half of
 * the root CLAUDE.md's "every user action emits its OTel event" doctrine,
 * crossing IPC through the single `ui_action` command (`tt-app/src/lib.rs`)
 * rather than per-feature plumbing.
 *
 * Discrete intents only: a stable dot-separated action id
 * (`preview.feedback.send`, `task.start`) plus the screen — never content,
 * keystrokes, or continuous input. `detail` is for a word of context (an
 * outcome, a count), not payloads. Fire-and-forget by design: `invoke` can't
 * throw, and a lost record must never block the gesture it describes (in
 * browser dev the call is a `NotInTauri` no-op). */
export function uiAction(action: string, screen: ScreenId, detail?: string): void {
  void invoke<void>("ui_action", { action, screen, detail: detail ?? null });
}
