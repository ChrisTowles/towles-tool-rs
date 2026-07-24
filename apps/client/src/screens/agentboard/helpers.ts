import { toast } from "sonner";
import { invoke } from "@/lib/tauri";
import { NotInTauri } from "@/lib/errors";
import { storeAddTask, storeAttachTaskIssue, storeItemDismiss } from "@/lib/data";
import { uiAction } from "@/lib/ui-action";
import type { PaneRect } from "@/lib/agentboard";
import type { NewTaskSubmit } from "@/components/inline-new-task";

/** Sentinel key in the persisted collapse map for "the whole rail is collapsed
 * to icons" — rides the same `ab_save_collapsed` store as the per-row keys
 * (`repo:<name>` / `<repoKey>::<dir>`), which it can never collide with. */
export const RAIL_COLLAPSE_KEY = "__rail__";

/** `onOpenChange` for a dialog whose only close-side effect is clearing
 * whatever state made it open — Radix fires `false` on outside-click, Esc,
 * and the built-in close button alike, so this covers all three at once. */
export const closeOnFalse = (fn: () => void) => (isOpen: boolean) => {
  if (!isOpen) fn();
};

/** Untrack every repo whose directory is gone from disk, reporting the count.
 * The Rust side re-probes at call time, so a directory restored since the last
 * poll survives. */
export async function cleanupMissing() {
  const removed = await invoke<string[]>("ab_untrack_missing", {});
  if (removed.isErr()) {
    toast.error(`Couldn't clean up — ${removed.error.message}`);
    return;
  }
  const n = removed.value.length;
  toast(n > 0 ? `Untracked ${n} missing repo${n === 1 ? "" : "s"}.` : "Nothing to clean up.");
}

/** Dismiss one PR out of the rail's attention strip: it drops out until it
 * changes again (see isItemDismissed). The snapshot re-emits from Rust on
 * success, so no optimistic update here. */
export async function dismissAttentionPr(repo: string, number: number, updatedTs: number) {
  uiAction("agentboard.attention_pr_dismiss", "agentboard");
  const result = await storeItemDismiss("pr", repo, number, updatedTs);
  if (result.isErr() && !NotInTauri.is(result.error)) toast.error(result.error.message);
}

/** A pane's grid rect as absolute-positioning percentages. */
export const paneStyle = (r: PaneRect) => ({
  left: `${r.left}%`,
  top: `${r.top}%`,
  width: `${r.width}%`,
  height: `${r.height}%`,
});

/**
 * Create the board task for a new-task submit (#339): the task row exists
 * from the moment of submit — before any worktree work — with the picked
 * issues attached. Best-effort: a store failure must not block the worktree
 * (the task is still useful without a card), so this resolves to `undefined`
 * on error after surfacing a toast.
 */
export async function createTaskForSubmit(input: NewTaskSubmit): Promise<number | undefined> {
  const title = input.title || input.goal || input.issues[0]?.title || input.branch;
  if (!title) return undefined;
  const status = input.worktree ? "doing" : "backlog";
  const created = await storeAddTask(title, { status, goal: input.goal || undefined });
  if (created.isErr()) {
    if (!NotInTauri.is(created.error)) {
      toast(`couldn't add the board task: ${created.error.message}`);
    }
    return undefined;
  }
  for (const issue of input.issues) {
    void storeAttachTaskIssue(created.value, issue.repo, issue.number, issue.url);
  }
  return created.value;
}
