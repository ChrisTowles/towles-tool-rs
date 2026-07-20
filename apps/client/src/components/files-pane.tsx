import { useEffect, useRef, useState } from "react";
import {
  AtSign,
  Columns2,
  Files as FilesIcon,
  Maximize2,
  Minimize2,
  RefreshCw,
  WrapText,
} from "lucide-react";
import { CodeViewer, type ViewerAnchor } from "@/components/code-viewer";
import { ClaudeBadge, IconBtn, LspBadge, PanePlaceholder } from "@/components/agentboard-bits";
import { FilePreview, previewKindFor } from "@/components/file-preview";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import { ideMention, useIdeConnected } from "@/lib/ide";
import { useLspStatus } from "@/lib/lsp";
import {
  attachExplorer,
  runMonacoCommand,
  setMonacoOpenHandler,
  setMonacoWorkspace,
} from "@/lib/monaco";
import { uiAction } from "@/lib/ui-action";
import { cn } from "@/lib/utils";
import { folderStatsKey, type FolderData } from "@/lib/agentboard";

/**
 * The files pane: VS Code's real Explorer view on the left (the workbench
 * sidebar part, hosted via `attachExplorer` — the checkout is the workspace
 * folder), a Monaco viewer on the right. Clicking a file in the Explorer
 * routes through the views override's open fallback into the viewer;
 * selecting text streams to the folder's Claude session, and two gestures
 * mention explicitly: the header @ button sends the whole file, while the
 * viewer's selection chip (or ⌘⇧A) sends just the highlighted lines. Long
 * lines wrap by default (toggle in the
 * viewer toolbar); Markdown/HTML files get a second toggle that opens a
 * resizable split preview alongside the editor, and a third lifts the whole
 * pane out of the Agentboard tiling to fill the viewport (Escape returns it).
 */

/** Claude called openFile — focus this file (new nonce per request). */
export type FilesOpenRequest = { path: string; anchor: ViewerAnchor; nonce: number };

/** Silent unless the bridge has something to say (a non-Rust checkout). */
function LspChip({ dir }: { dir: string }) {
  const { state, detail } = useLspStatus(dir);
  if (state === "off") return null;
  return <LspBadge state={state} detail={detail} />;
}

export function FilesPane({
  dir,
  connected,
  openRequest,
}: {
  dir: string;
  connected: boolean;
  openRequest?: FilesOpenRequest;
}) {
  const [open, setOpen] = useState<string | null>(null);
  const [dirty, setDirty] = useState(false);
  const [wordWrap, setWordWrap] = useState(true);
  const [previewOpen, setPreviewOpen] = useState(false);
  const [fullscreen, setFullscreen] = useState(false);
  const explorerRef = useRef<HTMLDivElement>(null);

  // Reset on a genuine dir *change* only — skipping the initial mount keeps
  // this independent of the openRequest effect below, whose request may have
  // just created this pane (a mount-time reset would clobber it).
  const prevDirRef = useRef(dir);
  useEffect(() => {
    if (prevDirRef.current === dir) return;
    prevDirRef.current = dir;
    setOpen(null);
    setDirty(false);
  }, [dir]);

  useEffect(() => {
    if (openRequest) setOpen(openRequest.path);
  }, [openRequest]);

  // A newly-opened file starts with the preview pane closed — it only makes
  // sense for the file that was previewable, not whatever's opened next.
  useEffect(() => {
    setPreviewOpen(false);
  }, [open]);

  // The only way back out of fullscreen is the header toggle, and the header
  // only renders while a file is open — so closing the file has to drop it too,
  // or the pane is stuck covering the viewport with no affordance.
  useEffect(() => {
    if (!open) setFullscreen(false);
  }, [open]);

  // Escape leaves fullscreen, mirroring zen mode in `App.tsx`. Bubble phase and
  // the `defaultPrevented` check are what keep this from stealing the key from
  // Monaco: the editor consumes Escape (closing its find widget, dismissing a
  // suggest popup) by preventing the default, so we only see the ones it
  // ignored. An open Radix dialog — including Monaco's own dialog host — closes
  // itself first for the same reason.
  useEffect(() => {
    if (!fullscreen) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      if (document.querySelector('[role="dialog"][data-state="open"]')) return;
      e.preventDefault();
      setFullscreen(false);
      uiAction("files.fullscreen", "agentboard", "escape");
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [fullscreen]);

  const previewKind = open ? previewKindFor(open) : null;

  // This pane is the VS Code workspace: the Explorer sidebar renders into
  // this pane's container, quick-open (Ctrl+P in the editor) searches this
  // folder, and picked/clicked files open here. Keyed on `open` too — panes
  // stay mounted forever, so mount order says nothing about which pane the
  // user is in; the one they last opened a file in wins (workspace, sidebar,
  // and open-handler all steal together).
  useEffect(() => {
    let disposed = false;
    let detach: (() => void) | null = null;
    setMonacoWorkspace(dir).catch((e: unknown) => {
      console.error("[files] failed to set the VS Code workspace", e);
    });
    if (explorerRef.current) {
      attachExplorer(explorerRef.current)
        .then((d) => {
          if (disposed) d();
          else detach = d;
        })
        .catch((e: unknown) => {
          console.error("[files] failed to attach the Explorer", e);
        });
    }
    setMonacoOpenHandler((absolutePath) => {
      if (absolutePath.startsWith(`${dir}/`)) setOpen(absolutePath.slice(dir.length + 1));
    });
    return () => {
      disposed = true;
      detach?.();
      setMonacoOpenHandler(null);
    };
  }, [dir, open]);

  // Whole-file mention. A range mention is the viewer's own gesture (select
  // lines, then the chip's @ send or ⌘⇧A) — it needs the live selection, which
  // only the editor has.
  const mention = (path: string) => void ideMention(dir, path, null);

  return (
    <div
      className={cn(
        "flex min-h-0 flex-1 overflow-hidden rounded-lg border",
        // Fullscreen leaves the pane in the React tree — portalling it would
        // remount `CodeViewer`, and rebuilding the Monaco model throws away the
        // undo stack and scroll position. Nothing in the Agentboard tiling is
        // transformed, so `fixed` still resolves against the viewport here.
        fullscreen && "fixed inset-0 z-50 rounded-none border-0 bg-background",
      )}
    >
      <div className="flex w-64 shrink-0 flex-col border-r bg-card">
        <div className="flex shrink-0 items-center gap-1.5 border-b bg-card px-2 py-1.5">
          <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-muted-foreground">
            explorer
          </span>
          <LspChip dir={dir} />
          <IconBtn
            title="refresh the explorer"
            onClick={() => void runMonacoCommand("workbench.files.action.refreshFilesExplorer")}
            className="hover:text-sky-500"
          >
            <RefreshCw className="size-3" />
          </IconBtn>
        </div>
        <div ref={explorerRef} className="min-h-0 flex-1 overflow-hidden" />
        <div className="shrink-0 border-t bg-card px-2 py-1 text-[10.5px] text-muted-foreground">
          <span className="font-mono text-violet-500">@</span> mentions the open file — select lines
          and ⌘⇧A to mention a range
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
                  title="Unsaved changes — autosaves after a pause; ⌘S saves now"
                  className="size-1.5 shrink-0 rounded-full bg-amber-500"
                />
              )}
              <span className="shrink-0 text-[10.5px] text-muted-foreground">
                editable · autosaves
              </span>
              <IconBtn
                title={
                  wordWrap
                    ? "Wrapping long lines — click to scroll instead"
                    : "Scrolling long lines — click to wrap instead"
                }
                onClick={() => setWordWrap((w) => !w)}
                className={cn("ml-auto", wordWrap && "text-violet-500")}
              >
                <WrapText className="size-3.5" />
              </IconBtn>
              {previewKind && (
                <IconBtn
                  title={previewOpen ? "Close preview" : `Open a ${previewKind} preview`}
                  onClick={() => {
                    setPreviewOpen((p) => !p);
                    uiAction("files.preview", "agentboard", previewOpen ? "off" : "on");
                  }}
                  className={previewOpen ? "text-violet-500" : undefined}
                >
                  <Columns2 className="size-3.5" />
                </IconBtn>
              )}
              <IconBtn
                title={fullscreen ? "Exit fullscreen (Escape)" : "Fill the window"}
                onClick={() => {
                  setFullscreen((f) => !f);
                  uiAction("files.fullscreen", "agentboard", fullscreen ? "off" : "on");
                }}
                className={fullscreen ? "text-violet-500" : undefined}
              >
                {fullscreen ? (
                  <Minimize2 className="size-3.5" />
                ) : (
                  <Maximize2 className="size-3.5" />
                )}
              </IconBtn>
              <button
                type="button"
                title={
                  connected
                    ? "Mention this whole file to the Claude session — select lines and press ⌘⇧A to mention just those"
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
                      connected={connected}
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
                  connected={connected}
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
  focused,
  onClose,
  openRequest,
}: {
  /** The checkout this pane browses; undefined when it left the rail. */
  folder: FolderData | undefined;
  /** This pane is the one the user last clicked into — see the focus-ring
   * rule in `screens/agentboard.tsx`'s `focusedPaneId`. */
  focused: boolean;
  /** Removes the pane from its window. */
  onClose: () => void;
  /** Claude called openFile — focus this file (new nonce per request). */
  openRequest?: FilesOpenRequest;
}) {
  const ideConnected = useIdeConnected(folder?.dir);

  // The Explorer's provider has no disk watch (see `lib/monaco-fs.ts`), so a
  // file an agent creates or deletes never appears on its own. The folder's
  // git stats (1.5s poll, bumped only on real change) notice exactly that —
  // refresh the tree when they move. Keyed on the *full* stats: a
  // count-only key misses membership changes that keep the count, like an
  // already-modified file being deleted (M→D) or renamed. Line counts also
  // move on every auto-saved edit, though, so the refresh is throttled — at
  // most one tree re-enumeration per window, trailing so the last change
  // always lands. Skipped on mount: the Explorer just enumerated the disk.
  // This pane only renders for the active folder, so the workspace the
  // refresh command hits is (at worst briefly) this one.
  const statsKey = folder ? folderStatsKey(folder) : "";
  const statsSeen = useRef(false);
  const lastRefreshAtRef = useRef(0);
  const pendingRefreshRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => {
    if (!statsKey) return;
    if (!statsSeen.current) {
      statsSeen.current = true;
      return;
    }
    const THROTTLE_MS = 5000;
    const refresh = () => {
      lastRefreshAtRef.current = Date.now();
      void runMonacoCommand("workbench.files.action.refreshFilesExplorer");
    };
    clearTimeout(pendingRefreshRef.current);
    const wait = lastRefreshAtRef.current + THROTTLE_MS - Date.now();
    if (wait <= 0) refresh();
    else pendingRefreshRef.current = setTimeout(refresh, wait);
  }, [statsKey]);
  useEffect(() => () => clearTimeout(pendingRefreshRef.current), []);
  if (!folder) return <PanePlaceholder label="folder gone" focused={focused} onRemove={onClose} />;
  return (
    <div
      className={cn(
        "flex h-full flex-col overflow-hidden rounded-lg border bg-card",
        focused && "border-violet-500/60",
      )}
    >
      <div className="flex shrink-0 items-center gap-2 border-b bg-card px-2 py-1">
        <FilesIcon className="size-3.5 shrink-0 text-muted-foreground" />
        <span className="truncate font-mono text-xs text-foreground">{folder.name}</span>
        {ideConnected && <ClaudeBadge />}
        <span className="ml-auto flex shrink-0 items-center gap-1.5">
          <IconBtn
            title="remove pane (files stay a click away on the folder)"
            onClick={onClose}
            className="hover:text-red-500"
          >
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
