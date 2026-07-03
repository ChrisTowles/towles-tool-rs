// Ported verbatim from slot-1
// `packages/agentboard/src/tui/components/status-visuals.ts`.

import type { AgentStatus } from "../types";
import type { ThemePalette } from "./themes";
import { SPINNERS } from "./constants";

/** Icon for a live (non-terminal) status. Returns "" for statuses without a glyph. */
export function liveStatusIcon(status: AgentStatus, spinIdx: number): string {
  if (status === "busy") return SPINNERS[spinIdx % SPINNERS.length];
  if (status === "waiting") return "?";
  return "";
}

/** Accent color for a terminal agent whose final status the user hasn't seen. */
export function unseenTerminalColor(status: AgentStatus, palette: ThemePalette): string {
  if (status === "error") return palette.red;
  if (status === "interrupted") return palette.peach;
  return palette.teal;
}
