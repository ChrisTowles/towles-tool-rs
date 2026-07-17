import { describe, expect, it } from "vitest";
import { linkAt, linkLabel, rowLinks, rowText } from "@/lib/term-links";
import type { Run } from "@/lib/term-protocol";

const COLS = 40;

/** A row whose text starts at column 0, padded to COLS by rowText. */
function row(text: string, x = 0): { runs: Run[]; wrapped?: boolean } {
  return { runs: [{ x, width: [...text].length, text }] };
}

/** A row the engine marked as soft-wrapping into the next row. */
function wrappedRow(text: string): { runs: Run[]; wrapped?: boolean } {
  return { ...row(text), wrapped: true };
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

describe("rowLinks", () => {
  it("marks every column a linked run spans, leaving the rest undefined", () => {
    const runs: Run[] = [
      { x: 0, width: 4, text: "see " },
      { x: 4, width: 4, text: "here", link: "https://example.com/pr/1" },
      { x: 8, width: 6, text: " today" },
    ];
    const links = rowLinks(runs, 14);
    expect(links.slice(0, 4)).toEqual([undefined, undefined, undefined, undefined]);
    expect(links.slice(4, 8)).toEqual(Array(4).fill("https://example.com/pr/1"));
    expect(links.slice(8)).toEqual(Array(6).fill(undefined));
  });
});

/** Narrow to a URL link (asserting the kind), for terse `.url` access. */
function urlAt(...args: Parameters<typeof linkAt>) {
  const link = linkAt(...args);
  if (link && link.kind !== "url") throw new Error(`expected url link, got ${link.kind}`);
  return link;
}

/** Narrow to a path link, for terse `.path`/`.line` access. */
function pathAt(...args: Parameters<typeof linkAt>) {
  const link = linkAt(...args);
  if (link && link.kind !== "path") throw new Error(`expected path link, got ${link.kind}`);
  return link;
}

describe("linkAt (urls)", () => {
  it("finds a URL under the probe cell", () => {
    const lines = [row("see https://example.com/x for docs")];
    const link = urlAt(lines, COLS, 10, 0);
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
    expect(urlAt(lines, COLS, 10, 0)?.url).toBe("https://example.com");
  });

  it("trims sentence punctuation", () => {
    const lines = [row("go to https://example.com/a.")];
    expect(urlAt(lines, COLS, 10, 0)?.url).toBe("https://example.com/a");
  });

  it("keeps balanced parens but trims unbalanced ones", () => {
    const wiki = row("https://en.wikipedia.org/wiki/A_(b)");
    expect(urlAt([wiki], COLS, 5, 0)?.url).toBe("https://en.wikipedia.org/wiki/A_(b)");
    const wrapped = row("(see https://example.com/a)");
    expect(urlAt([wrapped], COLS, 10, 0)?.url).toBe("https://example.com/a");
  });

  it("ignores bare scheme with no host", () => {
    expect(linkAt([row("https:// is a prefix")], COLS, 3, 0)).toBeNull();
  });

  it("joins a URL wrapped across rows and reports one segment per row", () => {
    const url = `https://example.com/${"a".repeat(60)}`;
    const first = url.slice(0, COLS);
    const second = url.slice(COLS);
    const lines = [wrappedRow(first), row(second)];

    for (const [x, y] of [
      [5, 0],
      [3, 1],
    ] as const) {
      const link = urlAt(lines, COLS, x, y);
      expect(link?.url).toBe(url);
      expect(link?.segments).toEqual([
        { y: 0, start: 0, end: COLS - 1 },
        { y: 1, start: 0, end: url.length - COLS - 1 },
      ]);
    }
  });

  it("misses cells after a wrapped URL's continuation", () => {
    // Row 0 soft-wraps; the URL continues on row 1 with a suffix.
    const part1 = `open https://example.com/${"a".repeat(COLS - 25)}`;
    expect(part1.length).toBe(COLS);
    const lines = [wrappedRow(part1), row("bcd now")];
    const full = `https://example.com/${"a".repeat(COLS - 25)}bcd`;
    expect(urlAt(lines, COLS, 10, 0)?.url).toBe(full);
    expect(urlAt(lines, COLS, 1, 1)?.url).toBe(full);
    expect(linkAt(lines, COLS, 5, 1)).toBeNull();
  });

  it("does not join rows the engine did not mark wrapped", () => {
    const lines = [row("https://example.com"), row("not-part-of-url")];
    expect(urlAt(lines, COLS, 5, 0)?.url).toBe("https://example.com");
  });

  it("does not join a full-width row ended by a real newline", () => {
    // Text fills every column but the engine says it was not soft-wrapped —
    // exactly the case the old last-column heuristic mis-joined.
    const full = `x`.repeat(COLS - "https://example.com/a".length) + "https://example.com/a";
    expect(full.length).toBe(COLS);
    const lines = [row(full), row("unrelated.rs:1 text")];
    expect(urlAt(lines, COLS, COLS - 3, 0)?.url).toBe("https://example.com/a");
  });

  it("handles out-of-range probes", () => {
    expect(linkAt([], COLS, 0, 0)).toBeNull();
    expect(linkAt([row("x")], COLS, 0, 5)).toBeNull();
  });
});

describe("linkAt (osc8 hyperlinks)", () => {
  it("trusts a hyperlink whose visible text doesn't look like a URL", () => {
    const lines = [
      {
        runs: [
          { x: 0, width: 4, text: "see " },
          { x: 4, width: 4, text: "here", link: "https://example.com/pr/1" },
          { x: 8, width: 6, text: " today" },
        ],
      },
    ];
    const link = urlAt(lines, COLS, 5, 0);
    expect(link?.url).toBe("https://example.com/pr/1");
    expect(link?.segments).toEqual([{ y: 0, start: 4, end: 7 }]);
  });

  it("misses cells outside the linked run", () => {
    const lines = [
      {
        runs: [
          { x: 0, width: 4, text: "see " },
          { x: 4, width: 4, text: "here", link: "https://example.com/pr/1" },
          { x: 8, width: 6, text: " today" },
        ],
      },
    ];
    expect(linkAt(lines, COLS, 1, 0)).toBeNull();
    expect(linkAt(lines, COLS, 10, 0)).toBeNull();
  });

  it("takes priority over regex detection when the label is itself a URL", () => {
    // The link text renders as a URL, but the OSC 8 target differs — the
    // real target must win, not the regex-matched display text.
    const lines = [
      {
        runs: [{ x: 0, width: 19, text: "https://example.com", link: "https://real.example/x" }],
      },
    ];
    expect(urlAt(lines, COLS, 3, 0)?.url).toBe("https://real.example/x");
  });

  it("joins a hyperlink spanning two rows into one segment set", () => {
    const lines = [
      {
        runs: [{ x: 0, width: COLS, text: "x".repeat(COLS), link: "https://example.com/long" }],
        wrapped: true,
      },
      { runs: [{ x: 0, width: 5, text: "yyyyy", link: "https://example.com/long" }] },
    ];
    const link = urlAt(lines, COLS, 5, 0);
    expect(link?.url).toBe("https://example.com/long");
    expect(link?.segments).toEqual([
      { y: 0, start: 0, end: COLS - 1 },
      { y: 1, start: 0, end: 4 },
    ]);
  });
});

describe("linkAt (paths)", () => {
  it("finds a repo-relative path with a :line suffix", () => {
    const lines = [row("edit crates/tt-vt/src/search.rs:42 now")];
    const link = pathAt(lines, COLS, 12, 0);
    expect(link?.kind).toBe("path");
    expect(link?.path).toBe("crates/tt-vt/src/search.rs");
    expect(link?.line).toBe(42);
    expect(link?.segments).toEqual([{ y: 0, start: 5, end: 33 }]);
  });

  it("keeps the :line suffix inside the underlined segment", () => {
    const lines = [row("crates/tt-vt/src/search.rs:42")];
    // The cell over the digits still resolves to the same link.
    expect(pathAt(lines, COLS, 28, 0)?.path).toBe("crates/tt-vt/src/search.rs");
  });

  it("supports a line:col suffix", () => {
    const lines = [row("at src/main.rs:12:5 here")];
    expect(pathAt(lines, COLS, 5, 0)?.line).toBe(12);
  });

  it("matches an absolute path", () => {
    const lines = [row("wrote /home/ctowles/app.tsx ok")];
    const link = pathAt(lines, COLS, 10, 0);
    expect(link?.path).toBe("/home/ctowles/app.tsx");
    expect(link?.line).toBeNull();
  });

  it("matches ./ and ../ relative prefixes", () => {
    expect(pathAt([row("see ./src/main.rs done")], COLS, 6, 0)?.path).toBe("./src/main.rs");
    expect(pathAt([row("see ../lib/x.ts done")], COLS, 6, 0)?.path).toBe("../lib/x.ts");
  });

  it("accepts a bare filename only when it carries a :line", () => {
    expect(pathAt([row("open search.rs:42 here")], COLS, 7, 0)?.path).toBe("search.rs");
    // No slash and no :line — treated as prose, not a path.
    expect(linkAt([row("open search.rs here")], COLS, 7, 0)).toBeNull();
  });

  it("ignores dotted prose that is not a path", () => {
    expect(linkAt([row("visit example.com for info")], COLS, 8, 0)).toBeNull();
    expect(linkAt([row("bumped to 1.2.3 today")], COLS, 11, 0)).toBeNull();
  });

  it("trims trailing sentence punctuation and brackets", () => {
    expect(pathAt([row("in crates/foo.rs:9.")], COLS, 5, 0)?.line).toBe(9);
    expect(pathAt([row("in crates/foo.rs:9.")], COLS, 5, 0)?.path).toBe("crates/foo.rs");
    expect(pathAt([row("(see src/a.rs)")], COLS, 6, 0)?.path).toBe("src/a.rs");
  });

  it("does not treat a URL's tail as a path", () => {
    const lines = [row("go https://example.com/docs/page.html now")];
    const link = linkAt(lines, COLS, 25, 0);
    expect(link?.kind).toBe("url");
    expect(link?.kind === "url" && link.url).toBe("https://example.com/docs/page.html");
  });

  it("joins a path wrapped across rows", () => {
    const path = `crates/${"deep/".repeat(9)}mod.rs`;
    expect(path.length).toBeGreaterThan(COLS);
    const lines = [wrappedRow(path.slice(0, COLS)), row(path.slice(COLS))];
    const link = pathAt(lines, COLS, 5, 0);
    expect(link?.path).toBe(path);
    expect(link?.segments.length).toBe(2);
  });
});

describe("linkLabel", () => {
  it("renders a URL as-is and a path with its line", () => {
    expect(linkLabel({ kind: "url", url: "https://x.dev", segments: [] })).toBe("https://x.dev");
    expect(linkLabel({ kind: "path", path: "src/a.rs", line: 7, segments: [] })).toBe(
      "src/a.rs:7",
    );
    expect(linkLabel({ kind: "path", path: "src/a.rs", line: null, segments: [] })).toBe(
      "src/a.rs",
    );
  });
});
