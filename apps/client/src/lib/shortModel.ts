/**
 * Shorten a model id for display. Ported verbatim from slot-1
 * `packages/agentboard/src/tui/components/short-model.ts`.
 *
 * Strips a leading `claude-` and a trailing `[1m]` (case-insensitive):
 * `claude-opus-4-6` → `opus-4-6`.
 */
export function shortModel(model: string): string {
  if (!model) return "";
  return model.replace(/^claude-/, "").replace(/\[1m\]$/i, "");
}
