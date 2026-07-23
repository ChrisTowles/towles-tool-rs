import {
  memo,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type Dispatch,
  type SetStateAction,
} from "react";
import { ChevronRight, Pencil, RefreshCw, X } from "lucide-react";
import { DiffReview, type DiffReviewRequest } from "@/components/diff-review";
import { MonacoMultiDiff, type ChangedFile } from "@/components/diff-monaco";
import { ClaudeBadge, IconBtn, PanePlaceholder } from "@/components/agentboard-bits";
import { PaneChrome, PaneLens } from "@/components/pane-chrome";
import { Checkbox } from "@/components/ui/checkbox";
import { folderStatsKey, type FolderData } from "@/lib/agentboard";
import { buildDiffTree, type DiffTreeNode } from "@/lib/diff";
import { ideReadFile, useIdeConnected } from "@/lib/ide";
import { invoke, isTauri } from "@/lib/tauri";
import { cn } from "@/lib/utils";
import { toast } from "sonner";

/** Which baseline the pane diffs against (mirrors `DiffMode` in tt-agentboard). */
type DiffMode = "main" | "uncommitted";

/** Git name-status letter → folder-rail-ish color in the tree rail. */
const STATUS_COLORS: Record<string, string> = {
  A: "text-emerald-500",
  "?": "text-emerald-500",
  D: "text-red-500",
  R: "text-sky-500",
  C: "text-sky-500",
  M: "text-amber-500",
};

/** Compact navigation tree beside the multi-diff: same compact-folders
 * grouping as the Files pane; clicking a file scrolls its diff into view.
 * Each row carries a "reviewed" checkbox — GitHub-review-style, not git
 * staging — that collapses that file's diff in the Monaco pane so an
 * approved file is out of the way; a folder's checkbox reflects and toggles
 * every file beneath it at once. Memoized: DiffPane re-renders on state
 * (refreshing, editingBase, reviews) this rail doesn't care about, and its
 * props are stable references except when `files`/`reviewed` actually change. */
const DiffTreeRail = memo(function DiffTreeRail({
  files,
  reviewed,
  dirty,
  conflict,
  onJump,
  onToggleReviewed,
  onToggleReviewedMany,
}: {
  files: ChangedFile[];
  /** Paths the reviewer has checked off. */
  reviewed: ReadonlySet<string>;
  /** Paths with unsaved edits made right in the diff pane — same signal the
   * Files tab's dirty dot shows, mirrored here per file. */
  dirty: ReadonlySet<string>;
  /** Paths whose disk content changed under those unsaved edits — resolution
   * lives in the Monaco pane's banner; this rail just marks which rows. */
  conflict: ReadonlySet<string>;
  onJump: (path: string) => void;
  /** Toggle one file's reviewed flag. */
  onToggleReviewed: (path: string) => void;
  /** Set (or clear) every path in the list at once — a folder's checkbox. */
  onToggleReviewedMany: (paths: string[], value: boolean) => void;
}) {
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set());
  const tree = useMemo(() => buildDiffTree(files.map((f) => f.path)), [files]);
  const byPath = useMemo(() => new Map(files.map((f) => [f.path, f])), [files]);
  // One bottom-up pass per file-set change, not a per-render re-walk of every
  // folder's subtree — leafPaths() is O(subtree), and calling it fresh inside
  // renderNodes for every folder node made a deeply nested tree (this repo's
  // own apps/client/src/components/... shape) cost O(n²) on every render,
  // including ones that only touch unrelated pane state (refreshing, mode).
  const leafPathsByFolder = useMemo(() => {
    const map = new Map<string, string[]>();
    const walk = (node: DiffTreeNode): string[] => {
      if (node.kind === "file") return [node.path];
      const leaves = node.children.flatMap(walk);
      map.set(node.path, leaves);
      return leaves;
    };
    tree.forEach(walk);
    return map;
  }, [tree]);

  const renderNodes = (nodes: DiffTreeNode[], depth: number) =>
    nodes.map((node) => {
      const paddingLeft = 4 + depth * 12;
      if (node.kind === "folder") {
        const isCollapsed = collapsed.has(node.path);
        const paths = leafPathsByFolder.get(node.path) ?? [];
        const reviewedCount = paths.filter((p) => reviewed.has(p)).length;
        const checked: boolean | "indeterminate" =
          reviewedCount === 0 ? false : reviewedCount === paths.length ? true : "indeterminate";
        return (
          <li key={node.path}>
            <div style={{ paddingLeft }} className="flex w-full items-center gap-1 py-0.5">
              {/* `<label htmlFor>`, not nested in the button below: Radix's
               * Checkbox renders a button and buttons can't nest. See
               * apps/client/CLAUDE.md. */}
              <label
                htmlFor={`reviewed-${node.path}`}
                onClick={(e) => e.stopPropagation()}
                className="flex shrink-0 items-center"
                title="mark every file in this folder reviewed"
              >
                <Checkbox
                  id={`reviewed-${node.path}`}
                  checked={checked}
                  onCheckedChange={(c) => onToggleReviewedMany(paths, c === true)}
                />
              </label>
              <button
                type="button"
                onClick={() =>
                  setCollapsed((prev) => {
                    const next = new Set(prev);
                    if (isCollapsed) next.delete(node.path);
                    else next.add(node.path);
                    return next;
                  })
                }
                className="flex min-w-0 flex-1 items-center gap-1 text-left font-mono text-[11px] text-muted-foreground hover:text-foreground"
              >
                <ChevronRight
                  className={cn(
                    "size-3 shrink-0 transition-transform",
                    !isCollapsed && "rotate-90",
                  )}
                />
                <span className="truncate">{node.name}</span>
              </button>
            </div>
            {!isCollapsed && <ul>{renderNodes(node.children, depth + 1)}</ul>}
          </li>
        );
      }
      const file = byPath.get(node.path);
      return (
        <li key={node.path}>
          <div
            style={{ paddingLeft: paddingLeft + 14 }}
            className="flex w-full items-center gap-1.5 py-0.5 font-mono text-[11px] text-muted-foreground hover:text-foreground"
          >
            <label
              htmlFor={`reviewed-${node.path}`}
              onClick={(e) => e.stopPropagation()}
              className="flex shrink-0 items-center"
              title="mark reviewed (collapses this file's diff)"
            >
              <Checkbox
                id={`reviewed-${node.path}`}
                checked={reviewed.has(node.path)}
                onCheckedChange={() => onToggleReviewed(node.path)}
              />
            </label>
            <button
              type="button"
              onClick={() => onJump(node.path)}
              title={file?.oldPath ? `${file.oldPath} → ${node.path}` : node.path}
              className="flex min-w-0 flex-1 items-center gap-1.5 text-left"
            >
              <span className={cn("shrink-0", STATUS_COLORS[file?.status ?? ""] ?? "")}>
                {file?.status ?? ""}
              </span>
              <span className="min-w-0 flex-1 truncate">{node.name}</span>
              {conflict.has(node.path) ? (
                <span
                  title="Changed on disk while you have unsaved edits — resolve in the banner"
                  className="size-1.5 shrink-0 rounded-full bg-red-500"
                />
              ) : (
                dirty.has(node.path) && (
                  <span
                    title="Unsaved changes — autosaves after a pause; ⌘S saves now"
                    className="size-1.5 shrink-0 rounded-full bg-amber-500"
                  />
                )
              )}
              {file && (file.linesAdded > 0 || file.linesRemoved > 0) && (
                <span className="shrink-0 pr-1 text-[10px]">
                  <span className="text-emerald-500">+{file.linesAdded}</span>{" "}
                  <span className="text-red-500">−{file.linesRemoved}</span>
                </span>
              )}
            </button>
          </div>
        </li>
      );
    });

  return <ul className="w-56 shrink-0 overflow-y-auto border-r pr-1">{renderNodes(tree, 0)}</ul>;
});

/** A state setter for a `Set<string>` → a `(path, on)` toggle that only
 * produces a new Set on an actual transition — shared by the dirty and
 * conflict mirrors so the flip logic can't drift between them. */
function flipPathIn(setter: Dispatch<SetStateAction<Set<string>>>) {
  return (path: string, on: boolean) => {
    setter((prev) => {
      if (prev.has(path) === on) return prev;
      const next = new Set(prev);
      if (on) next.add(path);
      else next.delete(path);
      return next;
    });
  };
}

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
  focused,
  onClose,
}: {
  /** The checkout this pane diffs; undefined when it left the rail. */
  folder: FolderData | undefined;
  /** This pane is the one the user last clicked into — see the focus-ring
   * rule in `screens/agentboard.tsx`'s `focusedPaneId`. */
  focused: boolean;
  /** Removes the pane from its window. */
  onClose: () => void;
}) {
  const dir = folder?.dir;
  const baseBranch = folder?.baseBranch?.trim() || null;
  // The worktree's own creation base (`.tt-task`'s `base=`), when this
  // folder is a task and nothing overrides it — what the backend actually
  // auto-compares against instead of always defaulting to main.
  const taskBaseBranch = folder?.taskBaseBranch?.trim() || null;
  const effectiveBase = baseBranch ?? taskBaseBranch;
  const [mode, setMode] = useState<DiffMode>("main");
  const [files, setFiles] = useState<ChangedFile[] | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const revealRef = useRef<((path: string) => void) | null>(null);
  const registerReveal = useCallback((fn: ((path: string) => void) | null) => {
    revealRef.current = fn;
  }, []);
  // Stable identity (unlike an inline arrow at the call site) so DiffTreeRail's
  // memo() isn't defeated by a fresh callback prop on every DiffPane render.
  const jumpTo = useCallback((path: string) => revealRef.current?.(path), []);
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
        void ideReadFile(dir, rel).then((read) => {
          // An unreadable old file (new file, binary) reviews against empty.
          const originalContent = read.map((f) => f.content).unwrapOr("");
          setReviews((prev) => [...prev, { ...e.payload, originalContent }]);
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
      : taskBaseBranch
        ? `Everything on this branch vs where it forked from "${taskBaseBranch}" (this task's creation base) — committed and uncommitted work alike`
        : "Everything on this branch vs where it forked from origin/main — committed and uncommitted work alike",
  };
  const modes = [mainMode, UNCOMMITTED_MODE];

  // Files the reviewer has checked off — a GitHub-review-style "viewed" mark,
  // purely client-side (not persisted, no git index involved). Checking a
  // file collapses its diff in the Monaco pane; unchecking expands it again.
  const [reviewed, setReviewed] = useState<Set<string>>(() => new Set());

  // Files with unsaved edits made right in the Monaco pane — mirrors what
  // MonacoMultiDiff also reports to the IDE bridge (`ideSetDiffDirty`), kept
  // here too so the tree rail can show the same dirty dot the Files tab does.
  const [dirty, setDirty] = useState<Set<string>>(() => new Set());
  // Files whose disk content changed under unsaved pane edits — reported by
  // MonacoMultiDiff (which owns the resolution banner); mirrored here so the
  // tree rail can mark the affected rows.
  const [conflict, setConflict] = useState<Set<string>>(() => new Set());
  const handleDirtyChange = useMemo(() => flipPathIn(setDirty), []);
  const handleConflictChange = useMemo(() => flipPathIn(setConflict), []);

  const fetchDiff = useCallback(async () => {
    if (!dir) return;
    setRefreshing(true);
    const list = await invoke<ChangedFile[]>("ab_get_diff_files", { dir, mode, baseBranch });
    const nextFiles = list.unwrapOr([]);
    setFiles(nextFiles);
    // Prune reviewed marks for paths that dropped out of the change set
    // (renamed/reverted/committed) — everything else keeps its mark across a
    // refresh, so a poll mid-review doesn't quietly re-expand what's already
    // been checked off.
    const paths = new Set(nextFiles.map((f) => f.path));
    setReviewed((prev) => {
      const next = new Set([...prev].filter((p) => paths.has(p)));
      return next.size === prev.size ? prev : next;
    });
    setRefreshing(false);
  }, [dir, mode, baseBranch]);

  // Switching folders starts a fresh review — marks from the last folder
  // don't belong to this one's file set.
  useEffect(() => {
    setReviewed(new Set());
    setDirty(new Set());
    setConflict(new Set());
  }, [dir]);

  // Refetch on mount and whenever the working tree measurably changes.
  const statsKey = folder ? folderStatsKey(folder) : "";
  // The baseline can only move when a commit lands, the branch is rebased
  // (commitsBehind snaps to 0 without commitsAhead moving), or the compared
  // ref changes — the multi-diff refetches its read-only base sides on
  // this, not on every working-tree stats bump.
  const baseKey = folder
    ? `${folder.commitsAhead}:${folder.commitsBehind}:${folder.comparedBase ?? ""}`
    : "";
  useEffect(() => {
    void fetchDiff();
  }, [fetchDiff, statsKey]);

  async function commitBaseBranch(value: string) {
    setEditingBase(false);
    if (!dir) return;
    const trimmed = value.trim();
    if (trimmed === (baseBranch ?? "")) return;
    const stored = await invoke<void>("ab_set_folder_base_branch", {
      dir,
      branch: trimmed || null,
    });
    // Silence here reads as success while the pane keeps diffing the old base —
    // the wrong diff is worse than no diff, so surface it.
    if (stored.isErr()) toast.error(`Couldn't set base branch — ${stored.error.message}`);
  }

  // Shared by the tree rail's checkboxes and the Monaco header's checkbox —
  // toggles one file's reviewed mark.
  const toggleReviewed = useCallback((path: string) => {
    setReviewed((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }, []);

  // A folder's checkbox in the tree rail — set (or clear) every file beneath
  // it at once.
  const toggleReviewedMany = useCallback((paths: string[], value: boolean) => {
    setReviewed((prev) => {
      const next = new Set(prev);
      for (const p of paths) {
        if (value) next.add(p);
        else next.delete(p);
      }
      return next;
    });
  }, []);

  if (!folder) return <PanePlaceholder label="folder gone" focused={focused} onRemove={onClose} />;

  return (
    <div
      className={cn(
        "flex h-full flex-col overflow-hidden rounded-lg border bg-card",
        focused && "border-violet-500/60",
      )}
    >
      <PaneChrome
        lens={<PaneLens kind="diff" />}
        controls={
          <>
            {ideConnected && <ClaudeBadge />}
            {editingBase ? (
              <input
                autoFocus
                defaultValue={baseBranch ?? ""}
                placeholder={
                  taskBaseBranch
                    ? `branch to compare against (blank = this task's base, "${taskBaseBranch}")`
                    : "branch to compare against (blank = auto-detect main)"
                }
                onBlur={(e) => void commitBaseBranch(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter")
                    void commitBaseBranch((e.target as HTMLInputElement).value);
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
                  taskBaseBranch
                    ? `set the parent branch this folder compares against (default: this task's base, "${taskBaseBranch}")`
                    : "set the parent branch this folder compares against (default: origin/main)"
                }
                onClick={() => setEditingBase(true)}
                className="hover:text-sky-500"
              >
                <Pencil className="size-3" />
              </IconBtn>
            )}
          </>
        }
        actions={
          <>
            <IconBtn
              title="refresh diff"
              onClick={() => void fetchDiff()}
              className="hover:text-sky-500"
            >
              <RefreshCw className={refreshing ? "size-3 animate-spin" : "size-3"} />
            </IconBtn>
            <IconBtn
              title="close pane (diff stays a click away on the folder)"
              onClick={onClose}
              className="hover:text-sky-500"
            >
              <X className="size-3" />
            </IconBtn>
          </>
        }
      />
      <div className="relative flex min-h-0 flex-1 p-2">
        {files == null ? (
          <p className="p-2 text-sm text-muted-foreground">Loading…</p>
        ) : files.length === 0 ? (
          <p className="p-2 text-sm text-muted-foreground">No changes.</p>
        ) : (
          <>
            <DiffTreeRail
              files={files}
              reviewed={reviewed}
              dirty={dirty}
              conflict={conflict}
              onJump={jumpTo}
              onToggleReviewed={toggleReviewed}
              onToggleReviewedMany={toggleReviewedMany}
            />
            <div className="min-w-0 flex-1">
              <MonacoMultiDiff
                dir={dir!}
                files={files}
                mode={mode}
                baseBranch={baseBranch}
                refreshKey={statsKey}
                baseKey={baseKey}
                connected={ideConnected}
                registerReveal={registerReveal}
                reviewed={reviewed}
                onToggleReviewed={toggleReviewed}
                onDirtyChange={handleDirtyChange}
                onConflictChange={handleConflictChange}
              />
            </div>
          </>
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
