import { useEffect, useRef, useState } from "react";
import { loadMonaco } from "@/lib/monaco";
import { abInvoke } from "@/lib/agentboard";
import { ideClearSelection, ideReadFile, ideSetSelection } from "@/lib/ide";

/** One changed file from `ab_get_diff_files` (tt_agentboard::DiffFile). */
export type ChangedFile = {
  path: string;
  oldPath: string | null;
  /** Git name-status letter (M/A/D/R/C/T), or "?" for untracked. */
  status: string;
  linesAdded: number;
  linesRemoved: number;
};

/**
 * VS Code's diff editor for one changed file in the diff pane: original side
 * is the file at the diff baseline (`ab_get_base_file`), modified side is the
 * working tree (`ide_read_file`). Read-only — edits belong in the Files tab's
 * CodeViewer. Selections on the modified side stream to the folder's Claude
 * session, same contract as CodeViewer. `refreshKey` refetches both sides in
 * place (scroll preserved) when the working tree measurably changes.
 */
export function MonacoFileDiff({
  dir,
  file,
  mode,
  baseBranch,
  refreshKey,
}: {
  dir: string;
  file: ChangedFile;
  mode: string;
  baseBranch: string | null;
  refreshKey: string;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const editorRef = useRef<import("monaco-editor").editor.IStandaloneDiffEditor | null>(null);

  useEffect(() => {
    let disposed = false;
    let editor: import("monaco-editor").editor.IStandaloneDiffEditor | undefined;
    let original: import("monaco-editor").editor.ITextModel | undefined;
    let modified: import("monaco-editor").editor.ITextModel | undefined;
    let debounce: ReturnType<typeof setTimeout> | undefined;

    setError(null);
    setLoading(true);
    void (async () => {
      let contents: { original: string; modified: string };
      try {
        const [, fetched] = await Promise.all([
          loadMonaco(),
          fetchSides(dir, file, mode, baseBranch),
        ]);
        contents = fetched;
      } catch (e) {
        if (!disposed) {
          setError(String(e));
          setLoading(false);
        }
        return;
      }
      if (disposed || !containerRef.current) return;
      const monaco = await loadMonaco();
      // Own scheme so these never collide with the CodeViewer's file: models
      // of the same path.
      const baseUri = monaco.Uri.parse(
        `tt-diff-base:${dir}/${file.oldPath ?? file.path}`,
      );
      const workUri = monaco.Uri.parse(`tt-diff-work:${dir}/${file.path}`);
      monaco.editor.getModel(baseUri)?.dispose();
      monaco.editor.getModel(workUri)?.dispose();
      original = monaco.editor.createModel(contents.original, undefined, baseUri);
      modified = monaco.editor.createModel(contents.modified, undefined, workUri);
      editor = monaco.editor.createDiffEditor(containerRef.current, {
        automaticLayout: true,
        readOnly: true,
        renderSideBySide: true,
        minimap: { enabled: false },
        fontSize: 12,
        scrollBeyondLastLine: false,
        contextmenu: false,
      });
      editor.setModel({ original, modified });
      editorRef.current = editor;
      setLoading(false);

      editor.getModifiedEditor().onDidChangeCursorSelection((e) => {
        clearTimeout(debounce);
        debounce = setTimeout(() => {
          const sel = e.selection;
          if (sel.isEmpty()) {
            ideClearSelection(dir, file.path);
            return;
          }
          ideSetSelection(
            dir,
            file.path,
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
      editorRef.current = null;
      editor?.dispose();
      original?.dispose();
      modified?.dispose();
      ideClearSelection(dir, file.path);
    };
    // refreshKey is handled by the setValue effect below, not a rebuild.
  }, [dir, file.path, file.oldPath, file.status, mode, baseBranch]);

  // Working tree changed under the pane — refresh both sides in place.
  useEffect(() => {
    const editor = editorRef.current;
    if (!editor) return;
    void (async () => {
      try {
        const contents = await fetchSides(dir, file, mode, baseBranch);
        const model = editor.getModel();
        if (!model || editorRef.current !== editor) return;
        if (model.original.getValue() !== contents.original) {
          model.original.setValue(contents.original);
        }
        if (model.modified.getValue() !== contents.modified) {
          model.modified.setValue(contents.modified);
        }
      } catch {
        // Transient refresh failure (file mid-write) — keep showing the last
        // good contents; the next stats bump retries.
      }
    })();
  }, [refreshKey]);

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

/** Both sides of the diff: base content (empty for added/untracked) and
 * working-tree content (empty for deleted). */
async function fetchSides(
  dir: string,
  file: ChangedFile,
  mode: string,
  baseBranch: string | null,
): Promise<{ original: string; modified: string }> {
  const added = file.status === "A" || file.status === "?";
  const [original, read] = await Promise.all([
    added
      ? Promise.resolve(null)
      : abInvoke<string | null>("ab_get_base_file", {
          dir,
          mode,
          baseBranch,
          path: file.oldPath ?? file.path,
        }),
    file.status === "D" ? Promise.resolve(null) : ideReadFile(dir, file.path),
  ]);
  return { original: original ?? "", modified: read?.content ?? "" };
}
