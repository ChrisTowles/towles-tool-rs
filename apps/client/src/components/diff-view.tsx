import { useEffect, useMemo, useState } from "react";
import { parseDiff, type DiffFile, type DiffLine } from "@/lib/diff";
import { cn } from "@/lib/utils";

/**
 * Plannotator-style diff viewer: a file rail (change-type letter · path ·
 * right-anchored ± counts) next to the selected file's patch. Colors are
 * fixed for light + dark via explicit dark: variants — never raw green/red
 * text on its own tinted background, which went unreadable in light mode.
 */

/** Leading change-type letter, fixed slot so paths align (plannotator's
 * FileRowBits language: A/D/R carry weight + color, M stays whisper-quiet). */
function ChangeTypeLetter({ file }: { file: DiffFile }) {
  const map = {
    added: { ch: "A", cls: "font-semibold text-emerald-600 dark:text-emerald-400" },
    deleted: { ch: "D", cls: "font-semibold text-red-600 dark:text-red-400" },
    renamed: { ch: "R", cls: "font-semibold text-sky-600 dark:text-sky-400" },
    modified: { ch: "M", cls: "text-muted-foreground/50" },
  } as const;
  const { ch, cls } = map[file.status];
  const title =
    file.status === "renamed" && file.oldPath
      ? `Renamed from ${file.oldPath}`
      : `${file.status[0].toUpperCase()}${file.status.slice(1)} file`;
  return (
    <span className={cn("w-3 shrink-0 text-center font-mono text-[10px]", cls)} title={title}>
      {ch}
    </span>
  );
}

/** Right-anchored ± pair in one fixed-width block, so counts end flush at the
 * row edge and add-only rows leave no phantom gap. */
function DiffCounts({ additions, deletions }: { additions: number; deletions: number }) {
  return (
    <span className="min-w-[7ch] shrink-0 whitespace-nowrap text-right font-mono text-[10px] tabular-nums">
      {additions > 0 && <span className="text-emerald-600 dark:text-emerald-400">+{additions}</span>}
      {additions > 0 && deletions > 0 && <span> </span>}
      {deletions > 0 && <span className="text-red-600 dark:text-red-400">−{deletions}</span>}
    </span>
  );
}

/** File path that keeps the filename visible: the directory part absorbs all
 * truncation, so a deep path still ends in its actual file name. */
function TruncatedPath({ path }: { path: string }) {
  const slash = path.lastIndexOf("/");
  if (slash === -1) return <span className="truncate">{path}</span>;
  return (
    <span className="flex min-w-0 items-center">
      <span className="truncate text-muted-foreground">{path.slice(0, slash)}</span>
      <span className="max-w-full shrink-0 truncate">{path.slice(slash)}</span>
    </span>
  );
}

const LINE_CLS: Record<DiffLine["kind"], string> = {
  add: "bg-emerald-500/10 text-emerald-700 dark:text-emerald-300",
  del: "bg-red-500/10 text-red-700 dark:text-red-300",
  hunk: "bg-sky-500/10 text-sky-700 dark:text-sky-300",
  meta: "text-muted-foreground/70",
  ctx: "text-foreground/80",
};

function FilePatch({ file }: { file: DiffFile }) {
  return (
    <pre className="min-w-max p-2 font-mono text-xs leading-relaxed whitespace-pre">
      {file.lines.map((line, i) => (
        <div key={i} className={cn("px-2", LINE_CLS[line.kind])}>
          {line.text || " "}
        </div>
      ))}
    </pre>
  );
}

export function DiffViewer({ text }: { text: string }) {
  const files = useMemo(() => parseDiff(text), [text]);
  const [selected, setSelected] = useState(0);
  // A new diff (dialog re-opened for another folder) resets the selection.
  useEffect(() => setSelected(0), [text]);
  const file = files[Math.min(selected, files.length - 1)];

  if (files.length === 0) {
    return <p className="p-4 text-sm text-muted-foreground">No changes.</p>;
  }

  return (
    <div className="flex min-h-0 flex-1 overflow-hidden rounded-lg border">
      <div className="flex w-64 shrink-0 flex-col overflow-y-auto border-r bg-card">
        <div className="sticky top-0 flex items-center justify-between border-b bg-card px-3 py-1.5">
          <span className="text-[11px] font-medium text-muted-foreground">
            {files.length} file{files.length === 1 ? "" : "s"}
          </span>
          <DiffCounts
            additions={files.reduce((s, f) => s + f.additions, 0)}
            deletions={files.reduce((s, f) => s + f.deletions, 0)}
          />
        </div>
        {files.map((f, i) => (
          <button
            key={f.path}
            type="button"
            onClick={() => setSelected(i)}
            className={cn(
              "flex items-center gap-2 border-l-2 border-transparent px-2 py-1.5 text-left text-xs",
              i === selected
                ? "border-l-violet-500 bg-accent text-foreground"
                : "text-muted-foreground hover:bg-accent/50",
            )}
          >
            <ChangeTypeLetter file={f} />
            <span className="min-w-0 flex-1">
              <TruncatedPath path={f.path} />
            </span>
            <DiffCounts additions={f.additions} deletions={f.deletions} />
          </button>
        ))}
      </div>
      <div className="flex min-w-0 flex-1 flex-col">
        <div className="flex shrink-0 items-center gap-2 border-b bg-card px-3 py-1.5">
          <ChangeTypeLetter file={file} />
          <span className="min-w-0 truncate font-mono text-xs text-foreground">{file.path}</span>
          {file.oldPath && (
            <span className="truncate font-mono text-[10.5px] text-muted-foreground">
              ← {file.oldPath}
            </span>
          )}
          <span className="ml-auto" />
          <DiffCounts additions={file.additions} deletions={file.deletions} />
        </div>
        <div className="min-h-0 flex-1 overflow-auto">
          <FilePatch file={file} />
        </div>
      </div>
    </div>
  );
}
