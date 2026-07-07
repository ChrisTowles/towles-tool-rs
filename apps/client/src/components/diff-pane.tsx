import { useCallback, useEffect, useState } from "react";
import { GitCompare, RefreshCw } from "lucide-react";
import { DiffViewer } from "@/components/diff-view";
import { IconBtn } from "@/components/agentboard-bits";
import { abInvoke, type FolderData } from "@/lib/agentboard";

/**
 * A folder's diff as a *pane* in the Agentboard tiling — it sits beside the
 * live terminals (review while the agent works) instead of covering them in a
 * modal. Content refetches whenever the folder's git stats change (the 1.5s
 * poll only bumps them on real change), so the patch tracks the agent's edits
 * without a manual refresh.
 */
export function DiffPane({
  folder,
  onClose,
}: {
  /** The checkout this pane diffs; undefined when it left the rail. */
  folder: FolderData | undefined;
  /** Removes the pane from its window. */
  onClose: () => void;
}) {
  const dir = folder?.dir;
  const [text, setText] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);

  const fetchDiff = useCallback(async () => {
    if (!dir) return;
    setRefreshing(true);
    const t = await abInvoke<string>("ab_get_diff", { dir });
    setText(t ?? "");
    setRefreshing(false);
  }, [dir]);

  // Refetch on mount and whenever the working tree measurably changes.
  const statsKey = folder
    ? `${folder.filesChanged}:${folder.linesAdded}:${folder.linesRemoved}:${folder.commitsDelta}`
    : "";
  useEffect(() => {
    void fetchDiff();
  }, [fetchDiff, statsKey]);

  if (!folder) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 rounded-lg border border-dashed text-muted-foreground">
        <span className="text-sm">folder gone</span>
        <button type="button" onClick={onClose} className="font-mono text-xs hover:text-red-500">
          ⊟ remove pane
        </button>
      </div>
    );
  }

  return (
    <div className="flex h-full flex-col overflow-hidden rounded-lg border bg-card">
      <div className="flex shrink-0 items-center gap-2 border-b bg-card px-2 py-1">
        <GitCompare className="size-3.5 shrink-0 text-muted-foreground" />
        <span className="truncate font-mono text-xs text-foreground">{folder.name}</span>
        <span className="truncate text-[11px] text-muted-foreground">
          vs pushed base (merge-base with upstream, else origin/main)
        </span>
        <span className="ml-auto flex shrink-0 items-center gap-1.5">
          <IconBtn title="refresh diff" onClick={() => void fetchDiff()} className="hover:text-sky-500">
            <RefreshCw className={refreshing ? "size-3 animate-spin" : "size-3"} />
          </IconBtn>
          <IconBtn title="remove pane (diff stays a click away on the folder)" onClick={onClose} className="hover:text-red-500">
            ⊟
          </IconBtn>
        </span>
      </div>
      <div className="flex min-h-0 flex-1 flex-col p-2">
        {text == null ? (
          <p className="p-2 text-sm text-muted-foreground">Loading…</p>
        ) : (
          <DiffViewer text={text} />
        )}
      </div>
    </div>
  );
}
