// Ported verbatim from slot-1 `packages/agentboard/src/tui/constants.ts`.

import type { MetadataTone } from "../types";
import type { ThemePalette } from "./themes";

/** Map a metadata tone to a palette color (default → overlay0). */
export function toneColor(tone: MetadataTone | undefined, palette: ThemePalette): string {
  switch (tone) {
    case "success":
      return palette.green;
    case "error":
      return palette.red;
    case "warn":
      return palette.yellow;
    case "info":
      return palette.blue;
    default:
      return palette.overlay0;
  }
}
