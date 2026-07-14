import { useCallback, useEffect, useMemo, useState } from "react";
import { AtSign, ChevronDown, ChevronRight, File, Folder, RefreshCw } from "lucide-react";
import { IconBtn } from "@/components/agentboard-bits";
import { buildDiffTree, type DiffFile, type DiffTreeNode } from "@/lib/diff";
import { ideAtMention } from "@/lib/ide";
import { invokeCmd } from "@/lib/tauri";
import { cn } from "@/lib/utils";

/**
 * The diff pane's "Files" tab: every file in the checkout (tracked +
 * untracked-not-ignored, via `ide_list_files`), not just what changed — so any
 * file can be @-mentioned into the folder's Claude session. A filter box
 * flattens the tree into matches; without it, the same compact-folders tree
 * the Changes tab uses.
 */

/** Wrap plain paths in the shape `buildDiffTree` groups on. */
function stubFiles(paths: string[]): DiffFile[] {
  return paths.map((path) => ({ path, status: "modified", additions: 0, deletions: 0, lines: [] }));
}

const TREE_INDENT_PX = 14;
const TREE_BASE_PX = 8;
/** Filtered matches shown at most — typing narrows further. */
const FILTER_RESULT_CAP = 200;

/** One file row: name + an @ button that mentions it to the Claude session. */
function FileRow({
  name,
  path,
  paddingLeft,
  connected,
  onMention,
  showPath,
}: {
  name: string;
  path: string;
  paddingLeft: number;
  connected: boolean;
  onMention: (path: string) => void;
  /** Filter results show the full path (tree context is gone). */
  showPath?: boolean;
}) {
  return (
    <div
      style={{ paddingLeft }}
      className="group flex w-full items-center gap-1.5 py-1 pr-2 text-left text-xs text-muted-foreground hover:bg-accent/50"
    >
      <File className="size-3.5 shrink-0 text-muted-foreground/50" />
      <span className="min-w-0 flex-1 truncate" title={path}>
        {showPath ? path : name}
      </span>
      <button
        type="button"
        title={
          connected
            ? `Mention ${path} to the Claude session in this folder`
            : "Run `claude` in this folder's terminal first"
        }
        onClick={() => onMention(path)}
        className={cn(
          "flex shrink-0 items-center gap-0.5 rounded-sm px-1 py-0.5 font-mono text-[10.5px] opacity-0 transition-opacity group-hover:opacity-100",
          connected ? "text-violet-500 hover:bg-accent" : "text-muted-foreground/50",
        )}
      >
        <AtSign className="size-3" /> claude
      </button>
    </div>
  );
}

function FileTreeRows({
  nodes,
  depth,
  collapsed,
  onToggleFolder,
  connected,
  onMention,
}: {
  nodes: DiffTreeNode[];
  depth: number;
  collapsed: Set<string>;
  onToggleFolder: (path: string) => void;
  connected: boolean;
  onMention: (path: string) => void;
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
                <FileTreeRows
                  nodes={node.children}
                  depth={depth + 1}
                  collapsed={collapsed}
                  onToggleFolder={onToggleFolder}
                  connected={connected}
                  onMention={onMention}
                />
              )}
            </div>
          );
        }
        return (
          <FileRow
            key={node.path}
            name={node.name}
            path={node.path}
            paddingLeft={paddingLeft}
            connected={connected}
            onMention={onMention}
          />
        );
      })}
    </>
  );
}

export function FilesPane({ dir, connected }: { dir: string; connected: boolean }) {
  const [files, setFiles] = useState<string[] | null>(null);
  const [filter, setFilter] = useState("");
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set());
  const [refreshing, setRefreshing] = useState(false);

  const fetchFiles = useCallback(async () => {
    setRefreshing(true);
    const list = await invokeCmd<string[]>("ide_list_files", { dir });
    setFiles(list ?? []);
    setRefreshing(false);
  }, [dir]);

  useEffect(() => {
    void fetchFiles();
  }, [fetchFiles]);

  const tree = useMemo(() => buildDiffTree(stubFiles(files ?? [])), [files]);
  const needle = filter.trim().toLowerCase();
  const matches = useMemo(
    () =>
      needle
        ? (files ?? []).filter((f) => f.toLowerCase().includes(needle)).slice(0, FILTER_RESULT_CAP)
        : null,
    [files, needle],
  );

  const mention = (path: string) => void ideAtMention(dir, path);

  const toggleFolder = (path: string) =>
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });

  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-hidden rounded-lg border">
      <div className="flex shrink-0 items-center gap-2 border-b bg-card px-3 py-1.5">
        <input
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder="filter files…"
          className="min-w-0 flex-1 rounded-sm border border-input bg-background px-1.5 py-0.5 font-mono text-[11px] outline-none"
        />
        <span className="shrink-0 font-mono text-[10.5px] text-muted-foreground">
          {files == null ? "…" : matches ? `${matches.length} match` : `${files.length} files`}
        </span>
        <IconBtn title="refresh file list" onClick={() => void fetchFiles()} className="hover:text-sky-500">
          <RefreshCw className={refreshing ? "size-3 animate-spin" : "size-3"} />
        </IconBtn>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto bg-card">
        {files == null ? (
          <p className="p-3 text-sm text-muted-foreground">Loading…</p>
        ) : matches ? (
          matches.map((path) => (
            <FileRow
              key={path}
              name={path}
              path={path}
              paddingLeft={TREE_BASE_PX}
              connected={connected}
              onMention={mention}
              showPath
            />
          ))
        ) : (
          <FileTreeRows
            nodes={tree}
            depth={0}
            collapsed={collapsed}
            onToggleFolder={toggleFolder}
            connected={connected}
            onMention={mention}
          />
        )}
      </div>
      <div className="shrink-0 border-t bg-card px-3 py-1 text-[10.5px] text-muted-foreground">
        <span className="font-mono text-violet-500">@</span> sends the file into the Claude
        session's prompt{connected ? "" : " — no session connected yet"}
      </div>
    </div>
  );
}
