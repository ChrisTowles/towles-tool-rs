import { TerminalSquare } from "lucide-react";
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
import {
  Command,
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import {
  sessionLabel,
  type RemoveTarget,
  type SessionData,
  type StartClaudeTarget,
} from "@/lib/agentboard";
import type { TaskItem, TaskOutcome } from "@/lib/data";
import { shortcutHint } from "@/lib/shortcuts";
import { cn } from "@/lib/utils";

/** ab-split-session picker: pick one of the active folder's not-yet-opened
 * sessions to add as a pane in its active window. */
export function SplitSessionDialog({
  open,
  onOpenChange,
  folderName,
  candidates,
  onPick,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  folderName?: string;
  candidates: SessionData[];
  onPick: (sessionId: string) => void;
}) {
  return (
    <CommandDialog
      open={open}
      onOpenChange={onOpenChange}
      title="Add to window"
      description={
        folderName
          ? `Pick a session from ${folderName} to add as a pane.`
          : "Pick a session to add as a pane."
      }
      className="sm:max-w-lg"
    >
      <Command>
        <CommandInput autoFocus placeholder="Search sessions…" />
        <CommandList className="max-h-[60vh]">
          <CommandEmpty>No sessions match.</CommandEmpty>
          <CommandGroup heading="Sessions">
            {candidates.map((s) => (
              <CommandItem key={s.id} value={sessionLabel(s)} onSelect={() => onPick(s.id)}>
                <TerminalSquare className="size-3.5 shrink-0 text-muted-foreground" />
                <span className="flex-1 truncate">{sessionLabel(s)}</span>
              </CommandItem>
            ))}
          </CommandGroup>
        </CommandList>
      </Command>
    </CommandDialog>
  );
}

/** Confirm removing a repo (or all its checkouts) from the rail when live
 * sessions would be stopped. */
export function RemoveRepoDialog({
  target,
  onOpenChange,
  onConfirm,
}: {
  target: RemoveTarget | null;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
}) {
  return (
    <AlertDialog open={target != null} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Remove {target?.label} from the rail?</AlertDialogTitle>
          <AlertDialogDescription>
            {target?.sessionIds.length}{" "}
            {target?.sessionIds.length === 1 ? "session is" : "sessions are"} still running.
            Removing will stop {target?.sessionIds.length === 1 ? "it" : "them"}.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction onClick={onConfirm}>Stop &amp; remove</AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

/** Confirm deleting a worktree from disk — and, when a board task is bound to
 * it, how that task ended (defaulted to done, with a swap link to abandoned). */
export function DeleteWorktreeDialog({
  target,
  task,
  outcome,
  onOpenChange,
  onSwapOutcome,
  onConfirm,
}: {
  target: RemoveTarget | null;
  task: TaskItem | null;
  outcome: TaskOutcome;
  onOpenChange: (open: boolean) => void;
  onSwapOutcome: () => void;
  onConfirm: () => void;
}) {
  return (
    <AlertDialog open={target != null} onOpenChange={onOpenChange}>
      {/* Same width as the blocked-delete dialog it can hand off to, so the
          flow doesn't jump size mid-decision. */}
      <AlertDialogContent className="max-w-[calc(100%-2rem)]! sm:max-w-xl!">
        <AlertDialogHeader>
          <AlertDialogTitle className="wrap-anywhere">
            {task
              ? `Close task & delete worktree ${target?.label}?`
              : `Delete worktree ${target?.label}?`}
          </AlertDialogTitle>
          <AlertDialogDescription className="text-pretty">
            Removes the checkout from disk (guarded — uncommitted changes, commits on no
            branch/remote, or a dev server still on its ports will stop it and tell you what to do).
            Its branch survives in the primary.
            {task && " The task stays on the board, closed."}
            {target && target.sessionIds.length > 0 && (
              <>
                {" "}
                {target.sessionIds.length}{" "}
                {target.sessionIds.length === 1 ? "session is" : "sessions are"} still running and
                will be stopped.
              </>
            )}
          </AlertDialogDescription>
        </AlertDialogHeader>
        {/* How the task ended, defaulted to `done` — the common case — with
            one underlined link to flip it to `abandoned`. Only rendered
            when a board task is bound; a bare worktree has nothing to
            record. */}
        {task && (
          <div className="flex flex-wrap items-center gap-2 text-xs">
            <span
              className={cn(
                "rounded px-1.5 py-0.5 font-mono",
                outcome === "done"
                  ? "bg-emerald-500/10 text-emerald-500"
                  : "bg-muted text-muted-foreground",
              )}
            >
              {(() => {
                const merged = task.prs.find((p) => p.state === "merged");
                return outcome === "done"
                  ? merged
                    ? `PR #${merged.number} merged — closing as done ✓`
                    : "closing as done ✓"
                  : merged
                    ? `closing as abandoned ⊘ (PR #${merged.number} merged)`
                    : "no merged PR — closing as abandoned ⊘";
              })()}
            </span>
            <button
              type="button"
              className="text-muted-foreground underline underline-offset-2 hover:text-foreground"
              onClick={onSwapOutcome}
            >
              record as {outcome === "done" ? "abandoned" : "done"} instead
            </button>
          </div>
        )}
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction
            onClick={onConfirm}
            title={`Confirm (${shortcutHint("ab-confirm-close-worktree")})`}
          >
            {task ? `Close as ${outcome}` : "Delete worktree"}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

/** The "what are you working toward?" prompt shown before Claude launches in a
 * fresh session. Blank is a valid answer — it just skips the initial prompt. */
export function StartClaudeDialog({
  target,
  prompt,
  onPromptChange,
  onCommit,
  onOpenChange,
}: {
  target: StartClaudeTarget | null;
  prompt: string;
  onPromptChange: (value: string) => void;
  onCommit: () => void;
  onOpenChange: (open: boolean) => void;
}) {
  return (
    <Dialog open={target != null} onOpenChange={onOpenChange}>
      <DialogContent showCloseButton={false}>
        <DialogHeader>
          <DialogTitle>✦ Start Claude{target ? ` in ${target.sessionName}` : ""}</DialogTitle>
        </DialogHeader>
        <Input
          autoFocus
          value={prompt}
          onChange={(e) => onPromptChange(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              onCommit();
            }
          }}
          placeholder="what are you working toward? (optional)"
        />
      </DialogContent>
    </Dialog>
  );
}
