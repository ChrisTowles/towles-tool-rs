import { useEffect, useRef, useState } from "react";
import { loadMonaco } from "@/lib/monaco";
import { abInvoke } from "@/lib/agentboard";
import { ideClearSelection, ideMention, ideReadFile, ideSetSelection } from "@/lib/ide";
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
type TextModel = import("monaco-editor").editor.ITextModel;

/**
 * VS Code's multi-diff editor over the whole change set: every file's diff
 * stacked in one scroll with sticky per-file headers, exactly the surface
 * VS Code uses for "view changes". Original sides come from the diff
 * baseline (`ab_get_base_file`), modified sides from the working tree —
 * read-only; edits belong in the Files tab. Selections on any modified side
 * stream to the folder's Claude session, and the selection chip (or ⌘⇧A)
 * mentions those lines explicitly — same contract as CodeViewer.
 * `refreshKey` refetches contents in place when the working tree measurably
 * changes; the set of files changing rebuilds the widget.
 */
export function MonacoMultiDiff({
  dir,
  files,
  mode,
  baseBranch,
  refreshKey,
  connected = false,
  registerReveal,
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
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  /** Which file's lines are highlighted — the multi-diff stacks many files,
   * so the chip has to name one. */
  const [selection, setSelection] = useState<{ path: string; range: MentionRange } | null>(null);
  const mentionRef = useRef<() => void>(() => {});
  const widgetRef = useRef<Widget | null>(null);
  const modelsRef = useRef<Map<string, { original?: TextModel; modified?: TextModel }>>(new Map());

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
          }
          models.set(f.path, entry);
          return {
            original: entry.original,
            modified: entry.modified,
            options: { readOnly: true, originalEditable: false },
          };
        });
        modelsRef.current = models;

        const widget = api.createInstanceSync(
          widgetMod.MultiDiffEditorWidget,
          containerRef.current,
          {
            headerClickToCollapse: true,
            createResourceLabel: (element: HTMLElement) => ({
              setUri(uri: { path: string } | undefined) {
                element.textContent = uri ? uri.path.replace(`${dir}/`, "") : "";
              },
              dispose() {},
            }),
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
          // Same ⌘⇧A chord as the file viewer. These are plain ICodeEditors
          // inside the multi-diff, not standalone ones, so there's no
          // addCommand — match the chord on the key event instead.
          disposables.push(
            modified.onKeyDown((e: import("monaco-editor").IKeyboardEvent) => {
              if (e.keyCode !== monaco.KeyCode.KeyA || !e.shiftKey) return;
              if (!(e.ctrlKey || e.metaKey)) return;
              e.preventDefault();
              e.stopPropagation();
              void mention();
            }),
          );

          disposables.push(
            modified.onDidChangeCursorSelection(
              (e: import("monaco-editor").editor.ICursorSelectionChangedEvent) => {
                // Resolve the file outside the debounce so the chip tracks the
                // selection immediately; only bridge traffic is debounced.
                const path = diffWorkPath(dir, modified.getModel()?.uri);
                if (!path) return;
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
          setError(String(e));
          setLoading(false);
        }
      }
    })();

    return () => {
      disposed = true;
      widgetRef.current = null;
      for (const d of disposables.toReversed()) d.dispose();
      for (const entry of modelsRef.current.values()) {
        entry.original?.dispose();
        entry.modified?.dispose();
      }
      modelsRef.current = new Map();
      mentionRef.current = () => {};
      setSelection(null);
      if (streamedPath != null) ideClearSelection(dir, streamedPath);
    };
    // filesKey stands in for `files`; refreshKey is the in-place path below.
    // registerReveal is deliberately excluded too: it's an unmemoized callback
    // prop from the parent, so listing it would rebuild this expensive widget
    // on every parent render instead of only on a real file-set/branch change.
    // oxlint-disable-next-line react-hooks/exhaustive-deps
  }, [dir, filesKey, mode, baseBranch]);

  // Working tree changed under the pane — refresh contents in place so the
  // scroll position and collapse states survive.
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
          if (
            entry.modified &&
            sides.modified != null &&
            entry.modified.getValue() !== sides.modified
          ) {
            entry.modified.setValue(sides.modified);
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
 * added/untracked) and working-tree content (undefined for deleted). */
async function fetchSides(
  dir: string,
  file: ChangedFile,
  mode: string,
  baseBranch: string | null,
): Promise<{ original: string | undefined; modified: string | undefined }> {
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
    file.status === "D" ? Promise.resolve(null) : ideReadFile(dir, file.path).catch(() => null),
  ]);
  return {
    original: added ? undefined : (original ?? ""),
    modified: file.status === "D" ? undefined : (read?.content ?? ""),
  };
}
