// Ported from slot-1 `packages/agentboard/src/tui/components/family-color.ts`.
//
// Port decision (UI-SPEC §4): keep the hash fallback, DROP the hardcoded
// KNOWN_FAMILIES map (it was pinned to the author's personal repos). Every
// family now hashes into the fallback hues.

import type { ThemePalette } from "./themes";

const FALLBACK_HUES: (keyof ThemePalette)[] = ["mauve", "blue", "green", "yellow", "red"];

const SLOT_SUFFIX = /-(?:primary|slot-\d+)$/;

/** Strip a `-primary` / `-slot-N` suffix to group slot clones of one repo. */
export function familyOf(sessionName: string): string {
  const stripped = sessionName.replace(SLOT_SUFFIX, "");
  return stripped || sessionName;
}

function hash(s: string): number {
  let h = 0;
  for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) | 0;
  return Math.abs(h);
}

/** Deterministic accent hue for a session's family (hash fallback only). */
export function familyColor(sessionName: string, palette: ThemePalette): string {
  const family = familyOf(sessionName);
  const key = FALLBACK_HUES[hash(family) % FALLBACK_HUES.length];
  return palette[key];
}
