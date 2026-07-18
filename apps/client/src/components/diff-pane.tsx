import { useCallback, useEffect, useState } from "react";
import { GitCompare, Pencil, RefreshCw } from "lucide-react";
import { DiffReview, type DiffReviewRequest } from "@/components/diff-review";
import { MonacoMultiDiff, type ChangedFile } from "@/components/diff-monaco";
import { IconBtn } from "@/components/agentboard-bits";
import { abInvoke, type FolderData } from "@/lib/agentboard";
import { ideReadFile, useIdeConnected } from "@/lib/ide";
import { isTauri } from "@/lib/tauri";
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
 * modal. A changed-file list on the left, the VS Code diff editor for the
 * selected file on the right. Content refetches whenever the folder's git
 * stats change (the 1.5s poll only bumps them on real change), so the diff
 * tracks the agent's edits without a manual refresh; the open file's contents
 * refresh in place. The header's segmented toggle picks the baseline:
 * the whole branch vs main, or just the uncommitted working tree. The full
 * checkout tree is its own pane (`FolderFilesPane`), not a tab here.
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
  // The worktree slot's own creation base (`.tt-slot`'s `base=`), when this
  // folder is a slot and nothing overrides it — what the backend actually
  // auto-compares against instead of always defaulting to main.
  const slotBaseBranch = folder?.slotBaseBranch?.trim() || null;
  const effectiveBase = baseBranch ?? slotBaseBranch;
  const [mode, setMode] = useState<DiffMode>("main");
  const [files, setFiles] = useState<ChangedFile[] | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [editingBase, setEditingBase] = useState(false);
  // Claude's pending openDiff reviews for this folder (shown one at a time,
  // oldest first). Each carries the on-disk "before" for the DiffEditor.
  const [reviews, setReviews] = useState<Array<DiffReviewRequest & { originalContent: string }>>(
    [],
  );

  // Claude called openDiff → queue an accept/reject review; close_tab /
  // closeAllDiffTabs dismiss (Rust already rejected the blocked calls).
  useEffect(() => {
    if (!dir || !isTauri()) return;
    let disposed = false;
    const unlistens: Array<() => void> = [];
    void (async () => {
      const { listen } = await import("@tauri-apps/api/event");
      const opened = await listen<DiffReviewRequest>("ide://open-diff", (e) => {
        if (e.payload.dir !== dir) return;
        const raw = e.payload.oldFilePath;
        const rel = raw.startsWith(`${dir}/`) ? raw.slice(dir.length + 1) : raw;
        void ideReadFile(dir, rel)
          .then((read) => {
            setReviews((prev) => [...prev, { ...e.payload, originalContent: read?.content ?? "" }]);
          })
          .catch(() => {
            // Unreadable old file (new file, binary) — review against empty.
            setReviews((prev) => [...prev, { ...e.payload, originalContent: "" }]);
          });
      });
      const closed = await listen<{ dir: string; tabName: string | null }>(
        "ide://close-diff",
        (e) => {
          if (e.payload.dir !== dir) return;
          const tab = e.payload.tabName;
          setReviews((prev) => (tab == null ? [] : prev.filter((r) => r.tabName !== tab)));
        },
      );
      if (disposed) {
        opened();
        closed();
      } else {
        unlistens.push(opened, closed);
      }
    })();
    return () => {
      disposed = true;
      for (const un of unlistens) un();
    };
  }, [dir]);

  // Claude Code IDE bridge badge — the MonacoFileDiff streams selections to
  // the folder's `claude` itself (same contract as CodeViewer).
  const ideConnected = useIdeConnected(dir);

  const mainMode = {
    key: "main" as const,
    label: effectiveBase ? `vs ${effectiveBase}` : "vs main",
    hint: baseBranch
      ? `Everything on this branch vs where it forked from "${baseBranch}" (your override) — committed and uncommitted work alike`
      : slotBaseBranch
        ? `Everything on this branch vs where it forked from "${slotBaseBranch}" (this slot's creation base) — committed and uncommitted work alike`
        : "Everything on this branch vs where it forked from origin/main — committed and uncommitted work alike",
  };
  const modes = [mainMode, UNCOMMITTED_MODE];

  const fetchDiff = useCallback(async () => {
    if (!dir) return;
    setRefreshing(true);
    const list = await abInvoke<ChangedFile[]>("ab_get_diff_files", { dir, mode, baseBranch });
    setFiles(list ?? []);
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
        {ideConnected && (
          <span
            title="A Claude Code session in this folder is connected — highlighted lines become its selection context"
            className="flex shrink-0 items-center gap-1 rounded-md border border-violet-500/50 bg-violet-500/10 px-1.5 font-mono text-[10.5px] text-violet-500"
          >
            ✦ claude
          </span>
        )}
        {editingBase ? (
          <input
            autoFocus
            defaultValue={baseBranch ?? ""}
            placeholder={
              slotBaseBranch
                ? `branch to compare against (blank = this slot's base, "${slotBaseBranch}")`
                : "branch to compare against (blank = auto-detect main)"
            }
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
            title={
              slotBaseBranch
                ? `set the parent branch this folder compares against (default: this slot's base, "${slotBaseBranch}")`
                : "set the parent branch this folder compares against (default: origin/main)"
            }
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
      <div className="relative flex min-h-0 flex-1 p-2">
        {files == null ? (
          <p className="p-2 text-sm text-muted-foreground">Loading…</p>
        ) : files.length === 0 ? (
          <p className="p-2 text-sm text-muted-foreground">No changes.</p>
        ) : (
          <MonacoMultiDiff
            dir={dir!}
            files={files}
            mode={mode}
            baseBranch={baseBranch}
            refreshKey={statsKey}
          />
        )}
        {reviews[0] && (
          <DiffReview
            key={reviews[0].requestId}
            review={reviews[0]}
            originalContent={reviews[0].originalContent}
            onDone={() => setReviews((prev) => prev.slice(1))}
          />
        )}
      </div>
    </div>
  );
}
