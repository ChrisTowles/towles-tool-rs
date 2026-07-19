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
  ideStat,
  ideUnwatchFiles,
  ideWatchFiles,
  onFilesChangedOnDisk,
  saveBufferSnapshot,
  snapshotModel,
} from "@/lib/ide";
import { AUTOSAVE_DELAY_MS, diskChangeAction } from "@/lib/viewer-refresh";
import { IdeSelectionOverlay } from "@/components/ide-selection-chip";
import { ViewerBanner } from "@/components/viewer-banner";
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
 * sides come from the working tree and are editable in place, and
 * **auto-save**: a `AUTOSAVE_DELAY_MS` pause in typing writes the buffer
 * (Cmd/Ctrl+S is save-now), through the same atomic/mtime-guarded write
 * CodeViewer's manual save uses. A conflicted file never auto-saves — the
 * banner's explicit choice is the only way out — and a widget rebuild or
 * unmount flushes pending edits first, so autosave can't eat the last
 * second of typing.
 * Selections on any modified side stream to the folder's Claude session, and
 * the selection chip (or ⌘⇧A) mentions those lines explicitly — same
 * contract as CodeViewer.
 *
 * Three refresh paths keep the pane honest while an agent works in the same
 * tree. Every working-tree side is disk-watched (`ide_watch_files` →
 * `ide://file-changed`, shared with CodeViewer): a changed file stat-checks,
 * re-reads, and — per `lib/viewer-refresh.ts` — a clean buffer reloads in
 * place while a dirty one is flagged as a conflict, surfaced in the banner
 * overlay ("load theirs" discards those buffers, "keep mine" overwrites the
 * disk with them now) and mirrored to the tree rail via `onConflictChange`.
 * `refreshKey` (the folder's git stats) re-runs the same per-file check as a
 * safety net behind the watch, and `baseKey` (commits/compared-ref only)
 * refetches the read-only base sides — the one thing no working-tree watch
 * can see. The set of files changing rebuilds the widget.
 */
export function MonacoMultiDiff({
  dir,
  files,
  mode,
  baseBranch,
  refreshKey,
  baseKey,
  connected = false,
  registerReveal,
  reviewed,
  onToggleReviewed,
  onDirtyChange,
  onConflictChange,
}: {
  dir: string;
  files: ChangedFile[];
  mode: string;
  baseBranch: string | null;
  refreshKey: string;
  /** Changes only when the diff *baseline* can have moved (a commit landed,
   * the compared ref changed) — refetching read-only base sides on every
   * working-tree keystroke-stats bump would be pure waste. */
  baseKey: string;
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
  /** A file's changed-on-disk-under-unsaved-edits state flipped — the tree
   * rail marks it; resolution lives in this component's banner. */
  onConflictChange?: (path: string, conflict: boolean) => void;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  /** Files whose disk content changed while their buffer has unsaved edits —
   * drives the banner overlay; `conflictsRef` is the transition-detecting
   * mirror (state alone can't say "was it already marked?" mid-callback). */
  const [conflicts, setConflictsState] = useState<ReadonlySet<string>>(() => new Set());
  const conflictsRef = useRef<Set<string>>(new Set());
  /** A disk reload is being applied — the per-model content listeners must
   * not treat it as user typing (the saved-version token only catches up
   * *after* the setValue, so an unguarded listener reports a false dirty
   * and would schedule a pointless autosave). */
  const applyingDiskRef = useRef(false);
  /** Pending debounced auto-saves, one timer per file. */
  const autosaveTimersRef = useRef<Map<string, ReturnType<typeof setTimeout>>>(new Map());
  /** In-flight save chain per file — see `saveFile`. Entries are only ever
   * awaited/extended, so stale paths after a rebuild are inert. */
  const saveChainsRef = useRef<Map<string, Promise<void>>>(new Map());
  /** Conflicted buffers carried across a widget rebuild — a rebuild is
   * agent-triggered (the file *set* changed), so it must not resolve a
   * conflict by disposal; the next build restores these and re-raises the
   * banner. Keyed by path, value is the buffer content at teardown. */
  const conflictCarryRef = useRef<Map<string, string>>(new Map());
  /** The `baseKey` whose base-side contents are currently in the models —
   * lets the base-refresh effect skip its post-mount firing (construction
   * just fetched exactly these) and re-run only on a real baseline move.
   * Stamped with the key as of the fetch's *start*, so a baseline moving
   * mid-fetch re-fires the effect rather than being skipped. */
  const appliedBaseKeyRef = useRef<string | null>(null);
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
  const onConflictChangeRef = useRef(onConflictChange);
  onConflictChangeRef.current = onConflictChange;
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

  const cancelAutosave = (path: string) => {
    const timer = autosaveTimersRef.current.get(path);
    if (timer !== undefined) {
      clearTimeout(timer);
      autosaveTimersRef.current.delete(path);
    }
  };

  /** The one save path for a modified buffer — ⌘S, the debounced autosave,
   * the banner's "keep mine", and the rebuild flush all land here. Cancels
   * the file's pending autosave so the same buffer isn't written twice
   * back-to-back, and serializes per file: the buffer is snapshotted now,
   * but the write waits for any in-flight save of the same path —
   * overlapping writes would race each other's mtime tokens and get the
   * later one refused (losing its tail when it was the rebuild flush).
   * Atomic + mtime-guarded (a refused save toasts and leaves the buffer
   * dirty). */
  const saveFile = (path: string, model: TextModel): Promise<void> => {
    if (model.isDisposed()) return Promise.resolve();
    cancelAutosave(path);
    const snapshot = snapshotModel(model);
    const chain = (saveChainsRef.current.get(path) ?? Promise.resolve()).then(async () => {
      // The token is read here — after the previous save refreshed it.
      const result = await saveBufferSnapshot(
        dir,
        path,
        snapshot,
        mtimesRef.current.get(path) ?? null,
      );
      if (!result) return;
      mtimesRef.current.set(path, result.mtimeMs);
      savedVersionsRef.current.set(path, result.versionAtSave);
      // Reconciles against `model`'s *current* version, not
      // `result.versionAtSave` — more may have been typed during the write,
      // in which case the buffer is still dirty post-save.
      if (!model.isDisposed()) reportDirty(path, model);
    });
    saveChainsRef.current.set(path, chain);
    return chain;
  };

  /** (Re)arm `path`'s debounced autosave — every keystroke pushes the
   * deadline out. At fire time the world may have moved on, so re-check:
   * a clean buffer has nothing to save, and a conflicted one must not save
   * (the banner's explicit choice is the only way out of a conflict). */
  const scheduleAutosave = (path: string, model: TextModel) => {
    cancelAutosave(path);
    autosaveTimersRef.current.set(
      path,
      setTimeout(() => {
        autosaveTimersRef.current.delete(path);
        if (model.isDisposed() || conflictsRef.current.has(path)) return;
        if (model.getAlternativeVersionId() === savedVersionsRef.current.get(path)) return;
        void saveFile(path, model);
      }, AUTOSAVE_DELAY_MS),
    );
  };

  // Flip one file's conflict mark — state for the banner, `onConflictChange`
  // for the tree rail — only on an actual transition, same discipline as
  // `reportDirty`. Entering a conflict kills the file's pending autosave:
  // the save would only bounce off the mtime guard with an error toast.
  const setConflict = (path: string, inConflict: boolean) => {
    if (conflictsRef.current.has(path) === inConflict) return;
    if (inConflict) {
      conflictsRef.current.add(path);
      cancelAutosave(path);
    } else {
      conflictsRef.current.delete(path);
    }
    setConflictsState(new Set(conflictsRef.current));
    onConflictChangeRef.current?.(path, inConflict);
  };

  /** Take a fresh working-tree read into `path`'s modified model in place —
   * the "load theirs" of a reload or a resolved conflict. Applied via
   * `pushEditOperations`, not `setValue`, so undo history survives (an
   * agent's edit stays undoable) — same mechanics as CodeViewer. */
  const applyDisk = (path: string, model: TextModel, content: string, mtimeMs: number | null) => {
    if (model.getValue() !== content) {
      applyingDiskRef.current = true;
      model.pushEditOperations(
        [],
        [{ range: model.getFullModelRange(), text: content }],
        () => null,
      );
      applyingDiskRef.current = false;
    }
    mtimesRef.current.set(path, mtimeMs);
    savedVersionsRef.current.set(path, model.getAlternativeVersionId());
    setConflict(path, false);
    reportDirty(path, model);
  };

  // One watched file changed on disk (`ide://file-changed`) — same policy as
  // CodeViewer, stat-first: the most frequent event is the echo of our own
  // save, and a stat answers "did anything actually move?" without paying a
  // full content read. A real change re-reads; a clean buffer reloads in
  // place, a dirty one is flagged for the banner.
  const onDiskChange = async (path: string) => {
    const model = modelsRef.current.get(path)?.modified;
    if (!model || model.isDisposed()) return;
    const stat = await ideStat(dir, path);
    // A failed stat here is a deleted file: the diff pane resolves those at
    // the file-list level (the next stats bump drops or re-statuses the
    // row), so the buffer just stays put until then.
    if (stat.isErr() || model.isDisposed()) return;
    if (stat.value.mtimeMs === (mtimesRef.current.get(path) ?? null)) return;
    const read = await ideReadFile(dir, path);
    if (read.isErr() || model.isDisposed() || modelsRef.current.get(path)?.modified !== model) {
      return;
    }
    const action = diskChangeAction({
      dirty: model.getAlternativeVersionId() !== savedVersionsRef.current.get(path),
      bufferMtimeMs: mtimesRef.current.get(path) ?? null,
      diskMtimeMs: read.value.mtimeMs,
    });
    if (action === "reload") applyDisk(path, model, read.value.content, read.value.mtimeMs);
    else if (action === "conflict") setConflict(path, true);
  };

  // The banner's buttons, applied to every conflicted file at once —
  // conflicts are rare enough here that per-file resolution isn't worth a
  // second UI. Both are decisive: "theirs" discards those buffers for the
  // disk content; "mine" overwrites the disk with the buffer right now
  // (mtime token re-armed to the current disk state first, so the save
  // can't bounce off its own conflict guard; null when the file vanished —
  // the save then recreates it).
  const resolveConflicts = async (choice: "theirs" | "mine") => {
    // Snapshot first — `setConflict` below mutates the live set mid-loop.
    const conflicted = Array.from(conflictsRef.current);
    for (const path of conflicted) {
      const model = modelsRef.current.get(path)?.modified;
      if (!model || model.isDisposed()) {
        setConflict(path, false);
        continue;
      }
      if (choice === "theirs") {
        const read = await ideReadFile(dir, path);
        if (read.isErr()) {
          const { toast } = await import("sonner");
          toast.error(`Couldn't reload ${path} — ${read.error.message}`);
          continue;
        }
        applyDisk(path, model, read.value.content, read.value.mtimeMs);
      } else {
        // A stat is enough — only the mtime is used to re-arm the token
        // before the buffer overwrites the disk.
        const stat = await ideStat(dir, path);
        mtimesRef.current.set(path, stat.isOk() ? stat.value.mtimeMs : null);
        setConflict(path, false);
        await saveFile(path, model);
      }
    }
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
              entry.modified.onDidChangeContent(() => {
                // A programmatic disk reload is not typing: it must neither
                // flap the dirty report nor arm an autosave.
                if (applyingDiskRef.current) return;
                reportDirty(f.path, entry.modified!);
                scheduleAutosave(f.path, entry.modified!);
              }),
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
        appliedBaseKeyRef.current = baseKey;

        // Restore conflicted buffers the previous generation carried across
        // the rebuild: put the user's content back, mark it dirty, and
        // re-raise the banner — the rebuild was agent-triggered, so it must
        // not stand in for the user's load-theirs/keep-mine choice. A carry
        // whose disk caught up (contents now equal) just drops.
        for (const [path, carried] of conflictCarryRef.current) {
          const restored = models.get(path)?.modified;
          if (!restored || restored.isDisposed() || restored.getValue() === carried) continue;
          applyingDiskRef.current = true;
          restored.pushEditOperations(
            [],
            [{ range: restored.getFullModelRange(), text: carried }],
            () => null,
          );
          applyingDiskRef.current = false;
          reportDirty(path, restored);
          setConflict(path, true);
        }
        conflictCarryRef.current = new Map();

        // Disk-watch every working-tree side (refcounted in Rust, shared
        // with any CodeViewer on the same file) and route change events to
        // the per-file refresh above. One batched IPC call for the whole
        // set. Registered here — after the models exist — so an event can
        // never race a half-built map. The follow-up sweep is the same
        // catch-up CodeViewer does: a write landing between the content
        // reads and the watch going live would otherwise be missed forever
        // (the stats safety net is blind to edits that keep the line counts
        // unchanged); stat-first makes the nothing-changed case nearly free.
        const watchedPaths = files.filter((f) => models.get(f.path)?.modified).map((f) => f.path);
        void ideWatchFiles(dir, watchedPaths).then((started) => {
          if (started.isErr() || disposed) return;
          for (const path of watchedPaths) void onDiskChange(path);
        });
        const offDiskChanges = onFilesChangedOnDisk(dir, (path) => void onDiskChange(path));
        disposables.push({
          dispose: () => {
            offDiskChanges();
            void ideUnwatchFiles(dir, watchedPaths);
          },
        });

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

          // Cmd/Ctrl+S is save-now — same path the debounced autosave takes
          // (`saveFile` cancels the pending timer itself), just without
          // waiting out the pause.
          const save = async () => {
            const model = modified.getModel();
            const path = diffWorkPath(dir, model?.uri);
            if (!model || !path) return;
            await saveFile(path, model);
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
      // Before the models die: flush every dirty, unconflicted buffer —
      // with autosave the user no longer thinks about saving, so a rebuild
      // (file set changed, mode switched) or unmount must not eat their
      // edits. Keyed off the dirty-report set, not the pending timers: a
      // file whose save was refused is dirty with no timer and needs the
      // flush most. Conflicted buffers can't be saved (the mtime guard
      // would bounce them) — they're stashed instead, and the next build
      // restores them with the banner re-raised, because a rebuild is
      // agent-triggered and must not stand in for the user's choice.
      for (const timer of autosaveTimersRef.current.values()) clearTimeout(timer);
      autosaveTimersRef.current = new Map();
      for (const path of dirtyReportedRef.current) {
        const model = modelsRef.current.get(path)?.modified;
        if (!model || model.isDisposed()) continue;
        if (conflictsRef.current.has(path)) conflictCarryRef.current.set(path, model.getValue());
        else void saveFile(path, model);
      }
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
      // Same for conflict marks — the buffers they described are gone.
      for (const path of conflictsRef.current) onConflictChangeRef.current?.(path, false);
      conflictsRef.current = new Set();
      setConflictsState(new Set());
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

  // The diff *baseline* moved (a commit landed, the compared ref changed) —
  // refetch the read-only base sides, concurrently, in place. Skips its
  // post-construction firing: the build just fetched exactly this baseKey.
  useEffect(() => {
    if (!widgetRef.current || appliedBaseKeyRef.current === baseKey) return;
    appliedBaseKeyRef.current = baseKey;
    const models = modelsRef.current;
    void Promise.all(
      files.map(async (f) => {
        const original = models.get(f.path)?.original;
        if (!original) return;
        const content = await fetchBase(dir, f, mode, baseBranch);
        if (models !== modelsRef.current || original.isDisposed()) return;
        if (content != null && original.getValue() !== content) original.setValue(content);
      }),
    );
    // dir/mode/baseBranch/files are read from the closure at call time, not
    // reactive triggers — this effect intentionally fires only on baseKey.
    // oxlint-disable-next-line react-hooks/exhaustive-deps
  }, [baseKey]);

  // Working tree measurably changed (the folder's git stats bumped) — the
  // safety net behind the per-file disk watch, catching anything the watch
  // missed (a watch that failed to start, inotify limits). Delegates to the
  // same stat-first `onDiskChange` the watch events use: near-free per file
  // when nothing actually moved, and one policy for reload-vs-conflict.
  useEffect(() => {
    if (!widgetRef.current) return;
    for (const f of files) void onDiskChange(f.path);
    // files is read from the closure at call time, not a reactive trigger —
    // this effect intentionally fires only on refreshKey.
    // oxlint-disable-next-line react-hooks/exhaustive-deps
  }, [refreshKey]);

  if (error) {
    return <p className="p-3 text-sm text-muted-foreground">{error}</p>;
  }
  return (
    <div className="relative h-full w-full">
      {loading && <p className="absolute p-3 text-sm text-muted-foreground">Loading…</p>}
      {conflicts.size > 0 && (
        <ViewerBanner
          message={
            conflicts.size === 1
              ? `${[...conflicts][0]} changed on disk while you have unsaved edits`
              : `${conflicts.size} files changed on disk while you have unsaved edits`
          }
          theirsTitle="Discard those buffers and load the files as they are on disk"
          mineTitle="Keep those buffers — overwrite the disk with them now"
          onTheirs={() => void resolveConflicts("theirs")}
          onMine={() => void resolveConflicts("mine")}
        />
      )}
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

/** One file's read-only base side, or null when it has none (added/untracked)
 * or the fetch failed. */
async function fetchBase(
  dir: string,
  file: ChangedFile,
  mode: string,
  baseBranch: string | null,
): Promise<string | null> {
  if (file.status === "A" || file.status === "?") return null;
  const content = await invoke<string | null>("ab_get_base_file", {
    dir,
    mode,
    baseBranch,
    path: file.oldPath ?? file.path,
  });
  return content.unwrapOr(null);
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
    fetchBase(dir, file, mode, baseBranch),
    file.status === "D" ? null : ideReadFile(dir, file.path),
  ]);
  return {
    original: added ? undefined : (original ?? ""),
    modified: file.status === "D" ? undefined : (read?.map((f) => f.content).unwrapOr("") ?? ""),
    modifiedMtimeMs: read?.map((f) => f.mtimeMs).unwrapOr(null) ?? null,
  };
}
