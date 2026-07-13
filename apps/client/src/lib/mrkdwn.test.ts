import { describe, expect, it } from "vitest";
import { mentionLabel, parseMrkdwn, unescapeEntities, type MrkdwnNode } from "@/lib/mrkdwn";

/** Flatten a node tree to its visible text, for terse assertions. */
function flat(nodes: MrkdwnNode[]): string {
  return nodes
    .map((n) => {
      switch (n.type) {
        case "text":
        case "code":
        case "pre":
        case "channel":
        case "broadcast":
          return "value" in n ? n.value : n.label;
        case "link":
          return n.label;
        case "user":
          return `@${n.id}`;
        case "strong":
        case "em":
        case "del":
          return flat(n.children);
      }
    })
    .join("");
}

describe("parseMrkdwn — plain and emphasis", () => {
  it("returns a single text node for plain text", () => {
    expect(parseMrkdwn("just some text")).toEqual([{ type: "text", value: "just some text" }]);
  });

  it("parses bold, italic, and strike", () => {
    expect(parseMrkdwn("*bold*")).toEqual([
      { type: "strong", children: [{ type: "text", value: "bold" }] },
    ]);
    expect(parseMrkdwn("_italic_")).toEqual([
      { type: "em", children: [{ type: "text", value: "italic" }] },
    ]);
    expect(parseMrkdwn("~gone~")).toEqual([
      { type: "del", children: [{ type: "text", value: "gone" }] },
    ]);
  });

  it("nests emphasis", () => {
    expect(parseMrkdwn("*_both_*")).toEqual([
      {
        type: "strong",
        children: [{ type: "em", children: [{ type: "text", value: "both" }] }],
      },
    ]);
  });

  it("keeps surrounding text around a span", () => {
    const nodes = parseMrkdwn("hey *now* ok");
    expect(flat(nodes)).toBe("hey now ok");
    expect(nodes[1]).toEqual({ type: "strong", children: [{ type: "text", value: "now" }] });
  });

  it("does not italicize snake_case or treat bare * as emphasis", () => {
    expect(parseMrkdwn("call some_long_name now")).toEqual([
      { type: "text", value: "call some_long_name now" },
    ]);
    expect(parseMrkdwn("2 * 3 * 4")).toEqual([{ type: "text", value: "2 * 3 * 4" }]);
  });

  it("leaves an unmatched marker as literal text", () => {
    expect(parseMrkdwn("a *lonely star")).toEqual([{ type: "text", value: "a *lonely star" }]);
  });
});

describe("parseMrkdwn — code", () => {
  it("parses inline code without formatting its contents", () => {
    expect(parseMrkdwn("run `npm *test*` now")).toEqual([
      { type: "text", value: "run " },
      { type: "code", value: "npm *test*" },
      { type: "text", value: " now" },
    ]);
  });

  it("parses a fenced block", () => {
    expect(parseMrkdwn("```\nline1\nline2\n```")).toEqual([
      { type: "pre", value: "\nline1\nline2\n" },
    ]);
  });
});

describe("parseMrkdwn — links and mentions", () => {
  it("parses a labeled link", () => {
    expect(parseMrkdwn("see <https://ex.com/a|the docs>")).toEqual([
      { type: "text", value: "see " },
      { type: "link", url: "https://ex.com/a", label: "the docs" },
    ]);
  });

  it("uses the url as the label for a bare link", () => {
    expect(parseMrkdwn("<https://ex.com>")).toEqual([
      { type: "link", url: "https://ex.com", label: "https://ex.com" },
    ]);
  });

  it("parses user, channel, and broadcast mentions", () => {
    expect(parseMrkdwn("<@U123>")).toEqual([{ type: "user", id: "U123", label: null }]);
    expect(parseMrkdwn("<@U123|Chris>")).toEqual([{ type: "user", id: "U123", label: "Chris" }]);
    expect(parseMrkdwn("<#C1|general>")).toEqual([{ type: "channel", label: "general" }]);
    expect(parseMrkdwn("<!here>")).toEqual([{ type: "broadcast", label: "@here" }]);
    expect(parseMrkdwn("<!subteam^S1|@team>")).toEqual([{ type: "broadcast", label: "@team" }]);
  });
});

describe("unescapeEntities", () => {
  it("reverses Slack's three escapes and avoids double-decoding", () => {
    expect(unescapeEntities("a &lt;b&gt; &amp; c")).toBe("a <b> & c");
    expect(unescapeEntities("&amp;lt;")).toBe("&lt;");
  });

  it("unescapes entities inside parsed text", () => {
    expect(parseMrkdwn("tom &amp; jerry")).toEqual([{ type: "text", value: "tom & jerry" }]);
  });
});

describe("mentionLabel", () => {
  const opts = { watchUserId: "U_WIFE", watchName: "Sarah" };

  it("prefers an explicit label", () => {
    expect(mentionLabel("U123", "Chris", opts)).toBe("@Chris");
    expect(mentionLabel("U123", "@Chris", opts)).toBe("@Chris");
  });

  it("resolves the watched user's id to their name", () => {
    expect(mentionLabel("U_WIFE", null, opts)).toBe("@Sarah");
  });

  it("falls back to @user for anyone else", () => {
    expect(mentionLabel("U_ME", null, opts)).toBe("@user");
    expect(mentionLabel("U_WIFE", null, {})).toBe("@user");
  });
});
