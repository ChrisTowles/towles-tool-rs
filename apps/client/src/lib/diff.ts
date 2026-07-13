/**
 * Unified-diff parsing for the Agentboard diff viewer: split `git diff` output
 * into per-file sections with change-type + line counts, so the dialog can
 * render a plannotator-style file rail next to the patch.
 */

export type DiffLineKind = "add" | "del" | "hunk" | "meta" | "ctx";

export type DiffLine = { kind: DiffLineKind; text: string };

export type DiffFileStatus = "modified" | "added" | "deleted" | "renamed";

export type DiffFile = {
  /** New path (post-change); the old path for pure deletions. */
  path: string;
  /** Pre-rename path, set only for renames. */
  oldPath?: string;
  status: DiffFileStatus;
  additions: number;
  deletions: number;
  /** Hunk headers + body lines (the `diff --git`/index/±±± preamble is meta). */
  lines: DiffLine[];
};

/** Strip git's `a/` / `b/` prefix from a diff header path. */
function stripPrefix(p: string): string {
  return p.replace(/^[ab]\//, "");
}

/** Both paths off a `diff --git a/<old> b/<new>` line. Paths with spaces are
 * split on the ` b/` boundary (quoted paths keep their quotes stripped). */
function headerPaths(line: string): { oldPath: string; newPath: string } {
  const body = line.slice("diff --git ".length).replace(/"/g, "");
  const idx = body.lastIndexOf(" b/");
  if (idx < 0) return { oldPath: body, newPath: body };
  return {
    oldPath: stripPrefix(body.slice(0, idx)),
    newPath: stripPrefix(body.slice(idx + 1)),
  };
}

/** Parse a full unified diff into per-file sections. Tolerant of anything it
 * doesn't recognize (unrecognized preamble lines become `meta`). */
export function parseDiff(text: string): DiffFile[] {
  const files: DiffFile[] = [];
  let cur: DiffFile | null = null;
  let inBody = false;

  for (const line of text.split("\n")) {
    if (line.startsWith("diff --git ")) {
      const { oldPath, newPath } = headerPaths(line);
      cur = {
        path: newPath,
        status: "modified",
        additions: 0,
        deletions: 0,
        lines: [{ kind: "meta", text: line }],
      };
      if (oldPath !== newPath) {
        cur.status = "renamed";
        cur.oldPath = oldPath;
      }
      files.push(cur);
      inBody = false;
      continue;
    }
    if (!cur) continue;

    if (!inBody) {
      if (line.startsWith("new file mode")) cur.status = "added";
      else if (line.startsWith("deleted file mode")) cur.status = "deleted";
      else if (line.startsWith("rename from")) {
        cur.status = "renamed";
        cur.oldPath = line.slice("rename from ".length);
      }
      if (line.startsWith("@@")) {
        inBody = true;
        cur.lines.push({ kind: "hunk", text: line });
      } else {
        cur.lines.push({ kind: "meta", text: line });
      }
      continue;
    }

    if (line.startsWith("@@")) {
      cur.lines.push({ kind: "hunk", text: line });
    } else if (line.startsWith("+")) {
      cur.additions += 1;
      cur.lines.push({ kind: "add", text: line });
    } else if (line.startsWith("-")) {
      cur.deletions += 1;
      cur.lines.push({ kind: "del", text: line });
    } else {
      cur.lines.push({ kind: "ctx", text: line });
    }
  }

  return files;
}

/** One row of a side-by-side rendering: either a left/right pair (a del and
 * an add lined up together, blanks where one side has no counterpart) or a
 * `full` line (hunk header, meta, or unchanged context) that spans both
 * columns. */
export type SplitDiffRow =
  | { full: DiffLine }
  | { left: DiffLine | null; right: DiffLine | null };

/** Pair up a file's flat line list into split-view rows: consecutive `del`
 * runs line up against the following `add` run positionally (GitHub's split
 * diff behavior), padding the shorter side with blanks. Anything else (ctx,
 * hunk, meta) flushes the pending pair and spans full width. */
export function pairDiffLines(lines: DiffLine[]): SplitDiffRow[] {
  const rows: SplitDiffRow[] = [];
  let dels: DiffLine[] = [];
  let adds: DiffLine[] = [];

  const flush = () => {
    const count = Math.max(dels.length, adds.length);
    for (let i = 0; i < count; i++) {
      rows.push({ left: dels[i] ?? null, right: adds[i] ?? null });
    }
    dels = [];
    adds = [];
  };

  for (const line of lines) {
    if (line.kind === "del") {
      dels.push(line);
      continue;
    }
    if (line.kind === "add") {
      adds.push(line);
      continue;
    }
    flush();
    rows.push({ full: line });
  }
  flush();

  return rows;
}
