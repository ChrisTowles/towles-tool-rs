import { invoke } from "@/lib/tauri";
import { shellQuote } from "@/lib/agentboard";

/** One dev-server config from a checkout's Claude Desktop
 * `.claude/launch.json` (`configurations[]`: name / runtimeExecutable /
 * runtimeArgs / port), with live status stamped by the backend — see
 * `launch_configs` in `crates-tauri/tt-app/src/launch.rs`. */
export type LaunchConfigStatus = {
  name: string;
  runtimeExecutable: string;
  runtimeArgs: string[];
  /** Port the server listens on once up — absent configs can't be probed. */
  port?: number | null;
  /** Something accepts TCP connections on `port` right now (always false
   * without a port — `port` itself tells "unknown" from "stopped"). */
  portListening: boolean;
  /** The app session this config was launched into, while that PTY lives —
   * the "focus that pane instead of launching again" target. */
  sessionId?: string | null;
};

/** A folder's launch.json configs + live status. `Err` carries a message
 * worth showing (e.g. a malformed launch.json); a folder without the file
 * resolves `Ok([])`. */
export const launchConfigs = (dir: string) =>
  invoke<LaunchConfigStatus[]>("launch_configs", { dir });

/** Record "config `name` now runs in session `sessionId`" right after typing
 * its command into that pane — the backend prunes the mapping when the PTY
 * dies, and logs the launch gesture to the event log. */
export const launchRegister = (
  dir: string,
  name: string,
  sessionId: string,
  port: number | null,
  command: string,
) => invoke<void>("launch_register", { dir, name, sessionId, port, command });

/** Tokens that read the same quoted or bare — quoting these anyway would
 * render `pnpm dev` as `'pnpm' 'dev'` in the user's own terminal. */
const SAFE_TOKEN = /^[\w@%+=:,./-]+$/;

function quoteToken(token: string): string {
  return SAFE_TOKEN.test(token) ? token : shellQuote(token);
}

/** The command line a config types into its PTY (no trailing `\r` — the
 * caller appends it): `runtimeExecutable runtimeArgs…`, each token quoted
 * only when the shell would otherwise mangle it. */
export function launchCommand(
  cfg: Pick<LaunchConfigStatus, "runtimeExecutable" | "runtimeArgs">,
): string {
  return [cfg.runtimeExecutable, ...cfg.runtimeArgs].map(quoteToken).join(" ");
}

/** Where a listening config serves. Dev servers in launch.json are local by
 * definition — Claude Desktop previews them on localhost the same way. */
export function devServerUrl(port: number): string {
  return `http://localhost:${port}/`;
}

/** What the row's one action slot offers. Independent of the status dot
 * (which tracks `portListening` alone): a pane we launched stays focusable
 * even after the server inside it was Ctrl-C'd. */
export type LaunchAction = "focus" | "external" | "launch";

export function launchAction(cfg: LaunchConfigStatus): LaunchAction {
  // Our own pane wins: even with the port also listening, the pane is the
  // thing the user can actually look at and act in.
  if (cfg.sessionId) return "focus";
  // Port held by something we didn't launch — offering a second launch
  // would just crash into EADDRINUSE.
  if (cfg.portListening) return "external";
  return "launch";
}
