import { describe, expect, it } from "vitest";
import {
  diffWorkPath,
  formatLineRange,
  formatMentionRef,
  mentionRangeFrom,
  sameMentionRange,
  streamRangeFrom,
  type MonacoSelectionLike,
} from "@/lib/ide-selection";

const sel = (
  startLineNumber: number,
  startColumn: number,
  endLineNumber: number,
  endColumn: number,
): MonacoSelectionLike => ({ startLineNumber, startColumn, endLineNumber, endColumn });

describe("streamRangeFrom", () => {
  it("keeps lines 1-based and drops columns to 0-based", () => {
    expect(streamRangeFrom(sel(12, 5, 40, 9))).toEqual({
      startLine: 12,
      endLine: 40,
      startChar: 4,
      endChar: 8,
    });
  });

  it("maps column 1 to character 0", () => {
    expect(streamRangeFrom(sel(3, 1, 3, 1))).toMatchObject({ startChar: 0, endChar: 0 });
  });
});

describe("mentionRangeFrom", () => {
  it("returns null for an empty selection, so the caller mentions the whole file", () => {
    expect(mentionRangeFrom(sel(12, 5, 12, 5))).toBeNull();
  });

  it("returns null for no selection at all", () => {
    expect(mentionRangeFrom(null)).toBeNull();
    expect(mentionRangeFrom(undefined)).toBeNull();
  });

  it("keeps a single-line selection on one line", () => {
    expect(mentionRangeFrom(sel(12, 3, 12, 20))).toEqual({ startLine: 12, endLine: 12 });
  });

  it("spans a multi-line selection", () => {
    expect(mentionRangeFrom(sel(12, 3, 40, 20))).toEqual({ startLine: 12, endLine: 40 });
  });

  // Triple-click / shift+down parks the caret in column 1 of the next line.
  it("drops a trailing line the user never actually selected", () => {
    expect(mentionRangeFrom(sel(12, 1, 41, 1))).toEqual({ startLine: 12, endLine: 40 });
  });

  it("does not trim a genuine single-line selection ending at column 1", () => {
    expect(mentionRangeFrom(sel(12, 1, 12, 1))).toBeNull();
  });

  it("normalizes a backwards (bottom-up) selection", () => {
    expect(mentionRangeFrom(sel(40, 9, 12, 5))).toEqual({ startLine: 12, endLine: 40 });
  });
});

describe("formatLineRange", () => {
  it("shows a single line bare", () => {
    expect(formatLineRange({ startLine: 12, endLine: 12 })).toBe("L12");
  });

  it("joins a span with an en dash", () => {
    expect(formatLineRange({ startLine: 12, endLine: 40 })).toBe("L12–40");
  });
});

describe("formatMentionRef", () => {
  it("is the bare path with no range", () => {
    expect(formatMentionRef("src/app.ts", null)).toBe("src/app.ts");
  });

  it("uses an ASCII hyphen — Claude parses this, it isn't display text", () => {
    expect(formatMentionRef("src/app.ts", { startLine: 12, endLine: 40 })).toBe(
      "src/app.ts#L12-40",
    );
  });

  it("collapses a single-line range", () => {
    expect(formatMentionRef("src/app.ts", { startLine: 12, endLine: 12 })).toBe("src/app.ts#L12");
  });
});

describe("sameMentionRange", () => {
  it("treats equal ranges as the same", () => {
    expect(sameMentionRange({ startLine: 1, endLine: 2 }, { startLine: 1, endLine: 2 })).toBe(true);
  });

  it("treats null as the same as null", () => {
    expect(sameMentionRange(null, null)).toBe(true);
  });

  it("distinguishes a range from no range", () => {
    expect(sameMentionRange({ startLine: 1, endLine: 2 }, null)).toBe(false);
  });

  it("distinguishes different ranges", () => {
    expect(sameMentionRange({ startLine: 1, endLine: 2 }, { startLine: 1, endLine: 3 })).toBe(false);
  });
});

describe("diffWorkPath", () => {
  it("returns the repo-relative path for a working-tree model", () => {
    expect(diffWorkPath("/w/repo", { scheme: "tt-diff-work", path: "/w/repo/src/app.ts" })).toBe(
      "src/app.ts",
    );
  });

  it("rejects the base side of the diff", () => {
    expect(diffWorkPath("/w/repo", { scheme: "tt-diff-base", path: "/w/repo/src/app.ts" })).toBeNull();
  });

  it("rejects a file outside the folder", () => {
    expect(diffWorkPath("/w/repo", { scheme: "tt-diff-work", path: "/other/src/app.ts" })).toBeNull();
  });

  // "/w/repo-2/x" must not read as "repo" + "-2/x".
  it("rejects a sibling folder sharing a name prefix", () => {
    expect(
      diffWorkPath("/w/repo", { scheme: "tt-diff-work", path: "/w/repo-2/src/app.ts" }),
    ).toBeNull();
  });

  it("rejects a missing model uri", () => {
    expect(diffWorkPath("/w/repo", null)).toBeNull();
    expect(diffWorkPath("/w/repo", undefined)).toBeNull();
  });
});
