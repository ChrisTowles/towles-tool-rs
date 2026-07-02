/**
 * Format an elapsed duration. Ported verbatim from slot-1
 * `packages/agentboard/src/tui/components/elapsed.ts`.
 *
 * Clamps negatives to 0; floors (never rounds). `<60s→"{s}s"`,
 * `<60m→"{m}m"`, else `"{h}h"`.
 */
export function formatElapsed(ms: number): string {
  if (ms < 0) ms = 0;
  const seconds = Math.floor(ms / 1000);
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  return `${hours}h`;
}
