import { useCallback, useEffect, useState } from "react";
import { RefreshCw, Trash2 } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { cn } from "@/lib/utils";
import { fetchShpoolSessions, killShpoolSession, type ShpoolSession } from "@/lib/shpool";

/**
 * Cleanup view for the persistent shells shpool holds for this slot. Lists
 * each session with its attached/disconnected state, flags orphans (a daemon
 * session whose agentboard record is gone — the shell kept running after the
 * pane was removed), and lets you kill any one, or all orphans at once.
 */
export function SessionsDialog({
  open,
  onOpenChange,
  knownIds,
}: {
  open: boolean;
  onOpenChange: (o: boolean) => void;
  /** Ids of every session that still has an agentboard record — used to flag
   * daemon sessions that don't (orphans). */
  knownIds: Set<string>;
}) {
  const [sessions, setSessions] = useState<ShpoolSession[] | null>(null);

  const refresh = useCallback(async () => {
    setSessions(await fetchShpoolSessions());
  }, []);

  useEffect(() => {
    if (open) void refresh();
  }, [open, refresh]);

  const orphans = (sessions ?? []).filter((s) => !knownIds.has(s.termId));

  const kill = async (name: string) => {
    try {
      await killShpoolSession(name);
      await refresh();
    } catch (e) {
      toast.error(String(e));
    }
  };

  const killOrphans = async () => {
    const n = orphans.length;
    for (const s of orphans) {
      try {
        await killShpoolSession(s.name);
      } catch {
        /* keep going; a failure just leaves that one for a retry */
      }
    }
    await refresh();
    toast.success(`Cleaned up ${n} orphaned session${n === 1 ? "" : "s"}`);
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>Terminal sessions</DialogTitle>
        </DialogHeader>
        <div className="flex items-center justify-between text-xs text-muted-foreground">
          <span>Persistent shells shpool keeps alive for this slot.</span>
          <button
            type="button"
            onClick={() => void refresh()}
            className="flex items-center gap-1 hover:text-foreground"
          >
            <RefreshCw className="size-3.5" /> refresh
          </button>
        </div>
        <div className="max-h-80 overflow-y-auto rounded-md border">
          {sessions === null ? (
            <div className="p-4 text-center text-sm text-muted-foreground">Loading…</div>
          ) : sessions.length === 0 ? (
            <div className="p-4 text-center text-sm text-muted-foreground">
              No persistent sessions.
            </div>
          ) : (
            sessions.map((s) => {
              const orphan = !knownIds.has(s.termId);
              return (
                <div
                  key={s.name}
                  className="flex items-center gap-2 border-b px-3 py-2 text-sm last:border-b-0"
                >
                  <span
                    className={cn(
                      "size-2 shrink-0 rounded-full",
                      s.status === "attached" ? "bg-emerald-500" : "bg-muted-foreground/40",
                    )}
                    title={s.status}
                  />
                  <span className="min-w-0 flex-1 truncate font-mono text-xs">{s.termId}</span>
                  {orphan && (
                    <span
                      className="shrink-0 rounded-md border border-amber-500/40 bg-amber-500/10 px-1.5 text-[10.5px] text-amber-600 dark:text-amber-400"
                      title="no agentboard record — the shell outlived its session"
                    >
                      orphan
                    </span>
                  )}
                  <button
                    type="button"
                    onClick={() => void kill(s.name)}
                    title="kill this session"
                    className="shrink-0 text-muted-foreground/60 hover:text-red-500"
                  >
                    <Trash2 className="size-3.5" />
                  </button>
                </div>
              );
            })
          )}
        </div>
        {orphans.length > 0 && (
          <Button
            variant="outline"
            size="sm"
            onClick={() => void killOrphans()}
            className="self-end border-amber-500/40 text-amber-600 hover:bg-amber-500/10 dark:text-amber-400"
          >
            Kill {orphans.length} orphaned
          </Button>
        )}
      </DialogContent>
    </Dialog>
  );
}
