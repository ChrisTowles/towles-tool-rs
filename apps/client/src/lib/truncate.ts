/**
 * Truncate a string to `max` chars, appending an ellipsis if clipped. Ported
 * verbatim from slot-1 `packages/agentboard/src/runtime/text-utils.ts`.
 */
export function truncate(s: string, max: number): string {
  return s.length > max ? s.slice(0, max - 1) + "…" : s;
}

/** Collapse runs of whitespace to a single space and trim. */
export function collapseWS(s: string): string {
  return s.replace(/\s+/g, " ").trim();
}
