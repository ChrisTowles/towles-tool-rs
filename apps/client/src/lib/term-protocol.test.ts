import { describe, expect, it } from "vitest";
import { urlAt, type Run } from "./term-protocol";

/** One unstyled run per row starting at column 0. */
function row(text: string): Run[] {
  return text ? [{ x: 0, width: [...text].length, text }] : [];
}

describe("urlAt", () => {
  const COLS = 40;

  it("finds a URL under the clicked cell", () => {
    const rows = [row("see https://example.com/docs for more")];
    expect(urlAt(rows, 4, 0, COLS)).toBe("https://example.com/docs");
    expect(urlAt(rows, 27, 0, COLS)).toBe("https://example.com/docs");
  });

  it("returns null outside the URL and on plain text", () => {
    const rows = [row("see https://example.com/docs for more")];
    expect(urlAt(rows, 0, 0, COLS)).toBeNull();
    expect(urlAt(rows, 30, 0, COLS)).toBeNull();
    expect(urlAt([row("no links here")], 3, 0, COLS)).toBeNull();
  });

  it("handles multiple runs and non-zero run offsets", () => {
    const rows = [
      [
        { x: 0, width: 5, text: "bold " },
        { x: 5, width: 19, text: "https://example.com", fg: 0xff0000 },
      ],
    ];
    expect(urlAt(rows, 10, 0, COLS)).toBe("https://example.com");
  });

  it("joins hard-wrapped rows so a wrapped URL matches whole", () => {
    // Row 0 fills all COLS columns and the URL continues on row 1.
    const part1 = `open https://example.com/${"a".repeat(COLS - 25)}`;
    expect(part1.length).toBe(COLS);
    const rows = [row(part1), row("bcd now")];
    const full = `https://example.com/${"a".repeat(COLS - 25)}bcd`;
    expect(urlAt(rows, 10, 0, COLS)).toBe(full);
    // Clicking the continuation on the second row finds it too.
    expect(urlAt(rows, 1, 1, COLS)).toBe(full);
    expect(urlAt(rows, 5, 1, COLS)).toBeNull();
  });

  it("does not join rows that are not full-width", () => {
    const rows = [row("https://example.com"), row("/not-part")];
    expect(urlAt(rows, 5, 0, COLS)).toBe("https://example.com");
  });

  it("trims trailing punctuation and unbalanced parens", () => {
    expect(urlAt([row("(https://example.com/a)")], 5, 0, COLS)).toBe("https://example.com/a");
    expect(urlAt([row("see https://example.com.")], 10, 0, COLS)).toBe("https://example.com");
    // Balanced parens inside the URL are kept (wikipedia-style).
    expect(urlAt([row("https://en.wikipedia.org/wiki/Rust_(lang)")], 5, 0, 60)).toBe(
      "https://en.wikipedia.org/wiki/Rust_(lang)",
    );
  });

  it("returns null out of bounds", () => {
    expect(urlAt([], 0, 0, COLS)).toBeNull();
    expect(urlAt([row("https://example.com")], 0, 5, COLS)).toBeNull();
  });
});
