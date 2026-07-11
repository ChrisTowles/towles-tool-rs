import { describe, expect, it } from "vitest";
import { linkAt, rowText } from "@/lib/term-links";
import type { Run } from "@/lib/term-protocol";

const COLS = 40;

/** A row whose text starts at column 0, padded to COLS by rowText. */
function row(text: string, x = 0): { runs: Run[] } {
  return { runs: [{ x, width: [...text].length, text }] };
}

describe("rowText", () => {
  it("places runs at their columns and pads to cols", () => {
    const t = rowText([{ x: 3, width: 2, text: "ab" }], 8);
    expect(t).toBe("   ab   ");
    expect(t.length).toBe(8);
  });

  it("gives wide characters two columns", () => {
    const t = rowText([{ x: 0, width: 4, text: "日本" }], 6);
    expect(t[0]).toBe("日");
    expect(t[2]).toBe("本");
    expect(t.length).toBe(6);
  });
});

describe("linkAt", () => {
  it("finds a URL under the probe cell", () => {
    const lines = [row("see https://example.com/x for docs")];
    const link = linkAt(lines, COLS, 10, 0);
    expect(link?.url).toBe("https://example.com/x");
    expect(link?.segments).toEqual([{ y: 0, start: 4, end: 24 }]);
  });

  it("misses cells outside the URL", () => {
    const lines = [row("see https://example.com/x for docs")];
    expect(linkAt(lines, COLS, 2, 0)).toBeNull();
    expect(linkAt(lines, COLS, 27, 0)).toBeNull();
  });

  it("handles multiple runs and non-zero run offsets", () => {
    const lines = [
      {
        runs: [
          { x: 0, width: 5, text: "bold " },
          { x: 5, width: 19, text: "https://example.com", fg: 0xff0000 },
        ],
      },
    ];
    expect(linkAt(lines, COLS, 10, 0)?.url).toBe("https://example.com");
  });

  it("trims sentence punctuation", () => {
    const lines = [row("go to https://example.com/a.")];
    expect(linkAt(lines, COLS, 10, 0)?.url).toBe("https://example.com/a");
  });

  it("keeps balanced parens but trims unbalanced ones", () => {
    const wiki = row("https://en.wikipedia.org/wiki/A_(b)");
    expect(linkAt([wiki], COLS, 5, 0)?.url).toBe("https://en.wikipedia.org/wiki/A_(b)");
    const wrapped = row("(see https://example.com/a)");
    expect(linkAt([wrapped], COLS, 10, 0)?.url).toBe("https://example.com/a");
  });

  it("ignores bare scheme with no host", () => {
    expect(linkAt([row("https:// is a prefix")], COLS, 3, 0)).toBeNull();
  });

  it("joins a URL wrapped across rows and reports one segment per row", () => {
    const url = `https://example.com/${"a".repeat(60)}`;
    const first = url.slice(0, COLS);
    const second = url.slice(COLS);
    const lines = [row(first), row(second)];

    for (const [x, y] of [
      [5, 0],
      [3, 1],
    ] as const) {
      const link = linkAt(lines, COLS, x, y);
      expect(link?.url).toBe(url);
      expect(link?.segments).toEqual([
        { y: 0, start: 0, end: COLS - 1 },
        { y: 1, start: 0, end: url.length - COLS - 1 },
      ]);
    }
  });

  it("misses cells after a wrapped URL's continuation", () => {
    // Row 0 fills all COLS columns; the URL continues on row 1 with a suffix.
    const part1 = `open https://example.com/${"a".repeat(COLS - 25)}`;
    expect(part1.length).toBe(COLS);
    const lines = [row(part1), row("bcd now")];
    const full = `https://example.com/${"a".repeat(COLS - 25)}bcd`;
    expect(linkAt(lines, COLS, 10, 0)?.url).toBe(full);
    expect(linkAt(lines, COLS, 1, 1)?.url).toBe(full);
    expect(linkAt(lines, COLS, 5, 1)).toBeNull();
  });

  it("does not join rows separated by trailing blank columns", () => {
    const lines = [row("https://example.com"), row("not-part-of-url")];
    expect(linkAt(lines, COLS, 5, 0)?.url).toBe("https://example.com");
  });

  it("handles out-of-range probes", () => {
    expect(linkAt([], COLS, 0, 0)).toBeNull();
    expect(linkAt([row("x")], COLS, 0, 5)).toBeNull();
  });
});
