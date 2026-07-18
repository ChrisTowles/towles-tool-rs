import { useEffect, useMemo, useState } from "react";
import { Folder } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ScrollArea } from "@/components/ui/scroll-area";
import { requestOpenSession, resumeCandidates } from "@/lib/agentboard";
import type { ResumeCandidate } from "@/lib/agentboard";
import { fmtAge } from "@/lib/data";
import { useWorkspace } from "@/lib/workspace";
import { cn } from "@/lib/utils";

/**
 * After an unexpected exit, offer to relaunch the Claude sessions that were
 * running in tt's panes (`claude --resume`).
 *
 * The backend decides *whether* to prompt at all — it returns candidates only
 * when the previous run left a dirty run marker, and only once per launch — so
 * this mounts unconditionally and stays invisible after a clean shutdown. That
 * keeps the "did we crash?" logic in one tested place
 * (`tt_agentboard::resume`) rather than split across the UI.
 *
 * Everything is pre-checked: after a crash the common case is "give me all of
 * it back", and unticking the odd one is cheaper than hunting down five
 * checkboxes.
 */
export function ResumePicker() {
  const [candidates, setCandidates] = useState<ResumeCandidate[]>([]);
  const [chosen, setChosen] = useState<Set<string>>(new Set());
  const [open, setOpen] = useState(false);
  const { openTab } = useWorkspace();

  useEffect(() => {
    void (async () => {
      const found = await resumeCandidates();
      if (found.length === 0) return;
      setCandidates(found);
      setChosen(new Set(found.map((c) => c.paneId)));
      setOpen(true);
    })();
  }, []);

  function toggle(paneId: string) {
    setChosen((prev) => {
      const next = new Set(prev);
      if (!next.delete(paneId)) next.add(paneId);
      return next;
    });
  }

  function resume() {
    const picked = candidates.filter((c) => chosen.has(c.paneId));
    setOpen(false);
    if (picked.length === 0) return;
    // Agentboard owns the pane→PTY machinery, so hand off rather than
    // duplicating it. It may not be mounted yet at boot, which is exactly why
    // the open-session bridge stashes a queue (see `requestOpenSession`).
    openTab("agentboard");
    // Oldest first: Agentboard activates each folder as it restores it, so the
    // last one handed over is the one left on screen — and after a crash that
    // should be the session you were most recently in.
    for (const c of [...picked].reverse()) {
      requestOpenSession({
        folderDir: c.folderDir,
        sessionId: c.paneId,
        resumeId: c.claudeSessionId,
        label: c.title ?? c.paneName,
      });
    }
  }

  const now = Date.now();
  const byFolder = useMemo(() => {
    const m = new Map<string, ResumeCandidate[]>();
    for (const c of candidates) m.set(c.folderDir, [...(m.get(c.folderDir) ?? []), c]);
    return m;
  }, [candidates]);

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>Resume your sessions?</DialogTitle>
          <DialogDescription>
            Towles Tool closed unexpectedly. These panes were running Claude —
            pick the ones to relaunch with{" "}
            <span className="font-mono text-xs">claude --resume</span>.
          </DialogDescription>
        </DialogHeader>

        <ScrollArea className="max-h-80 -mx-2 px-2">
          {[...byFolder].map(([dir, list]) => (
            <div key={dir} className="mb-3 last:mb-0">
              <div className="flex items-center gap-2 border-b border-border px-1 pb-1">
                <Folder className="size-3.5 text-muted-foreground/70" />
                <span className="truncate font-medium text-muted-foreground text-[13px]">
                  {folderLabel(dir)}
                </span>
              </div>
              {list.map((c) => {
                const picked = chosen.has(c.paneId);
                return (
                  // `<label htmlFor>`, not `<button>`: Radix's Checkbox renders
                  // a button and buttons can't nest. See apps/client/CLAUDE.md.
                  <label
                    key={c.paneId}
                    htmlFor={`resume-${c.paneId}`}
                    className={cn(
                      "flex w-full cursor-pointer items-center gap-2.5 rounded-md py-2 pr-2 pl-3 text-left",
                      "hover:bg-accent/50",
                      picked && "bg-accent",
                    )}
                  >
                    <Checkbox
                      id={`resume-${c.paneId}`}
                      checked={picked}
                      onCheckedChange={() => toggle(c.paneId)}
                    />
                    <span className="w-4 text-center font-mono text-violet-500 text-xs">✦</span>
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-[13px] text-foreground">
                        {c.title ?? c.paneName}
                      </span>
                      <span className="block truncate font-mono text-[11px] text-muted-foreground/60">
                        {c.paneName}
                      </span>
                    </span>
                    <span className="shrink-0 font-mono text-[11px] text-muted-foreground">
                      {fmtAge(c.lastActiveMs, now)}
                    </span>
                  </label>
                );
              })}
            </div>
          ))}
        </ScrollArea>

        <DialogFooter>
          <Button variant="ghost" onClick={() => setOpen(false)}>
            Not now
          </Button>
          <Button onClick={resume} disabled={chosen.size === 0}>
            {chosen.size === 1
              ? "Resume 1 session"
              : `Resume ${chosen.size} sessions`}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/** Last two path segments — enough to tell slots of one repo apart. */
function folderLabel(dir: string): string {
  return dir.split("/").filter(Boolean).slice(-2).join("/");
}
