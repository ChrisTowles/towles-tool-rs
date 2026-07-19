/**
 * The two repo mutations — track and untrack — as one seam both surfaces call.
 *
 * Kept out of {@link "./repo-manager"} on purpose: that module is pure so it
 * unit-tests without a DOM (see `apps/client/CLAUDE.md`), and these do IPC and
 * raise toasts. Kept out of the *screens* on purpose too: the Settings pane and
 * the rail's kebab both untrack, and when each owned a copy they drifted —
 * one checked the `Result` and one discarded it, one emitted its `ui.action`
 * event and the other was invisible in the event log, one suppressed
 * `NotInTauri` for browser dev and the other toasted a spurious error.
 */
import { toast } from "sonner";
import { NotInTauri } from "@/lib/errors";
import type { ScreenId } from "@/lib/screens";
import { invoke } from "@/lib/tauri";
import { uiAction } from "@/lib/ui-action";

/** Live session ids across a repo's checkouts — what {@link untrackRepo} must
 * close, and what a caller shows a count of when confirming. */
export function liveSessionIds(repo: {
  folders: { sessions: { id: string; live: boolean }[] }[];
}): string[] {
  return repo.folders.flatMap((f) => f.sessions.filter((s) => s.live).map((s) => s.id));
}

/**
 * Track `rawPath`. Returns whether it is tracked now, so a caller can avoid
 * announcing an add that didn't happen.
 *
 * `NotInTauri` is swallowed deliberately — plain-Vite browser dev has no host,
 * and that is not a failure worth a toast.
 */
export async function trackRepo(rawPath: string, screen: ScreenId): Promise<boolean> {
  const path = rawPath.trim();
  if (!path) return false;
  const added = await invoke("ab_add_repo", { path });
  if (added.isErr()) {
    if (!NotInTauri.is(added.error)) toast.error(`Couldn't track ${path} — ${added.error.message}`);
    return false;
  }
  uiAction("repo.tracked", screen);
  return true;
}

/**
 * Untrack `dir`, closing its live PTYs **first**.
 *
 * That ordering is the invariant this function exists to own: untrack first and
 * the sessions are orphaned — the repo leaves the rail and no UI can reach them
 * again. Callers confirm with the user beforehand whenever `sessionIds` is
 * non-empty, since closing them is destructive and has no undo.
 */
export async function untrackRepo(
  dir: string,
  name: string,
  sessionIds: readonly string[],
  screen: ScreenId,
): Promise<boolean> {
  for (const id of sessionIds) {
    await invoke("ab_close_session", { id });
  }
  const removed = await invoke("ab_remove_repo", { dir });
  if (removed.isErr()) {
    if (!NotInTauri.is(removed.error)) {
      toast.error(`Couldn't untrack ${name} — ${removed.error.message}`);
    }
    return false;
  }
  uiAction("repo.untracked", screen);
  return true;
}
