import { useEffect, useRef, useState } from "react";
import { loadMonaco } from "@/lib/monaco";
import {
  ideClearSelection,
  ideMention,
  ideReadFile,
  ideSetOpenFile,
  ideSetSelection,
  ideWriteFile,
} from "@/lib/ide";
import { IdeSelectionOverlay } from "@/components/ide-selection-chip";
import {
  mentionRangeFrom,
  sameMentionRange,
  streamRangeFrom,
  type MentionRange,
} from "@/lib/ide-selection";

/**
 * Monaco editor for one repo file (the Files tab's right pane). Two bridges:
 * selections stream to the folder's Claude session as character-precise
 * selection_changed (debounced 300ms, like VS Code), and edits save with
 * Cmd/Ctrl+S — atomically, refused if the file changed on disk since it was
 * read (an agent may be editing the same tree). Dirty state rides to Claude
 * via getOpenEditors / checkDocumentDirty.
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
  const editorRef = useRef<import("monaco-editor").editor.IStandaloneCodeEditor | null>(null);
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
  const mention = async () => {
    const editor = editorRef.current;
    if (!editor) return;
    await ideMention(dir, path, mentionRangeFrom(editor.getSelection()));
  };

  useEffect(() => {
    let disposed = false;
    let editor: import("monaco-editor").editor.IStandaloneCodeEditor | undefined;
    let model: import("monaco-editor").editor.ITextModel | undefined;
    let debounce: ReturnType<typeof setTimeout> | undefined;
    /** Whether a selection is currently parked in the session's context. */
    let streamed = false;

    setError(null);
    setLoading(true);
    setSelection(null);
    dirtyRef.current = false;
    void (async () => {
      let read: Awaited<ReturnType<typeof ideReadFile>>;
      try {
        const [, r] = await Promise.all([loadMonaco(), ideReadFile(dir, path)]);
        read = r;
      } catch (e) {
        if (!disposed) {
          setError(String(e));
          setLoading(false);
        }
        return;
      }
      if (disposed || !containerRef.current) return;
      if (read == null) {
        setError("not available in browser dev");
        setLoading(false);
        return;
      }
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

      model.onDidChangeContent(() => {
        setDirty(model!.getAlternativeVersionId() !== savedVersionRef.current);
      });

      const save = async () => {
        if (!model) return;
        const versionAtSave = model.getAlternativeVersionId();
        const newMtime = await ideWriteFile(dir, path, model.getValue(), mtimeRef.current);
        if (newMtime == null || !model || model.isDisposed()) return;
        mtimeRef.current = newMtime;
        savedVersionRef.current = versionAtSave;
        setDirty(model.getAlternativeVersionId() !== versionAtSave);
      };
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
      editorRef.current = null;
      editor?.dispose();
      model?.dispose();
      // Closing a file with text selected must not leave that range as the
      // folder's ambient context — getLatestSelection would keep serving it
      // into the next prompt.
      if (streamed) ideClearSelection(dir, path);
      ideSetOpenFile(dir, null);
    };
  }, [dir, path]);

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
