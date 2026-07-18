/**
 * File-tree grouping for path lists (the Files pane's rail). The unified-diff
 * parser that used to live here died with the hand-rolled diff renderer — the
 * diff pane now uses the VS Code diff editor over full file contents.
 */

/** A row in a file rail's tree rendering: a directory (with its children) or
 * a leaf file. `index` is the file's position in the flat path list the tree
 * was built from, so selection state stays keyed by that array. */
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
function collapseSingleChildChain(
  name: string,
  path: string,
  children: DiffTreeNode[],
): DiffTreeNode {
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

/** Group a flat path list into a directory tree for a file rail: folders
 * sort before files, both alphabetically within their level. */
export function buildDiffTree(paths: string[]): DiffTreeNode[] {
  const root: BuildingFolder = { name: "", path: "", folders: new Map(), files: [] };

  paths.forEach((filePath, index) => {
    const segments = filePath.split("/");
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
    node.files.push({ kind: "file", name, path: filePath, index });
  });

  function finalize(node: BuildingFolder): DiffTreeNode[] {
    const folderNodes = Array.from(node.folders.values())
      .toSorted((a, b) => a.name.localeCompare(b.name))
      .map((f) => collapseSingleChildChain(f.name, f.path, finalize(f)));
    const fileNodes = [...node.files].toSorted((a, b) => a.name.localeCompare(b.name));
    return [...folderNodes, ...fileNodes];
  }

  return finalize(root);
}
