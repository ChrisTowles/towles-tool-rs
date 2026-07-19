import { useCallback, useEffect, useRef, useState } from "react";
import { loadMonaco } from "@/lib/monaco";
import {
  ideClearSelection,
  ideMention,
  ideReadFile,
  ideSetOpenFile,
  ideSetSelection,
  ideStat,
  ideUnwatchFiles,
  ideWatchFiles,
  onFileChangedOnDisk,
  saveBufferSnapshot,
  snapshotModel,
  type FileRead,
} from "@/lib/ide";
import { AUTOSAVE_DELAY_MS, diskChangeAction } from "@/lib/viewer-refresh";
import { NotInTauri, errorMessage } from "@/lib/errors";
import { IdeSelectionOverlay } from "@/components/ide-selection-chip";
import { ViewerBanner } from "@/components/viewer-banner";
import {
  mentionRangeFrom,
  sameMentionRange,
  streamRangeFrom,
  type MentionRange,
} from "@/lib/ide-selection";

/**
 * Monaco editor for one repo file (the Files tab's right pane). Two bridges:
 * selections stream to the folder's Claude session as character-precise
 * selection_changed (debounced 300ms, like VS Code), and edits **auto-save**
 * after an `AUTOSAVE_DELAY_MS` typing pause (⌘S is save-now; both take the
 * same atomic write, refused if the file changed on disk since it was read —
 * an agent may be editing the same tree). Unmount/file-switch flushes a
 * pending save so autosave can't eat the last second of typing. Dirty state
 * rides to Claude via getOpenEditors / checkDocumentDirty.
 *
 * The open file is also watched on disk (`ide_watch_file` →
 * `ide://file-changed`), because the usual other writer is a Claude session
 * working in this same checkout: a clean buffer silently reloads in place
 * (view state and undo history survive), while a dirty one raises a
 * conflict banner — "load theirs" discards the buffer, "keep mine"
 * overwrites the disk with it now — so neither side's edits are ever
 * dropped without a choice. A conflicted or deleted-on-disk file never
 * auto-saves: conflict resolution is the banner's explicit choice, and
 * recreating a deleted file is ⌘S's deliberate act. `lib/viewer-refresh.ts`
 * is the decision; the watcher's echo of our own save is ignored there by
 * mtime.
 */

/** Text anchors from Claude's openFile tool: select startText..endText. */
export type ViewerAnchor = {
  startText?: string | null;
  endText?: string | null;
  selectToEndOfLine?: boolean | null;
};

export function CodeViewer({
  dir,
  path,
  anchor,
  wordWrap = true,
  connected = false,
  onDirtyChange,
}: {
  dir: string;
  path: string;
  /** Changes identity per openFile request so re-anchoring re-runs. */
  anchor?: ViewerAnchor & { nonce?: number };
  /** Soft-wrap long lines instead of horizontal scrolling. Defaults on. */
  wordWrap?: boolean;
  /** A Claude session is live in this folder — enables the @-send gesture. */
  connected?: boolean;
  onDirtyChange?: (dirty: boolean) => void;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [selection, setSelection] = useState<MentionRange | null>(null);
  /** The disk moved underneath the buffer: "conflict" = changed while the
   * buffer has unsaved edits (the banner's choice resolves it), "deleted" =
   * the file is gone and the buffer is all that's left (⌘S recreates it —
   * the save token is nulled so the stale-mtime refusal can't block it).
   * Mutually exclusive by construction, hence one state. */
  const [banner, setBannerState] = useState<"none" | "conflict" | "deleted">("none");
  const editorRef = useRef<import("monaco-editor").editor.IStandaloneCodeEditor | null>(null);
  /** Set per mount — the banner's buttons resolve against the live model. */
  const resolveConflictRef = useRef<((choice: "theirs" | "mine") => Promise<void>) | null>(null);
  const savedVersionRef = useRef(0);
  const mtimeRef = useRef<number | null>(null);
  const dirtyRef = useRef(false);
  const onDirtyRef = useRef(onDirtyChange);
  onDirtyRef.current = onDirtyChange;
  const wordWrapRef = useRef(wordWrap);
  wordWrapRef.current = wordWrap;

  /** Explicit @-mention of whatever is selected right now — read live from the
   * editor, never from the debounced chip state, so the gesture can't fire
   * against a stale range. */
  const mention = useCallback(async () => {
    const editor = editorRef.current;
    if (!editor) return;
    await ideMention(dir, path, mentionRangeFrom(editor.getSelection()));
    // Stable across renders so listing it in the editor effect below doesn't
    // tear down and rebuild Monaco on every render.
  }, [dir, path]);

  useEffect(() => {
    let disposed = false;
    let editor: import("monaco-editor").editor.IStandaloneCodeEditor | undefined;
    let model: import("monaco-editor").editor.ITextModel | undefined;
    let debounce: ReturnType<typeof setTimeout> | undefined;
    /** Whether a selection is currently parked in the session's context. */
    let streamed = false;
    /** A disk reload is being applied — the content listener must not treat
     * it as user typing. */
    let applyingDisk = false;
    let offDiskChange: (() => void) | undefined;
    let autosaveTimer: ReturnType<typeof setTimeout> | undefined;
    /** Closure mirror of the banner state — the autosave deadline fires
     * outside React and needs it synchronously. */
    let bannerNow: "none" | "conflict" | "deleted" = "none";
    /** Set once the editor (and its `save`) exists — cleanup flushes a
     * dirty buffer through it before disposal. */
    let flushSave: (() => void) | undefined;

    setError(null);
    setLoading(true);
    setSelection(null);
    setBannerState("none");
    dirtyRef.current = false;
    void (async () => {
      let read: FileRead;
      try {
        const [, r] = await Promise.all([loadMonaco(), ideReadFile(dir, path)]);
        if (r.isErr()) {
          if (!disposed) {
            setError(NotInTauri.is(r.error) ? "not available in browser dev" : r.error.message);
            setLoading(false);
          }
          return;
        }
        read = r.value;
      } catch (e) {
        if (!disposed) {
          setError(errorMessage(e));
          setLoading(false);
        }
        return;
      }
      if (disposed || !containerRef.current) return;
      const monaco = await loadMonaco();
      const uri = monaco.Uri.file(`${dir}/${path}`);
      monaco.editor.getModel(uri)?.dispose();
      model = monaco.editor.createModel(read.content, undefined, uri);
      mtimeRef.current = read.mtimeMs;
      editor = monaco.editor.create(containerRef.current, {
        model,
        automaticLayout: true,
        minimap: { enabled: false },
        fontSize: 12,
        lineNumbersMinChars: 4,
        scrollBeyondLastLine: false,
        renderLineHighlight: "line",
        occurrencesHighlight: "off",
        contextmenu: false,
        wordWrap: wordWrapRef.current ? "on" : "off",
      });
      editorRef.current = editor;
      savedVersionRef.current = model.getAlternativeVersionId();
      setLoading(false);
      ideSetOpenFile(dir, path, false);

      const setDirty = (dirty: boolean) => {
        if (dirtyRef.current === dirty) return;
        dirtyRef.current = dirty;
        ideSetOpenFile(dir, path, dirty);
        onDirtyRef.current?.(dirty);
      };

      // Banner transitions in one place: mirror into the closure for the
      // autosave deadline, and kill a pending autosave on entry — it would
      // only bounce off the save's mtime guard with an error toast.
      const setBanner = (value: "none" | "conflict" | "deleted") => {
        bannerNow = value;
        if (value !== "none") clearTimeout(autosaveTimer);
        setBannerState(value);
      };

      // The one save path — ⌘S, the debounced autosave, the banner's "keep
      // mine", and the cleanup flush all land here. Cancels any pending
      // autosave so the same buffer isn't written twice back-to-back.
      // Serialized: the buffer is snapshotted now, but the write waits for
      // any in-flight save — overlapping writes would race each other's
      // mtime tokens and get the later one refused (losing its tail when
      // it was the unmount flush).
      let saveChain: Promise<void> = Promise.resolve();
      const save = () => {
        if (!model || model.isDisposed()) return saveChain;
        clearTimeout(autosaveTimer);
        const snapshot = snapshotModel(model);
        saveChain = saveChain.then(async () => {
          // The token is read here — after the previous save refreshed it.
          const result = await saveBufferSnapshot(dir, path, snapshot, mtimeRef.current);
          if (!result) return;
          mtimeRef.current = result.mtimeMs;
          savedVersionRef.current = result.versionAtSave;
          if (model && !model.isDisposed()) {
            setDirty(model.getAlternativeVersionId() !== result.versionAtSave);
          }
          // A successful save means disk and buffer agree again — a deleted
          // file is recreated by it, and a conflict can only reach here
          // after its resolution re-armed the token.
          setBanner("none");
        });
        return saveChain;
      };
      flushSave = () => {
        if (dirtyRef.current && bannerNow === "none") void save();
      };

      /** (Re)arm the debounced autosave — every keystroke pushes the
       * deadline out. At fire time the world may have moved on, so re-check:
       * a clean buffer has nothing to save, and a raised banner means the
       * user owns the next move (conflict resolution is the banner's
       * explicit choice; recreating a deleted file stays ⌘S's deliberate
       * act). */
      const scheduleAutosave = () => {
        clearTimeout(autosaveTimer);
        autosaveTimer = setTimeout(() => {
          if (disposed || !model || model.isDisposed()) return;
          if (bannerNow !== "none") return;
          if (model.getAlternativeVersionId() === savedVersionRef.current) return;
          void save();
        }, AUTOSAVE_DELAY_MS);
      };

      model.onDidChangeContent(() => {
        if (applyingDisk) return;
        setDirty(model!.getAlternativeVersionId() !== savedVersionRef.current);
        scheduleAutosave();
      });

      /** Take the disk's content into the buffer in place — cursor/scroll and
       * undo history survive (an agent's edit stays undoable). The equality
       * guard makes an mtime-only change (touch, identical rewrite) free. */
      const applyDisk = (disk: FileRead) => {
        if (!editor || !model || model.isDisposed()) return;
        if (model.getValue() !== disk.content) {
          const viewState = editor.saveViewState();
          applyingDisk = true;
          model.pushEditOperations(
            [],
            [{ range: model.getFullModelRange(), text: disk.content }],
            () => null,
          );
          applyingDisk = false;
          if (viewState) editor.restoreViewState(viewState);
        }
        mtimeRef.current = disk.mtimeMs;
        savedVersionRef.current = model.getAlternativeVersionId();
        setDirty(false);
        setBanner("none");
      };

      // Stat-first: the most frequent event this watcher delivers is the
      // echo of our own save, and a stat answers "did anything actually
      // move?" without paying a full content read + IPC transfer for it.
      const onDiskChange = async () => {
        const stat = await ideStat(dir, path);
        if (disposed || !model || model.isDisposed()) return;
        if (stat.isErr()) {
          if (NotInTauri.is(stat.error)) return;
          // Gone — an agent deleted it. Surface that, keep the buffer as
          // the sole copy, and null the save token so ⌘S recreates it. The
          // dirty flag is deliberately left alone: a buffer that was clean
          // at deletion silently adopts the recreated content when the file
          // comes back (dirty=false → "reload"), instead of raising a
          // conflict over edits that don't exist.
          mtimeRef.current = null;
          setBanner("deleted");
          return;
        }
        if (stat.value.mtimeMs === mtimeRef.current) return;
        const reread = await ideReadFile(dir, path);
        // A failed read after a successful stat (mid-rename, turned binary)
        // keeps the buffer as-is; a later event re-checks.
        if (disposed || reread.isErr() || !model || model.isDisposed()) return;
        const action = diskChangeAction({
          dirty: dirtyRef.current,
          bufferMtimeMs: mtimeRef.current,
          diskMtimeMs: reread.value.mtimeMs,
        });
        if (action === "reload") applyDisk(reread.value);
        else if (action === "conflict") setBanner("conflict");
      };

      resolveConflictRef.current = async (choice) => {
        if (choice === "theirs") {
          const reread = await ideReadFile(dir, path);
          if (disposed || !model || model.isDisposed()) return;
          if (reread.isErr()) {
            const { toast } = await import("sonner");
            toast.error(`Couldn't reload ${path} — ${reread.error.message}`);
            return;
          }
          applyDisk(reread.value);
        } else {
          // Keep mine: re-arm the save token to the current disk state (a
          // stat is enough — only the mtime is used; null when the file
          // vanished, the save then recreates it) and overwrite the disk
          // with the buffer right now — decisive, same as the diff pane,
          // instead of leaving a zombie-dirty buffer around.
          const stat = await ideStat(dir, path);
          if (disposed || !model || model.isDisposed()) return;
          mtimeRef.current = stat.isOk() ? stat.value.mtimeMs : null;
          setBanner("none");
          await save();
        }
      };

      void ideWatchFiles(dir, [path]).then((started) => {
        // One catch-up check once the watch is live: an edit landing between
        // the initial read and the watch start would otherwise be missed
        // forever. The stat-first check makes the common nothing-changed
        // case nearly free.
        if (started.isOk() && !disposed) void onDiskChange();
      });
      offDiskChange = onFileChangedOnDisk(dir, path, () => void onDiskChange());

      editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, () => void save());

      editor.addCommand(
        monaco.KeyMod.CtrlCmd | monaco.KeyMod.Shift | monaco.KeyCode.KeyA,
        () => void mention(),
      );

      editor.onDidChangeCursorSelection((e) => {
        // The chip tracks the selection immediately — only the bridge traffic
        // is debounced.
        const next = mentionRangeFrom(e.selection);
        setSelection((prev) => (sameMentionRange(prev, next) ? prev : next));
        clearTimeout(debounce);
        debounce = setTimeout(() => {
          const sel = e.selection;
          if (sel.isEmpty()) {
            ideClearSelection(dir, path);
            streamed = false;
            return;
          }
          const range = streamRangeFrom(sel);
          streamed = true;
          ideSetSelection(
            dir,
            path,
            range.startLine,
            range.endLine,
            range.startChar,
            range.endChar,
          );
        }, 300);
      });
    })();

    return () => {
      disposed = true;
      clearTimeout(debounce);
      clearTimeout(autosaveTimer);
      // Flush a dirty buffer before the model dies — with autosave the user
      // no longer thinks about saving, so switching files must not eat the
      // last second of typing. (The buffer is read synchronously before the
      // dispose below; conflicted/deleted buffers are skipped — unresolved
      // means neither side has won.)
      flushSave?.();
      offDiskChange?.();
      // Unmatched unwatches are a no-op, so this needs no "did the watch
      // actually start" bookkeeping.
      void ideUnwatchFiles(dir, [path]);
      resolveConflictRef.current = null;
      editorRef.current = null;
      editor?.dispose();
      model?.dispose();
      // Closing a file with text selected must not leave that range as the
      // folder's ambient context — getLatestSelection would keep serving it
      // into the next prompt.
      if (streamed) ideClearSelection(dir, path);
      ideSetOpenFile(dir, null);
    };
  }, [dir, path, mention]);

  useEffect(() => {
    editorRef.current?.updateOptions({ wordWrap: wordWrap ? "on" : "off" });
  }, [wordWrap]);

  // Claude's openFile can ask for a startText..endText selection — find the
  // anchors in the buffer, select, and scroll them into view.
  useEffect(() => {
    if (!anchor?.startText) return;
    void (async () => {
      const monaco = await loadMonaco();
      const editor = editorRef.current;
      const model = editor?.getModel();
      if (!editor || !model) return;
      const start = model.findMatches(anchor.startText!, false, false, false, null, false, 1)[0];
      if (!start) return;
      let range = start.range;
      if (anchor.endText) {
        const after = model.findMatches(anchor.endText, false, false, false, null, false, 50);
        const end = after.find(
          (m) =>
            m.range.startLineNumber > range.startLineNumber ||
            (m.range.startLineNumber === range.startLineNumber &&
              m.range.startColumn >= range.endColumn),
        );
        if (end) {
          range = new monaco.Range(
            range.startLineNumber,
            range.startColumn,
            end.range.endLineNumber,
            end.range.endColumn,
          );
        }
      }
      if (anchor.selectToEndOfLine) {
        range = new monaco.Range(
          range.startLineNumber,
          range.startColumn,
          range.endLineNumber,
          model.getLineMaxColumn(range.endLineNumber),
        );
      }
      editor.setSelection(range);
      editor.revealRangeInCenter(range);
      editor.focus();
    })();
  }, [anchor?.nonce, anchor?.startText, anchor?.endText, anchor?.selectToEndOfLine]);

  if (error) {
    return <p className="p-3 text-sm text-muted-foreground">{error}</p>;
  }
  return (
    <div className="relative h-full w-full">
      {loading && <p className="absolute p-3 text-sm text-muted-foreground">Loading…</p>}
      {banner === "deleted" && (
        <ViewerBanner message="Deleted on disk — this buffer is all that's left · ⌘S recreates the file" />
      )}
      {banner === "conflict" && (
        <ViewerBanner
          message="Changed on disk while you have unsaved edits"
          onTheirs={() => void resolveConflictRef.current?.("theirs")}
          onMine={() => void resolveConflictRef.current?.("mine")}
        />
      )}
      <div ref={containerRef} className="h-full w-full" />
      <IdeSelectionOverlay
        selection={selection}
        connected={connected}
        loading={loading}
        onSend={() => void mention()}
      />
    </div>
  );
}
