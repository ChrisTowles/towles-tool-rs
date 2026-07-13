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

/** A file attached to a DM message (mirrors Rust `DmFile`). The private URLs
 * need the token's bearer header, so images are fetched through
 * {@link slackDmFile} rather than loaded straight into an `<img>`. */
export type DmFile = {
  id: string;
  name: string;
  mimetype: string;
  urlPrivate: string;
  thumbUrl: string;
  permalink: string;
  isImage: boolean;
};

/** One message of the conversation. `fromMe` is true for anything I sent. */
export type DmMessage = {
  text: string;
  ts: number;
  fromMe: boolean;
  files: DmFile[];
};

/**
 * The chat panel's view. `configured` is false when the slack collector has no
 * token/member id yet — the panel shows setup guidance instead of a thread.
 * `watchUserId` lets the renderer resolve `<@id>` mentions to the watched name.
 */
export type SlackDmView = {
  configured: boolean;
  watchName: string;
  watchUserId: string;
  messages: DmMessage[];
};

/** Bytes of a Slack file, base64-encoded (mirrors Rust `SlackFileData`). */
export type SlackFileData = {
  mimetype: string;
  dataBase64: string;
};

/** Error marker the backend uses when the token lacks `files:read`, so the
 * panel can show a "re-auth for images" placeholder instead of a hard error. */
export function isFileScopeError(message: string): boolean {
  return message.includes("files:read");
}

/** Slack error codes that mean "your token can't post" — the send failed
 * because the OAuth token is missing the `chat:write` scope (the DM *watcher*
 * only needs read scopes, so an existing token predates two-way chat). */
const SCOPE_ERROR_CODES = ["missing_scope", "not_allowed_token_type"] as const;

/** True when a `slack_dm_send` rejection is really a missing-scope problem, so
 * the UI can tell the user to re-authorize rather than showing a raw code. */
export function isScopeError(message: string): boolean {
  return SCOPE_ERROR_CODES.some((code) => message.includes(code));
}

/** Slack error codes that mean the token itself is bad — revoked, expired, or
 * for a deactivated account — so the fix is to re-issue and paste a fresh one. */
const AUTH_ERROR_CODES = ["invalid_auth", "token_revoked", "account_inactive", "not_authed"] as const;

/** True when a fetch/send failure is a dead-token problem (as opposed to a
 * missing scope), so the UI can prompt a re-auth walkthrough. */
export function isAuthError(message: string): boolean {
  return AUTH_ERROR_CODES.some((code) => message.includes(code));
}

/** A tiny inline image so the mock thread exercises the attachment layout in
 * browser dev (a real thumb/url_private needs the Tauri file-fetch command). */
const MOCK_IMAGE =
  "data:image/svg+xml;utf8," +
  encodeURIComponent(
    `<svg xmlns="http://www.w3.org/2000/svg" width="240" height="150"><rect width="240" height="150" fill="#a78bfa"/><text x="120" y="80" font-family="sans-serif" font-size="18" fill="white" text-anchor="middle">photo</text></svg>`,
  );

/** A representative thread for plain-Vite browser dev (no Tauri host), so the
 * panel is visually workable without real Slack credentials. */
function mockView(now: number = Date.now()): SlackDmView {
  const MIN = 60_000;
  return {
    configured: true,
    watchName: "Danielle",
    watchUserId: "U_DANIELLE",
    messages: [
      {
        text: "hey, are you still on for *dinner* tonight? see <https://ex.com/menu|the menu>",
        ts: now - 42 * MIN,
        fromMe: false,
        files: [],
      },
      { text: "yes! leaving in about an hour", ts: now - 40 * MIN, fromMe: true, files: [] },
      {
        text: "found this place",
        ts: now - 38 * MIN,
        fromMe: false,
        files: [
          {
            id: "F_MOCK",
            name: "storefront.png",
            mimetype: "image/png",
            urlPrivate: MOCK_IMAGE,
            thumbUrl: MOCK_IMAGE,
            permalink: "https://ex.com/photo",
            isImage: true,
          },
        ],
      },
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

/**
 * Fetch a Slack file's bytes (base64) with the token's bearer header — the
 * webview can't load `url_private` directly. Pass a {@link DmFile}'s `thumbUrl`
 * (images) or `urlPrivate`. Rejects with a scope error (see
 * {@link isFileScopeError}) when the token lacks `files:read`.
 */
export async function slackDmFile(url: string): Promise<SlackFileData> {
  return invokeOrThrow<SlackFileData>("slack_dm_file", { url });
}

/** A workspace member for the Settings watch-user picker (mirrors `SlackUser`). */
export type SlackUser = {
  id: string;
  name: string;
};

/**
 * List human workspace members (`users.list`, `users:read` scope) for the
 * watch-user picker. Returns [] when the token is blank so the picker can
 * degrade to a plain text input. Only meaningful in the Tauri shell.
 */
export async function slackListUsers(): Promise<SlackUser[]> {
  if (!isTauri()) return [];
  return invokeOrThrow<SlackUser[]>("slack_list_users");
}
