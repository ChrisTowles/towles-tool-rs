import { SCREENS, type ScreenId } from "@/lib/screens";

/** localStorage keys for the persisted tab state. Mirrors SIDEBAR_COLLAPSED_KEY
 * in workspace.tsx. */
export const ACTIVE_TAB_KEY = "tt-active-tab";
export const VISITED_TABS_KEY = "tt-visited-tabs";

/** The screen shown on a cold start (no valid persisted state). */
export const COLD_START_TAB: ScreenId = "cockpit";

function isScreenId(value: unknown): value is ScreenId {
  return typeof value === "string" && value in SCREENS;
}

/** Restore the persisted tab state from raw localStorage strings.
 *
 * Pure and DOM-independent (callers pass the raw strings) so it can be unit
 * tested and reasoned about in isolation. Any missing, malformed, or
 * stale-screen-id input degrades to the cold-start default rather than
 * throwing — a removed screen id can never break boot.
 *
 * `visited` always includes `activeTab`, and both only ever contain screen ids
 * that still exist in the registry, so a closed tab (dropped from the stored
 * visited list) never resurrects on reload. */
export function loadWorkspaceTabs(
  rawActive: string | null,
  rawVisited: string | null,
): { visited: ScreenId[]; activeTab: ScreenId } {
  const visited = parseVisited(rawVisited);
  const activeTab = isScreenId(rawActive) ? rawActive : COLD_START_TAB;
  if (!visited.includes(activeTab)) visited.push(activeTab);
  return { visited, activeTab };
}

function parseVisited(rawVisited: string | null): ScreenId[] {
  if (rawVisited === null) return [COLD_START_TAB];
  let parsed: unknown;
  try {
    parsed = JSON.parse(rawVisited);
  } catch {
    return [COLD_START_TAB];
  }
  if (!Array.isArray(parsed)) return [COLD_START_TAB];
  // Keep only ids that still exist, de-duplicated in stored order.
  const seen = new Set<ScreenId>();
  for (const id of parsed) {
    if (isScreenId(id)) seen.add(id);
  }
  const visited = [...seen];
  return visited.length > 0 ? visited : [COLD_START_TAB];
}
