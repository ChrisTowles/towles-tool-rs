import { useEffect, useMemo, useRef, useState } from "react";
import { ChevronRight, Columns2, Rows2, X } from "lucide-react";
import {
  buildDiffTree,
  pairDiffLines,
  parseDiff,
  type DiffFile,
  type DiffLine,
  type DiffTreeNode,
} from "@/lib/diff";
import { fileIconSpec, folderIconSpec } from "@/lib/file-icons";
import { cn } from "@/lib/utils";

/**
 * Plannotator-style diff viewer: a file rail (change-type letter · path ·
 * right-anchored ± counts) next to the selected file's patch. Colors are
 * fixed for light + dark via explicit dark: variants — never raw green/red
 * text on its own tinted background, which went unreadable in light mode.
 *
 * When an [`DiffIdeBridge`] is provided (the Agentboard diff pane), the
 * unified view grows a clickable line-number gutter: click / shift-click /
 * drag selects a new-file line range, which rides to the folder's Claude Code
 * session as its selection context (see docs/CLAUDE-CODE-IDE.md). Violet is
 * the app's agent/active accent — the highlight is a claim that an agent is
 * looking at these lines.
 */

/** How the viewer talks to the per-folder IDE servers. Lines are 1-based
 * inclusive positions in the post-change file. */
export type DiffIdeBridge = {
  /** A Claude Code CLI is connected in this folder right now. */
  connected: boolean;
  /** Debounced ambient highlight (mirrors VS Code's selection_changed). */
  select: (filePath: string, startLine: number, endLine: number) => void;
  /** The highlight was dismissed. */
  clear: (filePath: string) => void;
  /** Explicit @-mention ("send to Claude"). */
  send: (filePath: string, startLine: number, endLine: number) => void;
};

/** A selected new-file line range, 1-based inclusive, start <= end. */
type LineSel = { start: number; end: number };

/** Leading change-type letter, fixed slot so paths align (plannotator's
 * FileRowBits language: A/D/R carry weight + color, M stays whisper-quiet). */
function ChangeTypeLetter({ file }: { file: DiffFile }) {
  const map = {
    added: { ch: "A", cls: "font-semibold text-emerald-600 dark:text-emerald-400" },
    deleted: { ch: "D", cls: "font-semibold text-red-600 dark:text-red-400" },
    renamed: { ch: "R", cls: "font-semibold text-sky-600 dark:text-sky-400" },
    modified: { ch: "M", cls: "text-muted-foreground/50" },
  } as const;
  const { ch, cls } = map[file.status];
  const title =
    file.status === "renamed" && file.oldPath
      ? `Renamed from ${file.oldPath}`
      : `${file.status[0].toUpperCase()}${file.status.slice(1)} file`;
  return (
    <span className={cn("w-3 shrink-0 text-center font-mono text-[10px]", cls)} title={title}>
      {ch}
    </span>
  );
}

/** Right-anchored ± pair in one fixed-width block, so counts end flush at the
 * row edge and add-only rows leave no phantom gap. */
function DiffCounts({ additions, deletions }: { additions: number; deletions: number }) {
  return (
    <span className="min-w-[7ch] shrink-0 whitespace-nowrap text-right font-mono text-[10px] tabular-nums">
      {additions > 0 && <span className="text-emerald-600 dark:text-emerald-400">+{additions}</span>}
      {additions > 0 && deletions > 0 && <span> </span>}
      {deletions > 0 && <span className="text-red-600 dark:text-red-400">−{deletions}</span>}
    </span>
  );
}

/** Per-level left inset for the file rail's tree rows; folders and their
 * files share the same ladder so a folder's children visibly nest under it. */
const TREE_INDENT_PX = 14;
const TREE_BASE_PX = 8;

function DiffTreeRows({
  nodes,
  depth,
  files,
  selected,
  onSelect,
  collapsed,
  onToggleFolder,
}: {
  nodes: DiffTreeNode[];
  depth: number;
  files: DiffFile[];
  selected: number;
  onSelect: (index: number) => void;
  collapsed: Set<string>;
  onToggleFolder: (path: string) => void;
}) {
  return (
    <>
      {nodes.map((node) => {
        const paddingLeft = TREE_BASE_PX + depth * TREE_INDENT_PX;
        if (node.kind === "folder") {
          const isCollapsed = collapsed.has(node.path);
          const folder = folderIconSpec(node.name, !isCollapsed);
          return (
            <div key={node.path}>
              <button
                type="button"
                onClick={() => onToggleFolder(node.path)}
                style={{ paddingLeft }}
                className="flex w-full items-center gap-1.5 py-1 pr-2 text-left text-[11px] font-medium text-muted-foreground hover:bg-accent/50"
              >
                <ChevronRight
                  className={cn(
                    "size-3 shrink-0 text-muted-foreground/70 transition-transform",
                    !isCollapsed && "rotate-90",
                  )}
                />
                <folder.Icon className={cn("size-3.5 shrink-0", folder.className)} />
                <span className="truncate">{node.name}</span>
              </button>
              {!isCollapsed && (
                <DiffTreeRows
                  nodes={node.children}
                  depth={depth + 1}
                  files={files}
                  selected={selected}
                  onSelect={onSelect}
                  collapsed={collapsed}
                  onToggleFolder={onToggleFolder}
                />
              )}
            </div>
          );
        }

        const file = files[node.index];
        const icon = fileIconSpec(node.name);
        return (
          <button
            key={node.path}
            type="button"
            onClick={() => onSelect(node.index)}
            style={{ paddingLeft }}
            className={cn(
              "flex w-full items-center gap-1.5 border-l-2 border-transparent py-1.5 pr-2 text-left text-xs",
              node.index === selected
                ? "border-l-violet-500 bg-accent text-foreground"
                : "text-muted-foreground hover:bg-accent/50",
            )}
          >
            <ChangeTypeLetter file={file} />
            <icon.Icon className={cn("size-3.5 shrink-0", icon.className)} />
            <span className="min-w-0 flex-1 truncate">{node.name}</span>
            <DiffCounts additions={file.additions} deletions={file.deletions} />
          </button>
        );
      })}
    </>
  );
}

const LINE_CLS: Record<DiffLine["kind"], string> = {
  add: "bg-emerald-500/10 text-emerald-700 dark:text-emerald-300",
  del: "bg-red-500/10 text-red-700 dark:text-red-300",
  hunk: "bg-sky-500/10 text-sky-700 dark:text-sky-300",
  meta: "text-muted-foreground/70",
  ctx: "text-foreground/80",
};

/** Whether this line exists in the post-change file (what a highlight can
 * anchor to — Claude reads the working tree, where deleted lines are gone). */
function selectable(line: DiffLine): boolean {
  return line.newLine != null;
}

function inSel(line: DiffLine, sel: LineSel | null): boolean {
  return sel != null && line.newLine != null && line.newLine >= sel.start && line.newLine <= sel.end;
}

/** One gutter cell. Interactive (pointer + hover) only when the row is
 * selectable and the viewer has an IDE bridge to feed. */
function GutterNo({
  n,
  interactive,
  onMouseDown,
}: {
  n: number | undefined;
  interactive: boolean;
  onMouseDown?: (e: React.MouseEvent) => void;
}) {
  return (
    <span
      onMouseDown={onMouseDown}
      title={
        interactive
          ? "Click to select this line for Claude · shift-click or drag to extend"
          : undefined
      }
      className={cn(
        "w-9 shrink-0 pr-1.5 text-right tabular-nums select-none",
        "text-muted-foreground/40",
        interactive && "cursor-pointer hover:text-foreground",
      )}
    >
      {n ?? " "}
    </span>
  );
}

function FilePatch({
  file,
  sel,
  onBegin,
  onDrag,
}: {
  file: DiffFile;
  sel: LineSel | null;
  /** Gutter mousedown on a selectable row (shift extends from the anchor). */
  onBegin?: (line: number, extend: boolean) => void;
  /** Mouse entered a selectable row while dragging. */
  onDrag?: (line: number) => void;
}) {
  return (
    <pre className="min-w-max p-2 font-mono text-xs leading-relaxed whitespace-pre">
      {file.lines.map((line, i) => {
        const canSelect = onBegin != null && selectable(line);
        const begin = canSelect
          ? (e: React.MouseEvent) => {
              e.preventDefault(); // no text-selection fight while dragging
              onBegin(line.newLine!, e.shiftKey);
            }
          : undefined;
        return (
          <div
            key={i}
            onMouseEnter={
              onDrag != null && selectable(line) ? () => onDrag(line.newLine!) : undefined
            }
            className={cn(
              "flex",
              LINE_CLS[line.kind],
              inSel(line, sel) && "bg-violet-500/20",
            )}
          >
            <GutterNo n={line.oldLine} interactive={canSelect} onMouseDown={begin} />
            <GutterNo n={line.newLine} interactive={canSelect} onMouseDown={begin} />
            <span className="px-2">{line.text || " "}</span>
          </div>
        );
      })}
    </pre>
  );
}

/** Split-view cell: blank (no counterpart on this side) renders as an empty,
 * unhighlighted gutter rather than matching the other side's color. Lines
 * wrap (rather than the unified view's horizontal scroll) so overflow can't
 * bleed past the 50% column into its sibling. */
function SplitCell({ line, sel }: { line: DiffLine | null; sel: LineSel | null }) {
  return (
    <div
      className={cn(
        "min-w-0 flex-1 px-2 break-all whitespace-pre-wrap",
        line ? LINE_CLS[line.kind] : "",
        line && inSel(line, sel) && "bg-violet-500/20",
      )}
    >
      {line ? line.text || " " : " "}
    </div>
  );
}

/** Split view shows an active highlight but doesn't create one — range
 * selection lives in the unified view's gutter. */
function SplitFilePatch({ file, sel }: { file: DiffFile; sel: LineSel | null }) {
  const rows = useMemo(() => pairDiffLines(file.lines), [file]);
  return (
    <pre className="p-2 font-mono text-xs leading-relaxed">
      {rows.map((row, i) =>
        "full" in row ? (
          <div
            key={i}
            className={cn(
              "px-2 break-all whitespace-pre-wrap",
              LINE_CLS[row.full.kind],
              inSel(row.full, sel) && "bg-violet-500/20",
            )}
          >
            {row.full.text || " "}
          </div>
        ) : (
          <div key={i} className="flex items-stretch">
            <SplitCell line={row.left} sel={sel} />
            <div className="w-px shrink-0 self-stretch bg-border/70" />
            <SplitCell line={row.right} sel={sel} />
          </div>
        ),
      )}
    </pre>
  );
}

/** Floating summary of the active highlight: the range, whether a Claude
 * session is live for it, and the explicit "send" affordance. */
function SelectionChip({
  sel,
  connected,
  onSend,
  onClear,
}: {
  sel: LineSel;
  connected: boolean;
  onSend: () => void;
  onClear: () => void;
}) {
  const range = sel.start === sel.end ? `L${sel.start}` : `L${sel.start}–${sel.end}`;
  return (
    <div className="absolute right-3 bottom-3 z-10 flex max-w-[calc(100%-1.5rem)] items-center gap-2 rounded-md border border-border bg-card px-2 py-1 whitespace-nowrap shadow-md">
      <span className="font-mono text-xs text-violet-500">✦</span>
      <span className="font-mono text-[11px] text-foreground tabular-nums">{range}</span>
      <span className="truncate text-[11px] text-muted-foreground">
        {connected ? "live to claude" : "no claude connected"}
      </span>
      <button
        type="button"
        title={
          connected
            ? "Insert an @file#range reference into the Claude session's prompt"
            : "Run `claude` in this folder's terminal to connect it"
        }
        disabled={!connected}
        onClick={onSend}
        className={cn(
          "shrink-0 rounded-sm px-1.5 py-0.5 text-[11px] font-medium",
          connected
            ? "text-violet-500 hover:bg-accent"
            : "cursor-not-allowed text-muted-foreground/50",
        )}
      >
        @ send
      </button>
      <button
        type="button"
        title="Clear the highlight (Esc)"
        onClick={onClear}
        className="text-muted-foreground hover:text-foreground"
      >
        <X className="size-3" />
      </button>
    </div>
  );
}

type ViewMode = "unified" | "split";

export function DiffViewer({ text, ide }: { text: string; ide?: DiffIdeBridge }) {
  const files = useMemo(() => parseDiff(text), [text]);
  const tree = useMemo(() => buildDiffTree(files), [files]);
  const [selected, setSelected] = useState(0);
  const [viewMode, setViewMode] = useState<ViewMode>("unified");
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set());
  // Line highlight (new-file coordinates) + drag bookkeeping. Refs, not
  // state: anchor/drag change on every mousemove and never affect rendering
  // beyond the derived `sel`.
  const [sel, setSel] = useState<LineSel | null>(null);
  const anchorRef = useRef<number | null>(null);
  const draggingRef = useRef(false);
  const lastPushedPath = useRef<string | null>(null);
  // A new diff (dialog re-opened for another folder) resets selection + tree state.
  useEffect(() => {
    setSelected(0);
    setCollapsed(new Set());
  }, [text]);
  // Switching file or refreshing the diff drops the highlight (its line
  // numbers no longer mean anything).
  useEffect(() => {
    setSel(null);
    anchorRef.current = null;
  }, [text, selected]);
  const toggleFolder = (path: string) =>
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  const file = files[Math.min(selected, files.length - 1)];
  const headerIcon = file ? fileIconSpec(file.path) : null;

  // Debounced push of the highlight to the folder's Claude session (VS Code
  // debounces selection_changed 300ms; match it). Clearing pushes an empty
  // selection so stale context never rides the next prompt.
  useEffect(() => {
    if (!ide) return;
    if (sel && file) {
      const timer = setTimeout(() => {
        ide.select(file.path, sel.start, sel.end);
        lastPushedPath.current = file.path;
      }, 300);
      return () => clearTimeout(timer);
    }
    if (lastPushedPath.current) {
      ide.clear(lastPushedPath.current);
      lastPushedPath.current = null;
    }
  }, [ide, sel, file]);

  // Drag ends anywhere in the window; Esc clears the highlight.
  useEffect(() => {
    const onUp = () => {
      draggingRef.current = false;
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setSel(null);
    };
    window.addEventListener("mouseup", onUp);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mouseup", onUp);
      window.removeEventListener("keydown", onKey);
    };
  }, []);

  const beginSelect = (line: number, extend: boolean) => {
    if (extend && anchorRef.current != null) {
      const anchor = anchorRef.current;
      setSel({ start: Math.min(anchor, line), end: Math.max(anchor, line) });
      return;
    }
    anchorRef.current = line;
    draggingRef.current = true;
    setSel({ start: line, end: line });
  };
  const dragSelect = (line: number) => {
    if (!draggingRef.current || anchorRef.current == null) return;
    const anchor = anchorRef.current;
    setSel({ start: Math.min(anchor, line), end: Math.max(anchor, line) });
  };

  if (files.length === 0) {
    return <p className="p-4 text-sm text-muted-foreground">No changes.</p>;
  }

  return (
    <div className="flex min-h-0 flex-1 overflow-hidden rounded-lg border">
      <div className="flex w-64 shrink-0 flex-col overflow-y-auto border-r bg-card">
        <div className="sticky top-0 flex items-center justify-between border-b bg-card px-3 py-1.5">
          <span className="text-[11px] font-medium text-muted-foreground">
            {files.length} file{files.length === 1 ? "" : "s"}
          </span>
          <DiffCounts
            additions={files.reduce((s, f) => s + f.additions, 0)}
            deletions={files.reduce((s, f) => s + f.deletions, 0)}
          />
        </div>
        <DiffTreeRows
          nodes={tree}
          depth={0}
          files={files}
          selected={selected}
          onSelect={setSelected}
          collapsed={collapsed}
          onToggleFolder={toggleFolder}
        />
      </div>
      <div className="flex min-w-0 flex-1 flex-col">
        <div className="flex shrink-0 items-center gap-2 border-b bg-card px-3 py-1.5">
          <ChangeTypeLetter file={file} />
          {headerIcon && <headerIcon.Icon className={cn("size-3.5 shrink-0", headerIcon.className)} />}
          <span className="min-w-0 truncate font-mono text-xs text-foreground">{file.path}</span>
          {file.oldPath && (
            <span className="truncate font-mono text-[10.5px] text-muted-foreground">
              ← {file.oldPath}
            </span>
          )}
          <span className="ml-auto flex shrink-0 items-center rounded-md border border-border/70 p-0.5">
            <button
              type="button"
              title="Unified view"
              aria-pressed={viewMode === "unified"}
              onClick={() => setViewMode("unified")}
              className={cn(
                "rounded-[5px] p-1 transition-colors",
                viewMode === "unified"
                  ? "bg-accent text-foreground"
                  : "text-muted-foreground hover:text-foreground",
              )}
            >
              <Rows2 className="size-3" />
            </button>
            <button
              type="button"
              title="Split view"
              aria-pressed={viewMode === "split"}
              onClick={() => setViewMode("split")}
              className={cn(
                "rounded-[5px] p-1 transition-colors",
                viewMode === "split"
                  ? "bg-accent text-foreground"
                  : "text-muted-foreground hover:text-foreground",
              )}
            >
              <Columns2 className="size-3" />
            </button>
          </span>
          <DiffCounts additions={file.additions} deletions={file.deletions} />
        </div>
        <div className="relative min-h-0 flex-1">
          <div className="h-full overflow-auto">
            {viewMode === "split" ? (
              <SplitFilePatch file={file} sel={sel} />
            ) : (
              <FilePatch
                file={file}
                sel={sel}
                onBegin={ide ? beginSelect : undefined}
                onDrag={ide ? dragSelect : undefined}
              />
            )}
          </div>
          {ide && sel && (
            <SelectionChip
              sel={sel}
              connected={ide.connected}
              onSend={() => ide.send(file.path, sel.start, sel.end)}
              onClear={() => setSel(null)}
            />
          )}
          {/* Discoverability: with a live claude and no highlight yet, say how. */}
          {ide?.connected && !sel && viewMode === "unified" && (
            <div className="pointer-events-none absolute right-3 bottom-3 z-10 flex max-w-[calc(100%-1.5rem)] items-center gap-1.5 rounded-md border border-violet-500/50 bg-violet-500/10 px-2 py-1 whitespace-nowrap">
              <span className="font-mono text-xs text-violet-500">✦</span>
              <span className="truncate text-[11px] text-violet-500">
                claude is connected — click a line number to share lines
              </span>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
