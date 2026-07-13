import { useCallback, useEffect, useState } from "react";
import { invokeOrThrow, isTauri } from "./tauri";

/**
 * Client-side view of the watched Slack DM conversation, served on demand by
 * the Tauri `slack_dm_history` / `slack_dm_send` commands (the `slack` bridge
 * module in tt-app). Mirrors the serialized `SlackDmView` / `DmMessage`
 * (camelCase). Timestamps are epoch milliseconds. Unlike the store snapshot
 * this is a pull, not a subscription — the panel refetches after a send and
 * whenever the store re-emits (a background `slack:dm` tick landed a reply).
 */

/** One message of the conversation. `fromMe` is true for anything I sent. */
export type DmMessage = {
  text: string;
  ts: number;
  fromMe: boolean;
};

/**
 * The chat panel's view. `configured` is false when the slack collector has no
 * token/member id yet — the panel shows setup guidance instead of a thread.
 */
export type SlackDmView = {
  configured: boolean;
  watchName: string;
  messages: DmMessage[];
};

/** Slack error codes that mean "your token can't post" — the send failed
 * because the OAuth token is missing the `chat:write` scope (the DM *watcher*
 * only needs read scopes, so an existing token predates two-way chat). */
const SCOPE_ERROR_CODES = ["missing_scope", "not_allowed_token_type"] as const;

/** True when a `slack_dm_send` rejection is really a missing-scope problem, so
 * the UI can tell the user to re-authorize rather than showing a raw code. */
export function isScopeError(message: string): boolean {
  return SCOPE_ERROR_CODES.some((code) => message.includes(code));
}

/** A representative thread for plain-Vite browser dev (no Tauri host), so the
 * panel is visually workable without real Slack credentials. */
function mockView(now: number = Date.now()): SlackDmView {
  const MIN = 60_000;
  return {
    configured: true,
    watchName: "Danielle",
    messages: [
      { text: "hey, are you still on for dinner tonight?", ts: now - 42 * MIN, fromMe: false },
      { text: "yes! leaving in about an hour", ts: now - 40 * MIN, fromMe: true },
      { text: "perfect, i'll meet you there", ts: now - 38 * MIN, fromMe: false },
    ],
  };
}

/**
 * Load the watched DM thread and keep it fresh. Refetches on mount, on demand
 * (`refresh`), and whenever the store snapshot re-emits (a background tick may
 * have landed a new incoming message). Outside Tauri it holds {@link mockView}.
 *
 * `view` is null only during the very first load; `error` holds a fetch failure
 * (e.g. a bad token) so the panel can distinguish "not configured" from "broke".
 */
export function useSlackDm(): {
  view: SlackDmView | null;
  loading: boolean;
  error: string | null;
  refresh: () => void;
} {
  const [view, setView] = useState<SlackDmView | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (!isTauri()) {
      setView(mockView());
      setLoading(false);
      return;
    }
    try {
      const next = await invokeOrThrow<SlackDmView>("slack_dm_history");
      setView(next);
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  const refresh = useCallback(() => {
    void load();
  }, [load]);

  useEffect(() => {
    void load();
    if (!isTauri()) return;

    let disposed = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        const { listen } = await import("@tauri-apps/api/event");
        const sub = await listen("store://snapshot", () => void load());
        if (disposed) sub();
        else unlisten = sub;
      } catch {
        // No Tauri event bus — the mount fetch + manual refresh still work.
      }
    })();
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [load]);

  return { view, loading, error, refresh };
}

/**
 * Send `text` to the watched DM as me. Resolves on success; rejects with the
 * Slack error string on failure (see {@link isScopeError}). The command
 * refreshes the store snapshot on its side, which nudges {@link useSlackDm} to
 * refetch the thread.
 */
export async function slackDmSend(text: string): Promise<void> {
  await invokeOrThrow<void>("slack_dm_send", { text });
}
