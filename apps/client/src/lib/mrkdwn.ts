/**
 * A small, pure parser for Slack's `mrkdwn` message format — the flavor
 * `conversations.history` returns for DM text. It is deliberately not full
 * Markdown: Slack uses single `*bold*`, `_italic_`, `~strike~`, backtick
 * `code`, triple-backtick blocks, angle-wrapped links (`<url|label>`) and
 * mentions (`<@U123>`, `<#C1|name>`, `<!here>`), and HTML-escapes only
 * `& < >` as `&amp; &lt; &gt;`.
 *
 * Parsing produces a flat list of {@link MrkdwnNode}s (emphasis nests) that the
 * {@link file:./../components/mrkdwn-text.tsx} renderer turns into React. Keeping
 * the parse pure (no React, no Tauri) makes it unit-testable and keeps link
 * opening — which must go through the OS browser, never the webview — a concern
 * of the renderer.
 */

export type MrkdwnNode =
  | { type: "text"; value: string }
  | { type: "strong"; children: MrkdwnNode[] }
  | { type: "em"; children: MrkdwnNode[] }
  | { type: "del"; children: MrkdwnNode[] }
  | { type: "code"; value: string }
  | { type: "pre"; value: string }
  | { type: "link"; url: string; label: string }
  | { type: "user"; id: string; label: string | null }
  | { type: "channel"; label: string }
  | { type: "broadcast"; label: string };

// Atoms whose contents are literal (no emphasis parsing inside): fenced code,
// inline code, and angle-wrapped entities. Tried left-to-right at each position,
// so a triple-backtick block wins over a stray single backtick.
const ATOM = /```([\s\S]*?)```|`([^`\n]+)`|<([^>\n]+)>/g;

/** Parse a Slack mrkdwn string into renderable nodes. */
export function parseMrkdwn(src: string): MrkdwnNode[] {
  const nodes: MrkdwnNode[] = [];
  let last = 0;
  ATOM.lastIndex = 0;
  let m: RegExpExecArray | null;
  while ((m = ATOM.exec(src)) !== null) {
    if (m.index > last) pushInline(nodes, src.slice(last, m.index));
    if (m[1] !== undefined) {
      nodes.push({ type: "pre", value: unescapeEntities(m[1]) });
    } else if (m[2] !== undefined) {
      nodes.push({ type: "code", value: unescapeEntities(m[2]) });
    } else {
      nodes.push(parseAngle(m[3]));
    }
    last = ATOM.lastIndex;
  }
  if (last < src.length) pushInline(nodes, src.slice(last));
  return nodes;
}

/** An angle-wrapped Slack entity: link, user/channel/broadcast mention. */
function parseAngle(inner: string): MrkdwnNode {
  const bar = inner.indexOf("|");
  const head = bar === -1 ? inner : inner.slice(0, bar);
  const label = bar === -1 ? "" : inner.slice(bar + 1);
  if (head.startsWith("@")) {
    return { type: "user", id: head.slice(1), label: label ? unescapeEntities(label) : null };
  }
  if (head.startsWith("#")) {
    return { type: "channel", label: unescapeEntities(label || head.slice(1)) };
  }
  if (head.startsWith("!")) {
    return { type: "broadcast", label: broadcastLabel(head.slice(1), label) };
  }
  return { type: "link", url: head, label: unescapeEntities(label || head) };
}

/** `<!here>`→`@here`; `<!subteam^S1|@team>`→`@team` (Slack ships the label). */
function broadcastLabel(name: string, label: string): string {
  if (label) return unescapeEntities(label);
  return `@${name}`;
}

/** Parse a run of non-atom text (emphasis + entities) and append it. */
function pushInline(nodes: MrkdwnNode[], text: string): void {
  for (const node of parseInline(text)) nodes.push(node);
}

const MARKERS: Record<string, "strong" | "em" | "del"> = { "*": "strong", _: "em", "~": "del" };

function parseInline(text: string): MrkdwnNode[] {
  const nodes: MrkdwnNode[] = [];
  let i = 0;
  while (i < text.length) {
    const span = findEmphasis(text, i);
    if (!span) {
      pushText(nodes, text.slice(i));
      break;
    }
    if (span.start > i) pushText(nodes, text.slice(i, span.start));
    const inner = text.slice(span.start + 1, span.end - 1);
    nodes.push({ type: span.node, children: parseInline(inner) });
    i = span.end;
  }
  return nodes;
}

/** The earliest valid emphasis span at or after `from`, or null. */
function findEmphasis(
  text: string,
  from: number,
): { start: number; end: number; node: "strong" | "em" | "del" } | null {
  for (let i = from; i < text.length; i++) {
    const marker = text[i];
    const node = MARKERS[marker];
    if (!node || !opensHere(text, i, marker)) continue;
    for (let j = i + 1; j < text.length; j++) {
      if (text[j] === marker && closesHere(text, j, marker)) {
        return { start: i, end: j + 1, node };
      }
    }
  }
  return null;
}

/**
 * Whether a marker at `i` can open an emphasis span. Requires a non-word char
 * (or start) before it — so `snake_case` and `2*3` don't become emphasis — and
 * a non-space, non-duplicate marker right after.
 */
function opensHere(text: string, i: number, marker: string): boolean {
  const prev = text[i - 1];
  const next = text[i + 1];
  if (next === undefined || /\s/.test(next) || next === marker) return false;
  return prev === undefined || !/[0-9A-Za-z]/.test(prev);
}

/** Whether a marker at `j` can close a span: non-space before, boundary after. */
function closesHere(text: string, j: number, marker: string): boolean {
  const prev = text[j - 1];
  const next = text[j + 1];
  if (/\s/.test(prev) || prev === marker) return false;
  return next === undefined || !/[0-9A-Za-z]/.test(next);
}

function pushText(nodes: MrkdwnNode[], raw: string): void {
  if (!raw) return;
  const value = unescapeEntities(raw);
  const last = nodes[nodes.length - 1];
  if (last && last.type === "text") last.value += value;
  else nodes.push({ type: "text", value });
}

/** Reverse Slack's only three HTML escapes. `&amp;` last, so `&amp;lt;`
 * round-trips to the literal `&lt;` rather than being double-decoded. */
export function unescapeEntities(s: string): string {
  return s.replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&amp;/g, "&");
}

/**
 * Resolve a `<@U…>` mention to a display label. An explicit label in the
 * payload wins; otherwise the watched user's id resolves to their name; anything
 * else (my own id, an unknown third party) falls back to a generic `@user`
 * since a DM only ever has the two of us.
 */
export function mentionLabel(
  id: string,
  label: string | null,
  opts: { watchUserId?: string; watchName?: string },
): string {
  const explicit = label?.trim();
  if (explicit) return `@${explicit.replace(/^@/, "")}`;
  const watchName = opts.watchName?.trim();
  if (opts.watchUserId && id === opts.watchUserId && watchName) return `@${watchName}`;
  return "@user";
}
