import { SCREENS, type ScreenId } from "@/lib/screens";

/** localStorage keys for the persisted tab state. Mirrors SIDEBAR_COLLAPSED_KEY
 * in workspace.tsx. */
export const ACTIVE_TAB_KEY = "tt-active-tab";
export const OPEN_TABS_KEY = "tt-open-tabs";

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
 * `openTabs` always includes `activeTab`, and both only ever contain screen
 * ids that still exist in the registry, so a closed tab (dropped from the
 * stored open-tabs list) never resurrects on reload. */
export function loadWorkspaceTabs(
  rawActive: string | null,
  rawOpenTabs: string | null,
): { openTabs: ScreenId[]; activeTab: ScreenId } {
  const openTabs = parseOpenTabs(rawOpenTabs);
  const activeTab = isScreenId(rawActive) ? rawActive : COLD_START_TAB;
  if (!openTabs.includes(activeTab)) openTabs.push(activeTab);
  return { openTabs, activeTab };
}

function parseOpenTabs(rawOpenTabs: string | null): ScreenId[] {
  if (rawOpenTabs === null) return [COLD_START_TAB];
  let parsed: unknown;
  try {
    parsed = JSON.parse(rawOpenTabs);
  } catch {
    return [COLD_START_TAB];
  }
  if (!Array.isArray(parsed)) return [COLD_START_TAB];
  // Keep only ids that still exist, de-duplicated in stored order.
  const seen = new Set<ScreenId>();
  for (const id of parsed) {
    if (isScreenId(id)) seen.add(id);
  }
  const openTabs = [...seen];
  return openTabs.length > 0 ? openTabs : [COLD_START_TAB];
}
