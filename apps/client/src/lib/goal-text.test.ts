import { describe, expect, it } from "vitest";
import { applyMention, highlightSegments, matchIssues, mentionQueryAt } from "./goal-text";

describe("highlightSegments", () => {
  it("covers the input exactly, so the overlay can't drift from the textarea", () => {
    const text = "fix #12 per https://example.com/x and also #7";
    expect(
      highlightSegments(text)
        .map((s) => s.text)
        .join(""),
    ).toBe(text);
  });

  it("classifies urls and issue refs", () => {
    const segs = highlightSegments("see #42 at https://a.test/b");
    expect(segs).toEqual([
      { text: "see ", kind: "plain" },
      { text: "#42", kind: "ref" },
      { text: " at ", kind: "plain" },
      { text: "https://a.test/b", kind: "url" },
    ]);
  });

  it("leaves plain text alone", () => {
    expect(highlightSegments("just a goal")).toEqual([{ text: "just a goal", kind: "plain" }]);
  });

  it("does not treat a bare # or #word as a ref", () => {
    expect(highlightSegments("#").every((s) => s.kind === "plain")).toBe(true);
    expect(highlightSegments("#nope").every((s) => s.kind === "plain")).toBe(true);
  });

  it("is empty for empty input", () => {
    expect(highlightSegments("")).toEqual([]);
  });
});

describe("mentionQueryAt", () => {
  it("finds a mention at the caret", () => {
    expect(mentionQueryAt("fix #12", 7)).toEqual({ start: 4, query: "12" });
  });

  it("finds a bare # (lists everything)", () => {
    expect(mentionQueryAt("fix #", 5)).toEqual({ start: 4, query: "" });
  });

  it("matches a title-ish query too", () => {
    expect(mentionQueryAt("see #dark", 9)).toEqual({ start: 4, query: "dark" });
  });

  it("is null when the # does not start a word", () => {
    // The realistic case: a pasted URL fragment must not pop the issue list.
    expect(mentionQueryAt("https://x/pull/4#issuecomment", 28)).toBeNull();
    expect(mentionQueryAt("foo#1", 5)).toBeNull();
  });

  it("is null with no # before the caret", () => {
    expect(mentionQueryAt("plain goal", 10)).toBeNull();
  });

  it("only reads up to the caret, not the rest of the line", () => {
    expect(mentionQueryAt("fix #12 later", 6)).toEqual({ start: 4, query: "1" });
  });
});

describe("applyMention", () => {
  it("replaces the token and puts the caret after the inserted space", () => {
    const r = applyMention("fix #da", 4, 7, 42);
    expect(r.text).toBe("fix #42 ");
    expect(r.caret).toBe(8);
  });

  it("keeps text after the caret", () => {
    const r = applyMention("fix # now", 4, 5, 7);
    expect(r.text).toBe("fix #7  now");
    expect(r.caret).toBe(7);
  });
});

describe("matchIssues", () => {
  const issues = [
    { number: 12, title: "Dark mode" },
    { number: 123, title: "Light mode" },
    { number: 7, title: "Fix dark toggle" },
  ];

  it("lists everything for an empty query", () => {
    expect(matchIssues(issues, "")).toHaveLength(3);
  });

  it("matches by number prefix when the query is digits", () => {
    expect(matchIssues(issues, "12").map((i) => i.number)).toEqual([12, 123]);
  });

  it("matches by title, case-insensitively, otherwise", () => {
    expect(matchIssues(issues, "DARK").map((i) => i.number)).toEqual([12, 7]);
  });

  it("is empty when nothing matches", () => {
    expect(matchIssues(issues, "zzz")).toEqual([]);
  });
});
