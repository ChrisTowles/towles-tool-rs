import { useEffect, useRef, useState } from "react";
import { Check, X } from "lucide-react";
import { loadMonaco } from "@/lib/monaco";
import { invokeOk } from "@/lib/tauri";
import { cn } from "@/lib/utils";

/** Payload of the `ide://open-diff` event (Claude called the openDiff tool). */
export type DiffReviewRequest = {
  requestId: number;
  dir: string;
  oldFilePath: string;
  newFilePath: string;
  newFileContents: string;
  tabName: string;
};

/**
 * Accept/reject review for one of Claude's proposed edits (the blocking
 * `openDiff` tool): original file vs proposed contents in a Monaco
 * DiffEditor. The proposed side is editable — accept saves whatever is in
 * it (tweak-then-accept, like VS Code). Resolution goes through
 * `ide_diff_resolve`, which answers the CLI's blocked tool call.
 */
export function DiffReview({
  review,
  originalContent,
  onDone,
}: {
  review: DiffReviewRequest;
  /** Current on-disk contents of the old file ("" for new files). */
  originalContent: string;
  /** Called after the review resolves (either way) or fails. */
  onDone: () => void;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const modifiedRef = useRef<import("monaco-editor").editor.ITextModel | null>(null);
  const [ready, setReady] = useState(false);

  useEffect(() => {
    let disposed = false;
    let editor: import("monaco-editor").editor.IStandaloneDiffEditor | undefined;
    let original: import("monaco-editor").editor.ITextModel | undefined;
    let modified: import("monaco-editor").editor.ITextModel | undefined;
    void (async () => {
      const monaco = await loadMonaco();
      if (disposed || !containerRef.current) return;
      original = monaco.editor.createModel(originalContent, undefined);
      modified = monaco.editor.createModel(review.newFileContents, undefined);
      modifiedRef.current = modified;
      editor = monaco.editor.createDiffEditor(containerRef.current, {
        automaticLayout: true,
        readOnly: false,
        originalEditable: false,
        renderSideBySide: true,
        minimap: { enabled: false },
        fontSize: 12,
        scrollBeyondLastLine: false,
      });
      editor.setModel({ original, modified });
      setReady(true);
    })();
    return () => {
      disposed = true;
      modifiedRef.current = null;
      editor?.dispose();
      original?.dispose();
      modified?.dispose();
    };
  }, [review.requestId, review.newFileContents, originalContent]);

  const resolve = async (accepted: boolean) => {
    await invokeOk("ide_diff_resolve", {
      requestId: review.requestId,
      accepted,
      finalContents: accepted ? (modifiedRef.current?.getValue() ?? review.newFileContents) : null,
    });
    onDone();
  };

  return (
    <div className="absolute inset-0 z-20 flex flex-col rounded-lg border border-violet-500/50 bg-background">
      <div className="flex shrink-0 items-center gap-2 border-b bg-card px-3 py-1.5">
        <span className="font-mono text-xs text-violet-500">✦</span>
        <span className="text-xs font-medium text-foreground">claude proposes an edit</span>
        <span
          className="min-w-0 truncate font-mono text-[11px] text-muted-foreground"
          title={review.newFilePath}
        >
          {review.tabName || review.newFilePath}
        </span>
        <span className="ml-auto flex shrink-0 items-center gap-1.5">
          <button
            type="button"
            onClick={() => void resolve(true)}
            disabled={!ready}
            className={cn(
              "flex items-center gap-1 rounded-md border px-2 py-0.5 text-[11px] font-medium",
              "border-emerald-600/50 bg-emerald-500/10 text-emerald-600 hover:bg-emerald-500/20 dark:text-emerald-400",
            )}
          >
            <Check className="size-3" /> accept & save
          </button>
          <button
            type="button"
            onClick={() => void resolve(false)}
            className="flex items-center gap-1 rounded-md border border-red-600/50 bg-red-500/10 px-2 py-0.5 text-[11px] font-medium text-red-600 hover:bg-red-500/20 dark:text-red-400"
          >
            <X className="size-3" /> reject
          </button>
        </span>
      </div>
      <div className="min-h-0 flex-1">
        <div ref={containerRef} className="h-full w-full" />
      </div>
      <div className="shrink-0 border-t bg-card px-3 py-1 text-[10.5px] text-muted-foreground">
        the right side is editable — accept saves exactly what it shows
      </div>
    </div>
  );
}
