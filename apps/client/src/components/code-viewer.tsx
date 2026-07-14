import { useEffect, useRef, useState } from "react";
import { loadMonaco, monacoTheme } from "@/lib/monaco";
import { ideClearSelection, ideReadFile, ideSetOpenFile, ideSetSelection } from "@/lib/ide";

/**
 * Read-only Monaco viewer for one repo file (the Files tab's right pane).
 * The whole point is the selection bridge: any Monaco selection streams to
 * the folder's Claude session as character-precise selection_changed
 * (debounced 300ms, like VS Code); collapsing it clears the context.
 * Editing lands in a later phase — the buffer is read-only.
 */
export function CodeViewer({ dir, path }: { dir: string; path: string }) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let disposed = false;
    let editor: import("monaco-editor").editor.IStandaloneCodeEditor | undefined;
    let model: import("monaco-editor").editor.ITextModel | undefined;
    let debounce: ReturnType<typeof setTimeout> | undefined;

    setError(null);
    setLoading(true);
    void (async () => {
      let content: string | null = null;
      try {
        const [, text] = await Promise.all([loadMonaco(), ideReadFile(dir, path)]);
        content = text;
      } catch (e) {
        if (!disposed) {
          setError(String(e));
          setLoading(false);
        }
        return;
      }
      if (disposed || !containerRef.current) return;
      if (content == null) {
        setError("not available in browser dev");
        setLoading(false);
        return;
      }
      const monaco = await loadMonaco();
      const uri = monaco.Uri.file(`${dir}/${path}`);
      monaco.editor.getModel(uri)?.dispose();
      model = monaco.editor.createModel(content, undefined, uri);
      editor = monaco.editor.create(containerRef.current, {
        model,
        readOnly: true,
        automaticLayout: true,
        theme: monacoTheme(),
        minimap: { enabled: false },
        fontSize: 12,
        lineNumbersMinChars: 4,
        scrollBeyondLastLine: false,
        renderLineHighlight: "none",
        occurrencesHighlight: "off",
        contextmenu: false,
      });
      setLoading(false);
      ideSetOpenFile(dir, path);

      editor.onDidChangeCursorSelection((e) => {
        clearTimeout(debounce);
        debounce = setTimeout(() => {
          const sel = e.selection;
          if (sel.isEmpty()) {
            ideClearSelection(dir, path);
            return;
          }
          // Monaco positions are 1-based lines/columns; the bridge takes
          // 1-based lines + 0-based character columns.
          ideSetSelection(
            dir,
            path,
            sel.startLineNumber,
            sel.endLineNumber,
            sel.startColumn - 1,
            sel.endColumn - 1,
          );
        }, 300);
      });
    })();

    return () => {
      disposed = true;
      clearTimeout(debounce);
      editor?.dispose();
      model?.dispose();
      ideSetOpenFile(dir, null);
    };
  }, [dir, path]);

  if (error) {
    return <p className="p-3 text-sm text-muted-foreground">{error}</p>;
  }
  return (
    <div className="relative h-full w-full">
      {loading && <p className="absolute p-3 text-sm text-muted-foreground">Loading…</p>}
      <div ref={containerRef} className="h-full w-full" />
    </div>
  );
}
