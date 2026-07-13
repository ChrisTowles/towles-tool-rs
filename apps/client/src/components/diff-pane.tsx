import { useCallback, useEffect, useState } from "react";
import { GitCompare, Pencil, RefreshCw } from "lucide-react";
import { DiffViewer } from "@/components/diff-view";
import { IconBtn } from "@/components/agentboard-bits";
import { abInvoke, type FolderData } from "@/lib/agentboard";
import { cn } from "@/lib/utils";

/** Which baseline the pane diffs against (mirrors `DiffMode` in tt-agentboard). */
type DiffMode = "main" | "uncommitted";

const UNCOMMITTED_MODE = {
  key: "uncommitted" as const,
  label: "uncommitted",
  hint: "Only what isn't committed yet — staged + unstaged changes vs HEAD",
};

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
  const baseBranch = folder?.baseBranch?.trim() || null;
  const [mode, setMode] = useState<DiffMode>("main");
  const [text, setText] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [editingBase, setEditingBase] = useState(false);

  const mainMode = {
    key: "main" as const,
    label: baseBranch ? `vs ${baseBranch}` : "vs main",
    hint: baseBranch
      ? `Everything on this branch vs where it forked from "${baseBranch}" (your override) — committed and uncommitted work alike`
      : "Everything on this branch vs where it forked from origin/main — committed and uncommitted work alike",
  };
  const modes = [mainMode, UNCOMMITTED_MODE];

  const fetchDiff = useCallback(async () => {
    if (!dir) return;
    setRefreshing(true);
    const t = await abInvoke<string>("ab_get_diff", { dir, mode, baseBranch });
    setText(t ?? "");
    setRefreshing(false);
  }, [dir, mode, baseBranch]);

  // Refetch on mount and whenever the working tree measurably changes.
  const statsKey = folder
    ? `${folder.filesChanged}:${folder.linesAdded}:${folder.linesRemoved}:${folder.commitsAhead}`
    : "";
  useEffect(() => {
    void fetchDiff();
  }, [fetchDiff, statsKey]);

  async function commitBaseBranch(value: string) {
    setEditingBase(false);
    if (!dir) return;
    const trimmed = value.trim();
    if (trimmed === (baseBranch ?? "")) return;
    await abInvoke("ab_set_folder_base_branch", { dir, branch: trimmed || null });
  }

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
        {editingBase ? (
          <input
            autoFocus
            defaultValue={baseBranch ?? ""}
            placeholder="branch to compare against (blank = auto-detect main)"
            onBlur={(e) => void commitBaseBranch(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") void commitBaseBranch((e.target as HTMLInputElement).value);
              if (e.key === "Escape") setEditingBase(false);
            }}
            className="w-48 rounded-sm border border-input bg-background px-1.5 py-0.5 font-mono text-[10.5px] outline-none"
          />
        ) : (
          <span className="flex shrink-0 items-center rounded-md border border-border/70 p-0.5">
            {modes.map((m) => (
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
        )}
        {!editingBase && mode === "main" && (
          <IconBtn
            title="set the parent branch this folder compares against (default: origin/main)"
            onClick={() => setEditingBase(true)}
            className="hover:text-sky-500"
          >
            <Pencil className="size-3" />
          </IconBtn>
        )}
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
