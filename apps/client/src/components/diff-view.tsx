import { useEffect, useMemo, useState } from "react";
import { ChevronDown, ChevronRight, Columns2, Folder, Rows2 } from "lucide-react";
import {
  buildDiffTree,
  pairDiffLines,
  parseDiff,
  type DiffFile,
  type DiffLine,
  type DiffTreeNode,
} from "@/lib/diff";
import { cn } from "@/lib/utils";

/**
 * Plannotator-style diff viewer: a file rail (change-type letter · path ·
 * right-anchored ± counts) next to the selected file's patch. Colors are
 * fixed for light + dark via explicit dark: variants — never raw green/red
 * text on its own tinted background, which went unreadable in light mode.
 */

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
          return (
            <div key={node.path}>
              <button
                type="button"
                onClick={() => onToggleFolder(node.path)}
                style={{ paddingLeft }}
                className="flex w-full items-center gap-1.5 py-1 pr-2 text-left text-[11px] font-medium text-muted-foreground hover:bg-accent/50"
              >
                {isCollapsed ? (
                  <ChevronRight className="size-3 shrink-0 text-muted-foreground/70" />
                ) : (
                  <ChevronDown className="size-3 shrink-0 text-muted-foreground/70" />
                )}
                <Folder className="size-3.5 shrink-0 text-muted-foreground/70" />
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
        return (
          <button
            key={node.path}
            type="button"
            onClick={() => onSelect(node.index)}
            style={{ paddingLeft }}
            className={cn(
              "flex w-full items-center gap-2 border-l-2 border-transparent py-1.5 pr-2 text-left text-xs",
              node.index === selected
                ? "border-l-violet-500 bg-accent text-foreground"
                : "text-muted-foreground hover:bg-accent/50",
            )}
          >
            <ChangeTypeLetter file={file} />
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

function FilePatch({ file }: { file: DiffFile }) {
  return (
    <pre className="min-w-max p-2 font-mono text-xs leading-relaxed whitespace-pre">
      {file.lines.map((line, i) => (
        <div key={i} className={cn("px-2", LINE_CLS[line.kind])}>
          {line.text || " "}
        </div>
      ))}
    </pre>
  );
}

/** Split-view cell: blank (no counterpart on this side) renders as an empty,
 * unhighlighted gutter rather than matching the other side's color. Lines
 * wrap (rather than the unified view's horizontal scroll) so overflow can't
 * bleed past the 50% column into its sibling. */
function SplitCell({ line }: { line: DiffLine | null }) {
  return (
    <div
      className={cn(
        "min-w-0 flex-1 px-2 break-all whitespace-pre-wrap",
        line ? LINE_CLS[line.kind] : "",
      )}
    >
      {line ? line.text || " " : " "}
    </div>
  );
}

function SplitFilePatch({ file }: { file: DiffFile }) {
  const rows = useMemo(() => pairDiffLines(file.lines), [file]);
  return (
    <pre className="p-2 font-mono text-xs leading-relaxed">
      {rows.map((row, i) =>
        "full" in row ? (
          <div
            key={i}
            className={cn("px-2 break-all whitespace-pre-wrap", LINE_CLS[row.full.kind])}
          >
            {row.full.text || " "}
          </div>
        ) : (
          <div key={i} className="flex items-stretch">
            <SplitCell line={row.left} />
            <div className="w-px shrink-0 self-stretch bg-border/70" />
            <SplitCell line={row.right} />
          </div>
        ),
      )}
    </pre>
  );
}

type ViewMode = "unified" | "split";

export function DiffViewer({ text }: { text: string }) {
  const files = useMemo(() => parseDiff(text), [text]);
  const tree = useMemo(() => buildDiffTree(files), [files]);
  const [selected, setSelected] = useState(0);
  const [viewMode, setViewMode] = useState<ViewMode>("unified");
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set());
  // A new diff (dialog re-opened for another folder) resets selection + tree state.
  useEffect(() => {
    setSelected(0);
    setCollapsed(new Set());
  }, [text]);
  const toggleFolder = (path: string) =>
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  const file = files[Math.min(selected, files.length - 1)];

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
        <div className="min-h-0 flex-1 overflow-auto">
          {viewMode === "split" ? <SplitFilePatch file={file} /> : <FilePatch file={file} />}
        </div>
      </div>
    </div>
  );
}
