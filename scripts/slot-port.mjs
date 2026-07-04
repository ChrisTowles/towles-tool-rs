// Deterministic per-slot Vite dev-server port.
//
// Multiple worktree slots of this repo (towles-tool-rs-slot-0, -slot-1, …) run
// `tauri dev` at the same time. If every slot defaults to 1420 they collide, so
// we derive a stable base port from the slot's repo-root directory name: each
// slot prefers its own port and keeps it run-to-run, and `dev-port.mjs` scans
// upward from there for a free one as a safety net. An explicit TT_DEV_PORT /
// `.env.local` pin still wins over this.
import { basename } from "node:path";

export const PORT_MIN = 1420;
const PORT_SPAN = 200; // ports 1420–1619, partitioned across slots by name hash

/** Stable base port for the slot rooted at `repoRoot` (keyed on its dir name). */
export function slotBasePort(repoRoot) {
  const name = basename(repoRoot);
  let hash = 0;
  for (let i = 0; i < name.length; i++) hash = (hash * 31 + name.charCodeAt(i)) | 0;
  return PORT_MIN + (Math.abs(hash) % PORT_SPAN);
}
