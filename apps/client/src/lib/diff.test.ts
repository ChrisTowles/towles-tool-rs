import { describe, expect, it } from "vitest";
import { buildDiffTree, parseDiff, type DiffFile } from "./diff";

const SAMPLE = `diff --git a/src/a.ts b/src/a.ts
index 111..222 100644
--- a/src/a.ts
+++ b/src/a.ts
@@ -1,3 +1,4 @@
 context
+added line
-removed line
 more context
diff --git a/src/new.ts b/src/new.ts
new file mode 100644
index 000..333
--- /dev/null
+++ b/src/new.ts
@@ -0,0 +1,2 @@
+first
+second
diff --git a/src/old-name.ts b/src/new-name.ts
similarity index 90%
rename from src/old-name.ts
rename to src/new-name.ts
index 444..555 100644
--- a/src/old-name.ts
+++ b/src/new-name.ts
@@ -1 +1 @@
-old
+new
diff --git a/src/gone.ts b/src/gone.ts
deleted file mode 100644
index 666..000
--- a/src/gone.ts
+++ /dev/null
@@ -1,1 +0,0 @@
-goodbye
`;

describe("parseDiff", () => {
  it("splits a multi-file diff into per-file sections", () => {
    const files = parseDiff(SAMPLE);
    expect(files.map((f) => f.path)).toEqual([
      "src/a.ts",
      "src/new.ts",
      "src/new-name.ts",
      "src/gone.ts",
    ]);
  });

  it("classifies file statuses", () => {
    const [modified, added, renamed, deleted] = parseDiff(SAMPLE);
    expect(modified.status).toBe("modified");
    expect(added.status).toBe("added");
    expect(renamed.status).toBe("renamed");
    expect(renamed.oldPath).toBe("src/old-name.ts");
    expect(deleted.status).toBe("deleted");
  });

  it("counts additions and deletions per file, ignoring the ±±± preamble", () => {
    const [modified, added, , deleted] = parseDiff(SAMPLE);
    expect({ add: modified.additions, del: modified.deletions }).toEqual({ add: 1, del: 1 });
    expect({ add: added.additions, del: added.deletions }).toEqual({ add: 2, del: 0 });
    expect({ add: deleted.additions, del: deleted.deletions }).toEqual({ add: 0, del: 1 });
  });

  it("tags body lines by kind for rendering", () => {
    const [modified] = parseDiff(SAMPLE);
    const kinds = modified.lines.map((l) => l.kind);
    expect(kinds).toContain("hunk");
    expect(kinds).toContain("add");
    expect(kinds).toContain("del");
    expect(kinds).toContain("ctx");
  });

  it("returns an empty list for an empty diff", () => {
    expect(parseDiff("")).toEqual([]);
  });

  it("numbers body lines against both file versions from the hunk header", () => {
    const [modified] = parseDiff(SAMPLE);
    // @@ -1,3 +1,4 @@ → ctx(1/1), add(new 2), del(old 2), ctx(3/3)
    const body = modified.lines.filter((l) => l.kind !== "meta" && l.kind !== "hunk");
    expect(body.map((l) => [l.kind, l.oldLine, l.newLine])).toEqual([
      ["ctx", 1, 1],
      ["add", undefined, 2],
      ["del", 2, undefined],
      ["ctx", 3, 3],
    ]);
  });

  it("restarts numbering at each hunk header", () => {
    const twoHunks = [
      "diff --git a/f.ts b/f.ts",
      "index 1..2 100644",
      "--- a/f.ts",
      "+++ b/f.ts",
      "@@ -1,1 +1,1 @@",
      "-a",
      "+b",
      "@@ -10,2 +10,3 @@",
      " ten",
      "+ten-and-a-half",
      " eleven",
    ].join("\n");
    const [file] = parseDiff(twoHunks);
    const adds = file.lines.filter((l) => l.kind === "add");
    expect(adds.map((l) => l.newLine)).toEqual([1, 11]);
    const ctx = file.lines.filter((l) => l.kind === "ctx");
    expect(ctx.map((l) => [l.oldLine, l.newLine])).toEqual([
      [10, 10],
      [11, 12],
    ]);
  });

  it("leaves the no-newline marker unnumbered", () => {
    const marker = [
      "diff --git a/f.ts b/f.ts",
      "index 1..2 100644",
      "--- a/f.ts",
      "+++ b/f.ts",
      "@@ -1,1 +1,1 @@",
      "-a",
      "+b",
      "\\ No newline at end of file",
    ].join("\n");
    const [file] = parseDiff(marker);
    const markerLine = file.lines.find((l) => l.text.startsWith("\\"));
    expect(markerLine?.kind).toBe("ctx");
    expect(markerLine?.oldLine).toBeUndefined();
    expect(markerLine?.newLine).toBeUndefined();
  });
});

function fakeFile(path: string): DiffFile {
  return { path, status: "modified", additions: 1, deletions: 1, lines: [] };
}

describe("buildDiffTree", () => {
  it("groups files under their shared directories", () => {
    const files = [fakeFile("src/a.ts"), fakeFile("src/b.ts"), fakeFile("README.md")];
    const tree = buildDiffTree(files);
    expect(tree.map((n) => n.name)).toEqual(["src", "README.md"]);
    const src = tree[0];
    if (src.kind !== "folder") throw new Error("expected folder");
    expect(src.children.map((n) => n.name)).toEqual(["a.ts", "b.ts"]);
  });

  it("collapses single-child directory chains into one row", () => {
    const files = [fakeFile("apps/client/src/lib/diff.ts")];
    const tree = buildDiffTree(files);
    expect(tree).toHaveLength(1);
    expect(tree[0]).toMatchObject({ kind: "folder", name: "apps/client/src/lib" });
  });

  it("does not collapse a directory that holds a file alongside a subfolder", () => {
    const files = [fakeFile("src/index.ts"), fakeFile("src/lib/diff.ts")];
    const tree = buildDiffTree(files);
    expect(tree).toHaveLength(1);
    const src = tree[0];
    if (src.kind !== "folder") throw new Error("expected folder");
    expect(src.name).toBe("src");
    expect(src.children.map((n) => n.name)).toEqual(["lib", "index.ts"]);
  });

  it("keeps each file's index into the original flat array", () => {
    const files = [fakeFile("src/a.ts"), fakeFile("src/b.ts")];
    const tree = buildDiffTree(files);
    const src = tree[0];
    if (src.kind !== "folder") throw new Error("expected folder");
    const [a, b] = src.children;
    if (a.kind !== "file" || b.kind !== "file") throw new Error("expected files");
    expect([a.index, b.index]).toEqual([0, 1]);
  });
});
