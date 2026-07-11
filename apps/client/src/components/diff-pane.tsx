import { useCallback, useEffect, useState } from "react";
import { GitCompare, RefreshCw } from "lucide-react";
import { DiffViewer } from "@/components/diff-view";
import { IconBtn } from "@/components/agentboard-bits";
import { abInvoke, type FolderData } from "@/lib/agentboard";
import { cn } from "@/lib/utils";

/** Which baseline the pane diffs against (mirrors `DiffMode` in tt-agentboard). */
type DiffMode = "main" | "uncommitted";

const MODES: { key: DiffMode; label: string; hint: string }[] = [
  {
    key: "main",
    label: "vs main",
    hint: "Everything on this branch vs where it forked from origin/main — committed and uncommitted work alike",
  },
  {
    key: "uncommitted",
    label: "uncommitted",
    hint: "Only what isn't committed yet — staged + unstaged changes vs HEAD",
  },
];

/**
 * A folder's diff as a *pane* in the Agentboard tiling — it sits beside the
 * live terminals (review while the agent works) instead of covering them in a
 * modal. Content refetches whenever the folder's git stats change (the 1.5s
 * poll only bumps them on real change), so the patch tracks the agent's edits
 * without a manual refresh. The header's segmented toggle picks the baseline:
 * the whole branch vs main, or just the uncommitted working tree.
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
  const [mode, setMode] = useState<DiffMode>("main");
  const [text, setText] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);

  const fetchDiff = useCallback(async () => {
    if (!dir) return;
    setRefreshing(true);
    const t = await abInvoke<string>("ab_get_diff", { dir, mode });
    setText(t ?? "");
    setRefreshing(false);
  }, [dir, mode]);

  // Refetch on mount and whenever the working tree measurably changes.
  const statsKey = folder
    ? `${folder.filesChanged}:${folder.linesAdded}:${folder.linesRemoved}:${folder.commitsAhead}`
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
        <span className="flex shrink-0 items-center rounded-md border border-border/70 p-0.5">
          {MODES.map((m) => (
            <button
              key={m.key}
              type="button"
              title={m.hint}
              aria-pressed={mode === m.key}
              onClick={() => setMode(m.key)}
              className={cn(
                "rounded-[5px] px-1.5 py-px font-mono text-[10.5px] transition-colors",
                mode === m.key
                  ? "bg-accent text-foreground"
                  : "text-muted-foreground hover:text-foreground",
              )}
            >
              {m.label}
            </button>
          ))}
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
