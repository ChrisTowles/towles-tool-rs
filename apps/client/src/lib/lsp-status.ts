/**
 * What the LSP bridge (`lib/lsp.ts`) is doing, as a store any component can
 * read. Split out from the bridge itself so that reading the status costs
 * nothing: `lsp.ts` pulls in monaco and `monaco-languageclient`, and the Files
 * pane only wants a badge. Keeping the two apart lets the bridge stay a lazy
 * chunk behind `setMonacoWorkspace`'s dynamic import.
 *
 * Same store-beside-the-service shape as `lib/monaco-dialog-store.ts`.
 */

import { useSyncExternalStore } from "react";

/**
 * - `off` — not a Rust checkout (or not running under Tauri): nothing to do.
 * - `starting` — rust-analyzer is spawning / the client is handshaking.
 * - `ready` — the language client started; hovers and completions are live.
 * - `failed` — spawn or handshake failed (usually no rust-analyzer on PATH).
 */
export type LspState = "off" | "starting" | "ready" | "failed";
export type LspStatus = { state: LspState; dir: string | null; detail?: string };

let status: LspStatus = { state: "off", dir: null };
const listeners = new Set<() => void>();

export function setLspStatus(next: LspStatus): void {
  status = next;
  for (const fn of [...listeners]) fn();
}

/** Snapshot for `useSyncExternalStore` — stable reference until it changes. */
const lspStatus = (): LspStatus => status;

const subscribeLspStatus = (fn: () => void): (() => void) => {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
};

/** One shared object for every folder the server isn't following, so the hook
 * returns a stable reference without a per-dir cache to grow unbounded. `dir`
 * is null because it carries no information in this state — the folder that
 * asked already knows which one it is. */
const OFF: LspStatus = { state: "off", dir: null };

/** Bridge status for one folder — `off` unless this folder is the one the
 * single shared server currently follows. */
export function useLspStatus(dir: string | undefined): LspStatus {
  const s = useSyncExternalStore(subscribeLspStatus, lspStatus);
  return dir != null && s.dir === dir ? s : OFF;
}
