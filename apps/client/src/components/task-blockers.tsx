import type { ReactNode } from "react";
import { CircleAlert, FileDiff, GitCommitHorizontal, Network } from "lucide-react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import {
  forceDeleteLabel,
  type TaskBlocker,
  type TaskBlockerKind,
  stoppablePort,
} from "@/lib/agentboard";
import { cn } from "@/lib/utils";

/** The dialog a refused `task_delete` opens, shared by the two screens that can
 * trigger one — the Agentboard rail's "Delete worktree…" and the Board's card
 * delete. Presentation only; each screen owns its own delete flow state. */

/** Glyph per blocker kind. Exhaustive over `TaskBlockerKind`, so a guard added
 * in Rust fails the build here rather than silently picking up whichever icon a
 * ternary happened to end on. */
const BLOCKER_ICONS: Record<TaskBlockerKind, typeof CircleAlert> = {
  dirtyTree: FileDiff,
  unreachableCommits: GitCommitHorizontal,
  foreignPort: Network,
};

/** Tinted by consequence rather than by kind: the one thing worth seeing at a
 * glance is which rows are work that disappears if forced (destructive) and
 * which are merely in the way (muted) — the row's own text says what it is.
 *
 * An unrecognized kind (an older frontend meeting a newer backend across the
 * IPC boundary) gets a neutral alert glyph. The row still reads fine — its
 * message and remedy come from Rust — and admitting we don't know beats
 * asserting the wrong thing. */
function BlockerIcon({ kind, losesWork }: { kind: string; losesWork: boolean }) {
  const Icon = BLOCKER_ICONS[kind as TaskBlockerKind] ?? CircleAlert;
  return (
    <Icon
      className={cn(
        "mt-0.5 size-4 shrink-0",
        losesWork ? "text-destructive" : "text-muted-foreground",
      )}
      aria-hidden
    />
  );
}

export function BlockedDeleteDialog({
  open,
  onOpenChange,
  /** What's being deleted, for the title. */
  name,
  /** One sentence on what's in the way and what the two answers do. */
  description,
  /** The keep-it button's label — the screens name different nouns. */
  cancelLabel,
  blockers,
  /** Caveats about how the verdict itself was reached — chiefly a failed
   * `fetch --prune`, meaning the blockers were judged against stale `origin/*`.
   * Rendered above the list because it qualifies every row below it: an offline
   * refusal must not read as an authoritative one. */
  messages,
  /** Locks the destructive actions: a delete or a stop+retry is in flight. */
  busy = false,
  /** Locks *cancel* specifically. Deliberately separate from `busy`: during a
   * port stop, backing out is still real (the retry checks the flow and stands
   * down), but once the delete itself is running "keep it" can no longer be
   * honored, so the dialog must stay up until the delete resolves and closes it
   * honestly. Collapsing the two would either lie or over-lock. */
  cancelDisabled = false,
  /** The port whose stop is in flight, so its own button can say so. */
  stoppingPort,
  /** Clear the port a blocker names and retry. Omitted on screens that don't
   * implement the retry loop — the row then still renders its remedy as text,
   * it just gets no button. */
  onStopPort,
  onForce,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  name: ReactNode;
  description: ReactNode;
  cancelLabel: string;
  blockers: TaskBlocker[];
  messages: string[];
  busy?: boolean;
  cancelDisabled?: boolean;
  stoppingPort?: number | null;
  onStopPort?: (port: number) => void;
  onForce: () => void;
}) {
  return (
    // Every reason gets its own row with its own way out, because the reasons
    // are independent and only some are actionable from here: a stale dev
    // server is one button, a dirty tree is work only the user can decide
    // about. The force sits in the footer under a label that names what it
    // discards — this dialog is where consent to lose work is actually given,
    // since the confirm before it promised the delete was guarded.
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      {/* Wider than the default alert, which is sized for a sentence and two
          buttons: this one carries a list of blocker cards. The `!` beats the
          primitive's own `data-[size=default]:` width, which outranks a plain
          utility class. */}
      <AlertDialogContent className="max-w-[calc(100%-2rem)]! sm:max-w-xl!">
        <AlertDialogHeader>
          <AlertDialogTitle className="wrap-anywhere">Can’t delete {name} yet</AlertDialogTitle>
          <AlertDialogDescription className="text-pretty">{description}</AlertDialogDescription>
        </AlertDialogHeader>
        {messages.length > 0 && (
          <ul className="flex flex-col gap-1 rounded-md border border-amber-500/40 bg-amber-500/10 px-2.5 py-2">
            {messages.map((message) => (
              <li key={message} className="text-[11.5px] text-amber-600 dark:text-amber-400">
                {message}
              </li>
            ))}
          </ul>
        )}
        {/* Scrolls rather than growing past the viewport — a task can be
            blocked by a dirty tree, unlanded commits and several ports at
            once, and the footer must stay reachable. */}
        <ul className="flex max-h-[45vh] flex-col gap-2 overflow-y-auto">
          {blockers.map((blocker, i) => {
            const port = stoppablePort(blocker);
            return (
              <li
                key={`${blocker.kind}-${port ?? i}`}
                className="flex items-start gap-3 rounded-lg border border-border bg-muted/40 px-3 py-2.5"
              >
                <BlockerIcon kind={blocker.kind} losesWork={blocker.losesWork} />
                <div className="flex min-w-0 flex-1 flex-col gap-0.5">
                  <span className="text-sm leading-snug wrap-anywhere">{blocker.message}</span>
                  <span className="text-xs leading-snug text-muted-foreground">
                    {blocker.remedy}
                  </span>
                </div>
                {port !== null &&
                  onStopPort && (
                    // Every action is disabled while any stop+retry runs, not
                    // just this row's: they all end in a delete of the same
                    // worktree, and two of those overlapping means concurrent
                    // `docker compose down` / `git worktree remove`.
                    <Button
                      size="sm"
                      variant="secondary"
                      className="shrink-0"
                      disabled={busy}
                      onClick={() => onStopPort(port)}
                    >
                      {stoppingPort === port ? "Stopping…" : "Stop it"}
                    </Button>
                  )}
              </li>
            );
          })}
        </ul>
        {/* Opposite ends for opposite answers — the discard label runs long
            enough that crowding it against keep reads as one two-part button. */}
        <AlertDialogFooter className="sm:justify-between">
          <AlertDialogCancel disabled={cancelDisabled}>{cancelLabel}</AlertDialogCancel>
          <AlertDialogAction variant="destructive" disabled={busy} onClick={onForce}>
            {forceDeleteLabel(blockers)}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
