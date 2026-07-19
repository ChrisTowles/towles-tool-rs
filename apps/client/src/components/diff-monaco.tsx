import { useEffect, useRef, useState } from "react";
import { loadMonaco } from "@/lib/monaco";
import { invoke } from "@/lib/tauri";
import { errorMessage } from "@/lib/errors";
import {
  ideClearSelection,
  ideMention,
  ideReadFile,
  ideSetDiffDirty,
  ideSetSelection,
  saveModelBuffer,
} from "@/lib/ide";
import { IdeSelectionOverlay } from "@/components/ide-selection-chip";
import {
  diffWorkPath,
  mentionRangeFrom,
  sameMentionRange,
  streamRangeFrom,
  type MentionRange,
} from "@/lib/ide-selection";

/** One changed file from `ab_get_diff_files` (tt_agentboard::DiffFile). */
export type ChangedFile = {
  path: string;
  oldPath: string | null;
  /** Git name-status letter (M/A/D/R/C/T), or "?" for untracked. */
  status: string;
  linesAdded: number;
  linesRemoved: number;
};

type Widget =
  import("@codingame/monaco-vscode-api/vscode/vs/editor/browser/widget/multiDiffEditor/multiDiffEditorWidget").MultiDiffEditorWidget;
type ViewModel =
  import("@codingame/monaco-vscode-api/vscode/vs/editor/browser/widget/multiDiffEditor/multiDiffEditorViewModel").MultiDiffEditorViewModel;
type DocItem =
  import("@codingame/monaco-vscode-api/vscode/vs/editor/browser/widget/multiDiffEditor/multiDiffEditorViewModel").DocumentDiffItemViewModel;
type TextModel = import("monaco-editor").editor.ITextModel;

/**
 * VS Code's multi-diff editor over the whole change set: every file's diff
 * stacked in one scroll with sticky per-file headers, exactly the surface
 * VS Code uses for "view changes". Original sides come from the diff
 * baseline (`ab_get_base_file`), read-only — history isn't editable. Modified
 * sides come from the working tree and are editable in place: Cmd/Ctrl+S
 * saves the active file, same atomic/mtime-guarded write CodeViewer uses.
 * Selections on any modified side stream to the folder's Claude session, and
 * the selection chip (or ⌘⇧A) mentions those lines explicitly — same
 * contract as CodeViewer. `refreshKey` refetches contents in place when the
 * working tree measurably changes (skipped for a file with unsaved edits, so
 * an external change never clobbers them); the set of files changing rebuilds
 * the widget.
 */
export function MonacoMultiDiff({
  dir,
  files,
  mode,
  baseBranch,
  refreshKey,
  connected = false,
  registerReveal,
  reviewed,
  onToggleReviewed,
  onDirtyChange,
}: {
  dir: string;
  files: ChangedFile[];
  mode: string;
  baseBranch: string | null;
  refreshKey: string;
  /** A Claude session is live in this folder — enables the @-send gesture. */
  connected?: boolean;
  /** Receives a jump-to-file function once the widget is up (null on
   * teardown) — the diff pane's tree rail calls it to scroll a file's diff
   * into view. */
  registerReveal?: (reveal: ((path: string) => void) | null) => void;
  /** Paths the reviewer has checked off — purely client-side (not persisted),
   * shared with the tree rail's checkboxes. */
  reviewed: ReadonlySet<string>;
  /** Toggle one file's reviewed flag. */
  onToggleReviewed?: (path: string) => void;
  /** A file's unsaved-edit state flipped — mirrors what's also reported to
   * the IDE bridge (`ideSetDiffDirty`), so the tree rail can show the same
   * dirty dot the Files tab does. */
  onDirtyChange?: (path: string, dirty: boolean) => void;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  /** Which file's lines are highlighted — the multi-diff stacks many files,
   * so the chip has to name one. */
  const [selection, setSelection] = useState<{ path: string; range: MentionRange } | null>(null);
  const mentionRef = useRef<() => void>(() => {});
  const widgetRef = useRef<Widget | null>(null);
  const viewModelRef = useRef<ViewModel | null>(null);
  // Populated once the widget's diffs resolve; keyed by the same relative
  // path used everywhere else (`ChangedFile.path`). Lets the reviewed-sync
  // effect below collapse/expand and check/uncheck a specific file without
  // rebuilding the whole widget.
  const itemsByPathRef = useRef<Map<string, DocItem> | null>(null);
  const checkboxesByPathRef = useRef<Map<string, HTMLInputElement>>(new Map());
  const modelsRef = useRef<Map<string, { original?: TextModel; modified?: TextModel }>>(new Map());
  // mtime token (from `ideReadFile`/`saveModelBuffer`) and the modified
  // model's `getAlternativeVersionId()` as of the last successful save, per
  // file — together these say whether a file has unsaved edits and what a
  // `saveModelBuffer` call should send as `expectedMtimeMs`.
  const mtimesRef = useRef<Map<string, number | null>>(new Map());
  const savedVersionsRef = useRef<Map<string, number>>(new Map());
  // Paths currently reported to the IDE bridge (`ideSetDiffDirty`) as dirty —
  // lets `reportDirty` below send a call only on an actual clean↔dirty
  // transition instead of on every keystroke, and lets teardown clear
  // exactly what it told the backend, never more or less.
  const dirtyReportedRef = useRef<Set<string>>(new Set());
  // "Latest ref" for the imperative header checkboxes' change handler, which
  // closes over this once at widget-construction time and would otherwise see
  // a stale callback across parent re-renders that don't rebuild the widget.
  const onToggleReviewedRef = useRef(onToggleReviewed);
  onToggleReviewedRef.current = onToggleReviewed;
  const onDirtyChangeRef = useRef(onDirtyChange);
  onDirtyChangeRef.current = onDirtyChange;
  // "Latest ref" for the header checkboxes' `setUri`, which can fire on
  // virtualized-row reuse well after the widget-construction effect's own
  // `reviewed` closure has gone stale.
  const reviewedRef = useRef(reviewed);
  reviewedRef.current = reviewed;

  // Collapse/expand each file's diff to match its reviewed flag and keep its
  // header checkbox's `.checked` in sync — shared by the post-construction
  // initial sync and the reviewed-toggle effect below, since both need
  // exactly this same per-file walk over the (viewModel item, checkbox) pair.
  const applyReviewedState = (
    currentFiles: ChangedFile[],
    currentReviewed: ReadonlySet<string>,
  ) => {
    const viewModel = viewModelRef.current;
    const itemsByPath = itemsByPathRef.current;
    if (!viewModel || !itemsByPath) return;
    for (const f of currentFiles) {
      const isReviewed = currentReviewed.has(f.path);
      const item = itemsByPath.get(f.path);
      if (item) {
        if (isReviewed) viewModel.collapse(item);
        else viewModel.expand(item);
      }
      const checkbox = checkboxesByPathRef.current.get(f.path);
      if (checkbox) checkbox.checked = isReviewed;
    }
  };

  // Tell the IDE bridge (`ideSetDiffDirty`) when a file's dirty state
  // actually flips, so Claude's getOpenEditors/checkDocumentDirty see the
  // diff pane's edits the same way they already see the Files tab's. Called
  // from the modified model's onDidChangeContent and right after a save
  // (which can race further typing during the write).
  const reportDirty = (path: string, model: TextModel) => {
    const isDirty = model.getAlternativeVersionId() !== savedVersionsRef.current.get(path);
    const wasDirty = dirtyReportedRef.current.has(path);
    if (isDirty === wasDirty) return;
    if (isDirty) dirtyReportedRef.current.add(path);
    else dirtyReportedRef.current.delete(path);
    void ideSetDiffDirty(dir, path, isDirty);
    onDirtyChangeRef.current?.(path, isDirty);
  };

  // The widget rebuilds only when the file *set* changes, not on every
  // content refresh — filesKey is stable across refetches of the same set.
  const filesKey = files.map((f) => `${f.status}:${f.path}`).join("\n");

  useEffect(() => {
    let disposed = false;
    const disposables: Array<{ dispose(): void }> = [];
    let streamedPath: string | null = null;

    setError(null);
    setLoading(true);
    void (async () => {
      try {
        const [monaco, api, widgetMod, eventMod, utilsMod, domMod] = await Promise.all([
          loadMonaco(),
          import("@codingame/monaco-vscode-api"),
          import("@codingame/monaco-vscode-api/vscode/vs/editor/browser/widget/multiDiffEditor/multiDiffEditorWidget"),
          import("@codingame/monaco-vscode-api/vscode/vs/base/common/event"),
          import("@codingame/monaco-vscode-api/vscode/vs/editor/browser/widget/diffEditor/utils"),
          import("@codingame/monaco-vscode-api/vscode/vs/base/browser/dom"),
        ]);
        const contents = await Promise.all(files.map((f) => fetchSides(dir, f, mode, baseBranch)));
        if (disposed || !containerRef.current) return;

        const models = new Map<string, { original?: TextModel; modified?: TextModel }>();
        mtimesRef.current = new Map();
        savedVersionsRef.current = new Map();
        const items = files.map((f, i) => {
          const baseUri = monaco.Uri.parse(`tt-diff-base:${dir}/${f.oldPath ?? f.path}`);
          const workUri = monaco.Uri.parse(`tt-diff-work:${dir}/${f.path}`);
          monaco.editor.getModel(baseUri)?.dispose();
          monaco.editor.getModel(workUri)?.dispose();
          const entry: { original?: TextModel; modified?: TextModel } = {};
          if (contents[i].original != null) {
            entry.original = monaco.editor.createModel(contents[i].original!, undefined, baseUri);
          }
          if (contents[i].modified != null) {
            entry.modified = monaco.editor.createModel(contents[i].modified!, undefined, workUri);
            mtimesRef.current.set(f.path, contents[i].modifiedMtimeMs);
            savedVersionsRef.current.set(f.path, entry.modified.getAlternativeVersionId());
            disposables.push(
              entry.modified.onDidChangeContent(() => reportDirty(f.path, entry.modified!)),
            );
          }
          models.set(f.path, entry);
          return {
            original: entry.original,
            modified: entry.modified,
            // Base side stays read-only (it's history); working-tree side is
            // editable. Some workbench feature occasionally tries to resolve
            // the synthetic tt-diff-work: URI through the full text-model
            // resolver once a file's diff is active — that lookup has no
            // registered provider and rejects, logged as a harmless one-time
            // console error. Registering a content provider to quiet it was
            // tried and reverted: it hands the resolver a reference-counted
            // handle to a model this file already owns outright, and the
            // resolver's own disposal of that handle raced ours and blanked
            // the pane ("TextModel got disposed before DiffEditorWidget model
            // got reset"). The rejection doesn't affect rendering or editing.
            options: { readOnly: false, originalEditable: false },
          };
        });
        modelsRef.current = models;

        const widget = api.createInstanceSync(
          widgetMod.MultiDiffEditorWidget,
          containerRef.current,
          {
            headerClickToCollapse: true,
            createResourceLabel: (element: HTMLElement) => {
              // Called twice per file: once for the primary (current-path)
              // label, once for the secondary (old-path, renames only) one.
              // VS Code marks the primary one with its own "modified" class
              // — only that one gets a reviewed checkbox, or a renamed file
              // would show two.
              if (!element.classList.contains("modified")) {
                return {
                  setUri(uri: { path: string } | undefined) {
                    element.textContent = uri ? uri.path.replace(`${dir}/`, "") : "";
                  },
                  dispose() {},
                };
              }
              const checkbox = document.createElement("input");
              checkbox.type = "checkbox";
              checkbox.className = "mr-1.5 size-3 shrink-0 cursor-pointer accent-emerald-500";
              checkbox.title = "mark reviewed (collapses this file's diff)";
              // The header itself toggles collapse on click; stop that from
              // also firing when the click lands on the checkbox.
              checkbox.addEventListener("click", (e) => e.stopPropagation());
              const text = document.createElement("span");
              element.replaceChildren(checkbox, text);
              let path: string | null = null;
              checkbox.addEventListener("change", () => {
                if (!path) return;
                onToggleReviewedRef.current?.(path);
              });
              return {
                setUri(uri: { path: string } | undefined) {
                  if (path) checkboxesByPathRef.current.delete(path);
                  path = uri ? uri.path.replace(`${dir}/`, "") : null;
                  text.textContent = path ?? "";
                  checkbox.style.visibility = path ? "visible" : "hidden";
                  if (path) {
                    checkboxesByPathRef.current.set(path, checkbox);
                    checkbox.checked = reviewedRef.current.has(path);
                  }
                },
                dispose() {
                  if (path && checkboxesByPathRef.current.get(path) === checkbox) {
                    checkboxesByPathRef.current.delete(path);
                  }
                },
              };
            },
          },
        );
        widgetRef.current = widget;
        disposables.push(widget);

        const store = { dispose() {} };
        const viewModel = widget.createViewModel({
          documents: new eventMod.ValueWithChangeEvent(
            items.map((item) => utilsMod.RefCounted.createOfNonDisposable(item, store)),
          ),
        });
        disposables.push(viewModel);
        widget.setViewModel(viewModel);
        viewModelRef.current = viewModel;

        // Per-file diff computation is async; apply the initial reviewed
        // state (collapse + checkbox sync) once resolved, without blocking
        // the rest of setup (registerReveal, selection wiring, loading).
        void viewModel.waitForDiffOr1s().then(() => {
          if (disposed) return;
          const itemsByPath = new Map<string, DocItem>();
          for (const item of viewModel.items.get()) {
            const p = (item.modifiedUri ?? item.originalUri)?.path.replace(`${dir}/`, "");
            if (p) itemsByPath.set(p, item);
          }
          itemsByPathRef.current = itemsByPath;
          applyReviewedState(files, reviewedRef.current);
        });

        // The widget needs explicit layout; track the pane's size.
        const layout = () => {
          const el = containerRef.current;
          if (el && el.clientWidth > 0 && el.clientHeight > 0) {
            widget.layout(new domMod.Dimension(el.clientWidth, el.clientHeight));
          }
        };
        layout();
        const observer = new ResizeObserver(layout);
        observer.observe(containerRef.current);
        disposables.push({ dispose: () => observer.disconnect() });

        // Selection → Claude bridge: whichever file's diff is active streams
        // its modified-side selection, keyed by the tt-diff-work uri's path.
        const wired = new WeakSet<object>();
        const wire = () => {
          const control = widget.getActiveControl();
          if (!control || wired.has(control)) return;
          wired.add(control);
          const modified = control.getModifiedEditor();
          let debounce: ReturnType<typeof setTimeout> | undefined;
          disposables.push({ dispose: () => clearTimeout(debounce) });

          // Explicit @-mention of whatever is selected in this editor. Reads
          // the selection live, so the keystroke can't fire against a stale
          // range. Stable for this editor's lifetime — the ref below just
          // tracks which editor the chip is currently speaking for.
          const mention = async () => {
            const path = diffWorkPath(dir, modified.getModel()?.uri);
            if (!path) return;
            await ideMention(dir, path, mentionRangeFrom(modified.getSelection()));
          };
          const sendFromThisEditor = () => void mention();

          // Cmd/Ctrl+S saves this file's modified buffer — atomically,
          // refused if the file changed on disk since it was read (an agent
          // may be editing the same tree). Same contract as CodeViewer.
          const save = async () => {
            const model = modified.getModel();
            const path = diffWorkPath(dir, model?.uri);
            if (!model || !path) return;
            const result = await saveModelBuffer(
              dir,
              path,
              model,
              mtimesRef.current.get(path) ?? null,
            );
            if (!result || model.isDisposed()) return;
            mtimesRef.current.set(path, result.mtimeMs);
            savedVersionsRef.current.set(path, result.versionAtSave);
            // Reconciles against `model`'s *current* version, not
            // `result.versionAtSave` — more may have been typed during the
            // write, in which case the buffer is still dirty post-save.
            reportDirty(path, model);
          };

          // Same ⌘⇧A / ⌘S chords as the file viewer. These are plain
          // ICodeEditors inside the multi-diff, not standalone ones, so
          // there's no addCommand — match the chord on the key event instead.
          disposables.push(
            modified.onKeyDown((e: import("monaco-editor").IKeyboardEvent) => {
              if (!(e.ctrlKey || e.metaKey)) return;
              const action =
                e.keyCode === monaco.KeyCode.KeyA && e.shiftKey
                  ? mention
                  : e.keyCode === monaco.KeyCode.KeyS && !e.shiftKey
                    ? save
                    : null;
              if (!action) return;
              e.preventDefault();
              e.stopPropagation();
              void action();
            }),
          );

          disposables.push(
            modified.onDidChangeCursorSelection(
              (e: import("monaco-editor").editor.ICursorSelectionChangedEvent) => {
                // Resolve the file outside the debounce so the chip tracks the
                // selection immediately; only bridge traffic is debounced.
                const path = diffWorkPath(dir, modified.getModel()?.uri);
                if (!path) {
                  // Nothing here is mentionable, so drop the chip rather than
                  // leaving it naming — and `@ send`-ing — whichever file was
                  // selected before.
                  setSelection(null);
                  return;
                }
                const next = mentionRangeFrom(e.selection);
                mentionRef.current = sendFromThisEditor;
                setSelection((prev) => {
                  if (!next) return null;
                  if (prev?.path === path && sameMentionRange(prev.range, next)) return prev;
                  return { path, range: next };
                });
                clearTimeout(debounce);
                debounce = setTimeout(() => {
                  const sel = e.selection;
                  if (sel.isEmpty()) {
                    ideClearSelection(dir, path);
                    if (streamedPath === path) streamedPath = null;
                    return;
                  }
                  streamedPath = path;
                  const range = streamRangeFrom(sel);
                  ideSetSelection(
                    dir,
                    path,
                    range.startLine,
                    range.endLine,
                    range.startChar,
                    range.endChar,
                  );
                }, 300);
              },
            ),
          );
        };
        disposables.push(widget.onDidChangeActiveControl(wire));
        wire();

        registerReveal?.((path) => {
          const entry = modelsRef.current.get(path);
          if (!entry) return;
          try {
            widget.reveal(
              { original: entry.original?.uri, modified: entry.modified?.uri },
              { highlight: true },
            );
          } catch {
            // Not in the view (set changed under us) — the rebuild catches up.
          }
        });
        disposables.push({ dispose: () => registerReveal?.(null) });

        setLoading(false);
      } catch (e) {
        if (!disposed) {
          setError(errorMessage(e));
          setLoading(false);
        }
      }
    })();

    return () => {
      disposed = true;
      widgetRef.current = null;
      viewModelRef.current = null;
      itemsByPathRef.current = null;
      checkboxesByPathRef.current = new Map();
      for (const d of disposables.toReversed()) d.dispose();
      for (const entry of modelsRef.current.values()) {
        entry.original?.dispose();
        entry.modified?.dispose();
      }
      modelsRef.current = new Map();
      mtimesRef.current = new Map();
      savedVersionsRef.current = new Map();
      // Clear exactly what this instance told the IDE bridge — and the tree
      // rail — was dirty. Whether that's because the file set changed
      // (rebuild) or the pane closed (unmount), nothing here is still an
      // editable diff buffer, so nothing here should still read as dirty.
      for (const path of dirtyReportedRef.current) {
        void ideSetDiffDirty(dir, path, false);
        onDirtyChangeRef.current?.(path, false);
      }
      dirtyReportedRef.current = new Set();
      mentionRef.current = () => {};
      setSelection(null);
      if (streamedPath != null) ideClearSelection(dir, streamedPath);
    };
    // filesKey stands in for `files`; refreshKey/reviewed are the in-place
    // paths below. registerReveal is deliberately excluded too: it's an
    // unmemoized callback prop from the parent, so listing it would rebuild
    // this expensive widget on every parent render instead of only on a real
    // file-set/branch change.
    // oxlint-disable-next-line react-hooks/exhaustive-deps
  }, [dir, filesKey, mode, baseBranch]);

  // A file's reviewed flag flipped without the file *set* changing (review
  // doesn't touch its path or status letter) — collapse/expand its diff and
  // sync its header checkbox in place, without rebuilding the widget.
  useEffect(() => {
    applyReviewedState(files, reviewed);
    // files is read fresh each render via closure, same as the refreshKey
    // effect above — `reviewed`'s identity is only the trigger.
    // oxlint-disable-next-line react-hooks/exhaustive-deps
  }, [reviewed]);

  // Working tree changed under the pane — refresh contents in place so the
  // scroll position and collapse states survive. A file with unsaved edits
  // (its modified model's version has moved past the last save) is skipped:
  // an external change must never clobber a buffer the reviewer is mid-edit
  // on — the next save will surface the real conflict via `ideWriteFile`'s
  // mtime check instead.
  useEffect(() => {
    if (!widgetRef.current) return;
    const models = modelsRef.current;
    void (async () => {
      for (const f of files) {
        const entry = models.get(f.path);
        if (!entry) continue;
        try {
          const sides = await fetchSides(dir, f, mode, baseBranch);
          if (models !== modelsRef.current) return;
          if (
            entry.original &&
            sides.original != null &&
            entry.original.getValue() !== sides.original
          ) {
            entry.original.setValue(sides.original);
          }
          const dirty =
            entry.modified &&
            entry.modified.getAlternativeVersionId() !== savedVersionsRef.current.get(f.path);
          if (
            !dirty &&
            entry.modified &&
            sides.modified != null &&
            entry.modified.getValue() !== sides.modified
          ) {
            entry.modified.setValue(sides.modified);
            mtimesRef.current.set(f.path, sides.modifiedMtimeMs);
            savedVersionsRef.current.set(f.path, entry.modified.getAlternativeVersionId());
          }
        } catch {
          // Transient refresh failure (file mid-write) — keep the last good
          // contents; the next stats bump retries.
        }
      }
    })();
    // dir/mode/baseBranch/files are read from the closure at call time, not
    // reactive triggers — this effect intentionally fires only on refreshKey.
    // oxlint-disable-next-line react-hooks/exhaustive-deps
  }, [refreshKey]);

  if (error) {
    return <p className="p-3 text-sm text-muted-foreground">{error}</p>;
  }
  return (
    <div className="relative h-full w-full">
      {loading && <p className="absolute p-3 text-sm text-muted-foreground">Loading…</p>}
      <div ref={containerRef} className="h-full w-full" />
      <IdeSelectionOverlay
        selection={selection?.range ?? null}
        label={selection?.path}
        connected={connected}
        loading={loading}
        onSend={() => mentionRef.current()}
      />
    </div>
  );
}

/** Both sides of one file's diff: base content (undefined for
 * added/untracked) and working-tree content (undefined for deleted), plus the
 * working-tree read's mtime token — the save path's `expectedMtimeMs`. */
async function fetchSides(
  dir: string,
  file: ChangedFile,
  mode: string,
  baseBranch: string | null,
): Promise<{
  original: string | undefined;
  modified: string | undefined;
  modifiedMtimeMs: number | null;
}> {
  const added = file.status === "A" || file.status === "?";
  const [original, read] = await Promise.all([
    added
      ? null
      : invoke<string | null>("ab_get_base_file", {
          dir,
          mode,
          baseBranch,
          path: file.oldPath ?? file.path,
        }).then((r) => r.unwrapOr(null)),
    file.status === "D" ? null : ideReadFile(dir, file.path),
  ]);
  return {
    original: added ? undefined : (original ?? ""),
    modified: file.status === "D" ? undefined : (read?.map((f) => f.content).unwrapOr("") ?? ""),
    modifiedMtimeMs: read?.map((f) => f.mtimeMs).unwrapOr(null) ?? null,
  };
}
