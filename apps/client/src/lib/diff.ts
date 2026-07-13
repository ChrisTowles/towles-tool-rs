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

/** A row in the file rail's tree rendering: a directory (with its children)
 * or a leaf file. `index` is the file's position in the flat `DiffFile[]`
 * the tree was built from, so selection state stays keyed by that array. */
export type DiffTreeNode =
  | { kind: "folder"; name: string; path: string; children: DiffTreeNode[] }
  | { kind: "file"; name: string; path: string; index: number };

type BuildingFolder = {
  name: string;
  path: string;
  folders: Map<string, BuildingFolder>;
  files: DiffTreeNode[];
};

/** Directory chains with only one child directory and no files of their own
 * collapse into a single row (`src/components` instead of `src` > `components`),
 * matching VS Code / GitHub's "compact folders" tree rendering. */
function collapseSingleChildChain(name: string, path: string, children: DiffTreeNode[]): DiffTreeNode {
  let mergedName = name;
  let mergedPath = path;
  let mergedChildren = children;
  while (mergedChildren.length === 1 && mergedChildren[0].kind === "folder") {
    const only = mergedChildren[0];
    mergedName = `${mergedName}/${only.name}`;
    mergedPath = only.path;
    mergedChildren = only.children;
  }
  return { kind: "folder", name: mergedName, path: mergedPath, children: mergedChildren };
}

/** Group a flat file list into a directory tree for the file rail: folders
 * sort before files, both alphabetically within their level. */
export function buildDiffTree(files: DiffFile[]): DiffTreeNode[] {
  const root: BuildingFolder = { name: "", path: "", folders: new Map(), files: [] };

  files.forEach((file, index) => {
    const segments = file.path.split("/");
    let node = root;
    for (let i = 0; i < segments.length - 1; i++) {
      const seg = segments[i];
      const path = node.path ? `${node.path}/${seg}` : seg;
      let child = node.folders.get(seg);
      if (!child) {
        child = { name: seg, path, folders: new Map(), files: [] };
        node.folders.set(seg, child);
      }
      node = child;
    }
    const name = segments[segments.length - 1];
    node.files.push({ kind: "file", name, path: file.path, index });
  });

  function finalize(node: BuildingFolder): DiffTreeNode[] {
    const folderNodes = Array.from(node.folders.values())
      .sort((a, b) => a.name.localeCompare(b.name))
      .map((f) => collapseSingleChildChain(f.name, f.path, finalize(f)));
    const fileNodes = [...node.files].sort((a, b) => a.name.localeCompare(b.name));
    return [...folderNodes, ...fileNodes];
  }

  return finalize(root);
}
