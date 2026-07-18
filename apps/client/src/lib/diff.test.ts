import { describe, expect, it } from "vitest";
import { buildDiffTree } from "./diff";

describe("buildDiffTree", () => {
  it("groups files under their shared directories", () => {
    const tree = buildDiffTree(["src/a.ts", "src/b.ts", "README.md"]);
    expect(tree.map((n) => n.name)).toEqual(["src", "README.md"]);
    const src = tree[0];
    if (src.kind !== "folder") throw new Error("expected folder");
    expect(src.children.map((n) => n.name)).toEqual(["a.ts", "b.ts"]);
  });

  it("collapses single-child directory chains into one row", () => {
    const tree = buildDiffTree(["apps/client/src/lib/diff.ts"]);
    expect(tree).toHaveLength(1);
    expect(tree[0]).toMatchObject({ kind: "folder", name: "apps/client/src/lib" });
  });

  it("does not collapse a directory that holds a file alongside a subfolder", () => {
    const tree = buildDiffTree(["src/index.ts", "src/lib/diff.ts"]);
    expect(tree).toHaveLength(1);
    const src = tree[0];
    if (src.kind !== "folder") throw new Error("expected folder");
    expect(src.name).toBe("src");
    expect(src.children.map((n) => n.name)).toEqual(["lib", "index.ts"]);
  });

  it("keeps each file's index into the original flat array", () => {
    const tree = buildDiffTree(["src/a.ts", "src/b.ts"]);
    const src = tree[0];
    if (src.kind !== "folder") throw new Error("expected folder");
    const [a, b] = src.children;
    if (a.kind !== "file" || b.kind !== "file") throw new Error("expected files");
    expect([a.index, b.index]).toEqual([0, 1]);
  });
});
