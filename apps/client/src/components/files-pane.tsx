import { useCallback, useEffect, useMemo, useState } from "react";
import {
  AtSign,
  ChevronDown,
  ChevronRight,
  Columns2,
  File,
  Folder,
  RefreshCw,
  WrapText,
} from "lucide-react";
import { CodeViewer, type ViewerAnchor } from "@/components/code-viewer";
import { IconBtn } from "@/components/agentboard-bits";
import { FilePreview, previewKindFor } from "@/components/file-preview";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import { buildDiffTree, type DiffFile, type DiffTreeNode } from "@/lib/diff";
import { ideAtMention, useIdeConnected } from "@/lib/ide";
import { invokeCmd } from "@/lib/tauri";
import { cn } from "@/lib/utils";
import type { FolderData } from "@/lib/agentboard";
import { Files as FilesIcon } from "lucide-react";

/**
 * The files pane: every file in the checkout (tracked +
 * untracked-not-ignored, via `ide_list_files`), not just what changed. A
 * VS-Code-shaped split: file tree + filter on the left, a Monaco viewer on
 * the right. Clicking a file opens it; selecting text in the viewer streams
 * to the folder's Claude session as selection context, and the per-file @
 * button sends a whole-file mention. Long lines wrap by default (toggle in
 * the viewer toolbar); Markdown/HTML files get a second toggle that opens a
 * resizable split preview alongside the editor.
 */

/** Wrap plain paths in the shape `buildDiffTree` groups on. */
function stubFiles(paths: string[]): DiffFile[] {
  return paths.map((path) => ({ path, status: "modified", additions: 0, deletions: 0, lines: [] }));
}

const TREE_INDENT_PX = 14;
const TREE_BASE_PX = 8;
/** Filtered matches shown at most — typing narrows further. */
const FILTER_RESULT_CAP = 200;

/** One file row: opens in the viewer on click; @ mentions it to Claude. */
function FileRow({
  name,
  path,
  paddingLeft,
  connected,
  active,
  onOpen,
  onMention,
  showPath,
}: {
  name: string;
  path: string;
  paddingLeft: number;
  connected: boolean;
  active: boolean;
  onOpen: (path: string) => void;
  onMention: (path: string) => void;
  /** Filter results show the full path (tree context is gone). */
  showPath?: boolean;
}) {
  return (
    <div
      style={{ paddingLeft }}
      className={cn(
        "group flex w-full items-center gap-1.5 border-l-2 border-transparent py-1 pr-2 text-left text-xs",
        active
          ? "border-l-violet-500 bg-accent text-foreground"
          : "text-muted-foreground hover:bg-accent/50",
      )}
    >
      <File className="size-3.5 shrink-0 text-muted-foreground/50" />
      <button
        type="button"
        onClick={() => onOpen(path)}
        className="min-w-0 flex-1 truncate text-left"
        title={path}
      >
        {showPath ? path : name}
      </button>
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
  open,
  onOpen,
  onMention,
}: {
  nodes: DiffTreeNode[];
  depth: number;
  collapsed: Set<string>;
  onToggleFolder: (path: string) => void;
  connected: boolean;
  open: string | null;
  onOpen: (path: string) => void;
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
                  open={open}
                  onOpen={onOpen}
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
            active={open === node.path}
            onOpen={onOpen}
            onMention={onMention}
          />
        );
      })}
    </>
  );
}

/** Claude called openFile — focus this file (new nonce per request). */
export type FilesOpenRequest = { path: string; anchor: ViewerAnchor; nonce: number };

export function FilesPane({
  dir,
  connected,
  openRequest,
}: {
  dir: string;
  connected: boolean;
  openRequest?: FilesOpenRequest;
}) {
  const [files, setFiles] = useState<string[] | null>(null);
  const [filter, setFilter] = useState("");
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set());
  const [open, setOpen] = useState<string | null>(null);
  const [dirty, setDirty] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [wordWrap, setWordWrap] = useState(true);
  const [previewOpen, setPreviewOpen] = useState(false);

  useEffect(() => {
    if (openRequest) setOpen(openRequest.path);
  }, [openRequest]);

  // A newly-opened file starts with the preview pane closed — it only makes
  // sense for the file that was previewable, not whatever's opened next.
  useEffect(() => {
    setPreviewOpen(false);
  }, [open]);

  const previewKind = open ? previewKindFor(open) : null;

  const fetchFiles = useCallback(async () => {
    setRefreshing(true);
    const list = await invokeCmd<string[]>("ide_list_files", { dir });
    setFiles(list ?? []);
    setRefreshing(false);
  }, [dir]);

  useEffect(() => {
    setOpen(null);
    setDirty(false);
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
    <div className="flex min-h-0 flex-1 overflow-hidden rounded-lg border">
      <div className="flex w-64 shrink-0 flex-col border-r bg-card">
        <div className="flex shrink-0 items-center gap-1.5 border-b bg-card px-2 py-1.5">
          <input
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="filter files…"
            className="min-w-0 flex-1 rounded-sm border border-input bg-background px-1.5 py-0.5 font-mono text-[11px] outline-none"
          />
          <span className="shrink-0 font-mono text-[10.5px] text-muted-foreground">
            {files == null ? "…" : matches ? matches.length : files.length}
          </span>
          <IconBtn
            title="refresh file list"
            onClick={() => void fetchFiles()}
            className="hover:text-sky-500"
          >
            <RefreshCw className={refreshing ? "size-3 animate-spin" : "size-3"} />
          </IconBtn>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto">
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
                active={open === path}
                onOpen={setOpen}
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
              open={open}
              onOpen={setOpen}
              onMention={mention}
            />
          )}
        </div>
        <div className="shrink-0 border-t bg-card px-2 py-1 text-[10.5px] text-muted-foreground">
          <span className="font-mono text-violet-500">@</span> mentions a file to Claude
          {connected ? "" : " — no session connected yet"}
        </div>
      </div>
      <div className="flex min-w-0 flex-1 flex-col">
        {open ? (
          <>
            <div className="flex shrink-0 items-center gap-2 border-b bg-card px-3 py-1.5">
              <span className="min-w-0 truncate font-mono text-xs text-foreground" title={open}>
                {open}
              </span>
              {dirty && (
                <span
                  title="Unsaved changes — ⌘S saves"
                  className="size-1.5 shrink-0 rounded-full bg-amber-500"
                />
              )}
              <span className="shrink-0 text-[10.5px] text-muted-foreground">
                editable · ⌘S saves
              </span>
              <IconBtn
                title={wordWrap ? "Wrapping long lines — click to scroll instead" : "Scrolling long lines — click to wrap instead"}
                onClick={() => setWordWrap((w) => !w)}
                className={cn("ml-auto", wordWrap && "text-violet-500")}
              >
                <WrapText className="size-3.5" />
              </IconBtn>
              {previewKind && (
                <IconBtn
                  title={previewOpen ? "Close preview" : `Open a ${previewKind} preview`}
                  onClick={() => setPreviewOpen((p) => !p)}
                  className={previewOpen ? "text-violet-500" : undefined}
                >
                  <Columns2 className="size-3.5" />
                </IconBtn>
              )}
              <button
                type="button"
                title={
                  connected
                    ? "Mention this file to the Claude session (select text to share a range instead)"
                    : "Run `claude` in this folder's terminal first"
                }
                onClick={() => mention(open)}
                className={cn(
                  "flex shrink-0 items-center gap-0.5 rounded-sm px-1.5 py-0.5 font-mono text-[10.5px]",
                  connected ? "text-violet-500 hover:bg-accent" : "text-muted-foreground/50",
                )}
              >
                <AtSign className="size-3" /> send to claude
              </button>
            </div>
            <div className="min-h-0 flex-1">
              {previewOpen && previewKind ? (
                <ResizablePanelGroup orientation="horizontal">
                  <ResizablePanel defaultSize={50} minSize={20}>
                    <CodeViewer
                      dir={dir}
                      path={open}
                      wordWrap={wordWrap}
                      anchor={
                        openRequest && openRequest.path === open
                          ? { ...openRequest.anchor, nonce: openRequest.nonce }
                          : undefined
                      }
                      onDirtyChange={setDirty}
                    />
                  </ResizablePanel>
                  <ResizableHandle withHandle />
                  <ResizablePanel defaultSize={50} minSize={20}>
                    <FilePreview dir={dir} path={open} kind={previewKind} />
                  </ResizablePanel>
                </ResizablePanelGroup>
              ) : (
                <CodeViewer
                  dir={dir}
                  path={open}
                  wordWrap={wordWrap}
                  anchor={
                    openRequest && openRequest.path === open
                      ? { ...openRequest.anchor, nonce: openRequest.nonce }
                      : undefined
                  }
                  onDirtyChange={setDirty}
                />
              )}
            </div>
          </>
        ) : (
          <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
            Select a file — selections in the viewer stream to Claude
          </div>
        )}
      </div>
    </div>
  );
}

/**
 * A folder's file tree as a *pane* in the Agentboard tiling — the sibling of
 * `DiffPane`, sitting beside the live terminals. Claude's openFile requests
 * are routed here by the screen (which opens the pane when none exists yet)
 * via `openRequest`.
 */
export function FolderFilesPane({
  folder,
  onClose,
  openRequest,
}: {
  /** The checkout this pane browses; undefined when it left the rail. */
  folder: FolderData | undefined;
  /** Removes the pane from its window. */
  onClose: () => void;
  /** Claude called openFile — focus this file (new nonce per request). */
  openRequest?: FilesOpenRequest;
}) {
  const ideConnected = useIdeConnected(folder?.dir);
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
        <FilesIcon className="size-3.5 shrink-0 text-muted-foreground" />
        <span className="truncate font-mono text-xs text-foreground">{folder.name}</span>
        {ideConnected && (
          <span
            title="A Claude Code session in this folder is connected — highlighted lines become its selection context"
            className="flex shrink-0 items-center gap-1 rounded-md border border-violet-500/50 bg-violet-500/10 px-1.5 font-mono text-[10.5px] text-violet-500"
          >
            ✦ claude
          </span>
        )}
        <span className="ml-auto flex shrink-0 items-center gap-1.5">
          <IconBtn title="remove pane (files stay a click away on the folder)" onClick={onClose} className="hover:text-red-500">
            ⊟
          </IconBtn>
        </span>
      </div>
      <div className="flex min-h-0 flex-1 flex-col p-2">
        <FilesPane dir={folder.dir} connected={ideConnected} openRequest={openRequest} />
      </div>
    </div>
  );
}
