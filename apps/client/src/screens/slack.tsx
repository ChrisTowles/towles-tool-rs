import { useEffect, useRef, useState } from "react";
import {
  Check,
  Copy,
  ExternalLink,
  ImageOff,
  KeyRound,
  MessageCircle,
  Paperclip,
  RefreshCw,
  Send,
  Settings,
  TriangleAlert,
} from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";
import { fmtClock } from "@/lib/data";
import {
  isAuthError,
  isFileScopeError,
  isScopeError,
  slackDmFile,
  slackDmSend,
  useSlackDm,
  type DmFile,
  type DmMessage,
} from "@/lib/slack";
import { MrkdwnText } from "@/components/mrkdwn-text";
import { openExternalUrl } from "@/lib/open-url";
import { isTauri } from "@/lib/tauri";
import { useWorkspace } from "@/lib/workspace";

/** api.slack.com app directory — where the app is created and tokens are issued. */
const SLACK_APPS_URL = "https://api.slack.com/apps";

/** The app manifest to paste into "Create app → From a manifest": display name,
 * the user scopes the watcher/chat/images need, and Socket Mode + message.im
 * events for live delivery. */
const APP_MANIFEST = `{
  "display_information": { "name": "Towles Tool DM Watch" },
  "oauth_config": {
    "scopes": {
      "user": [
        "im:history", "im:read", "im:write", "chat:write",
        "users:read", "users:read.email", "mpim:history", "mpim:read",
        "search:read", "reactions:write", "files:read"
      ]
    }
  },
  "settings": {
    "socket_mode_enabled": true,
    "event_subscriptions": { "user_events": ["message.im"] }
  }
}`;

/**
 * Messages — the in-app chat panel for the one watched Slack DM (the person the
 * `slack:dm` collector follows). Reads history via `slack_dm_history` and sends
 * replies via `slack_dm_send`; both hit the same collector settings the
 * background watcher uses but ignore its `enabled` flag, so the thread works
 * even with the watcher off. Unconfigured (no token/member id) is a friendly
 * setup hint, not an error. A send that fails on a missing `chat:write` scope
 * gets a specific "re-authorize your token" message rather than a raw code —
 * granting that scope is the user's job (a token re-auth in Slack).
 */
export function SlackScreen() {
  const { view, loading, error, refresh } = useSlackDm();
  const [draft, setDraft] = useState("");
  const [sending, setSending] = useState(false);
  const endRef = useRef<HTMLDivElement>(null);
  const composerRef = useRef<HTMLDivElement>(null);

  const messages = view?.messages ?? [];
  const lastTs = messages.at(-1)?.ts ?? 0;

  // Pin to the newest message as the thread grows (mount, refetch, send).
  useEffect(() => {
    endRef.current?.scrollIntoView({ block: "end" });
  }, [lastTs, view?.configured]);

  // The composer's textarea auto-grows with the draft (`field-sizing: content`),
  // which shrinks the messages viewport without touching `lastTs` — re-pin
  // whenever that height changes too, or the last message ends up short of the
  // actual bottom (or, on shrink back down, the scroll position jumps back up
  // as the browser clamps it to the new, larger scrollable range).
  useEffect(() => {
    const composer = composerRef.current;
    if (!composer) return;
    const observer = new ResizeObserver(() => {
      endRef.current?.scrollIntoView({ block: "end" });
    });
    observer.observe(composer);
    return () => observer.disconnect();
  }, [view?.configured]);

  async function send() {
    const text = draft.trim();
    if (!text || sending) return;
    setSending(true);
    try {
      await slackDmSend(text);
      setDraft("");
      refresh();
    } catch (e) {
      const message = String(e);
      if (isAuthError(message)) {
        toast.error(
          "Slack rejected the send: your token is no longer valid. Re-issue it in Settings → Slack.",
        );
      } else if (isScopeError(message)) {
        toast.error(
          "Slack rejected the send: your token can't post messages. Re-authorize it with the chat:write scope, then try again.",
        );
      } else {
        toast.error(message);
      }
    } finally {
      setSending(false);
    }
  }

  function onKeyDown(e: React.KeyboardEvent<HTMLTextAreaElement>) {
    // Enter sends; Shift+Enter (or a bare newline via IME) inserts a line break.
    if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      void send();
    }
  }

  const watchName = view?.watchName?.trim() || "Slack DM";

  return (
    <div className="flex h-full flex-col">
      <header className="flex shrink-0 items-center gap-2.5 border-b border-border bg-card px-4 py-2.5">
        <MessageCircle className="size-4 text-violet-500" />
        <span className="font-semibold text-foreground">{watchName}</span>
        {view?.configured && (
          <span className="font-mono text-[11px] text-muted-foreground/60">direct message</span>
        )}
        <div className="flex-1" />
        <Button
          variant="ghost"
          size="sm"
          className="h-7 gap-1.5 px-2 text-muted-foreground"
          onClick={refresh}
          disabled={loading}
        >
          <RefreshCw className={cn("size-3.5", loading && "animate-spin")} />
          Refresh
        </Button>
      </header>

      {view && !view.configured ? (
        <SetupGuide />
      ) : error && messages.length === 0 ? (
        <FetchError error={error} onRetry={refresh} />
      ) : (
        <>
          <ScrollArea className="min-h-0 flex-1">
            <div className="mx-auto flex w-full max-w-2xl flex-col gap-1.5 px-4 py-4">
              {messages.length === 0 && !loading && (
                <p className="py-8 text-center text-sm text-muted-foreground">
                  No messages yet. Say hello below.
                </p>
              )}
              {messages.map((m, i) => (
                <Bubble
                  key={`${m.ts}-${i}`}
                  message={m}
                  watchUserId={view?.watchUserId}
                  watchName={view?.watchName}
                />
              ))}
              <div ref={endRef} />
            </div>
          </ScrollArea>

          <div ref={composerRef} className="shrink-0 border-t border-border bg-card px-4 py-3">
            <div className="mx-auto flex w-full max-w-2xl items-end gap-2">
              <Textarea
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={onKeyDown}
                placeholder={`Message ${watchName}…`}
                rows={1}
                className="max-h-40 min-h-9 flex-1 resize-none"
              />
              <Button
                size="sm"
                className="h-9 gap-1.5 bg-violet-600 text-white hover:bg-violet-600/90"
                onClick={() => void send()}
                disabled={sending || draft.trim().length === 0}
              >
                <Send className="size-3.5" />
                Send
              </Button>
            </div>
          </div>
        </>
      )}
    </div>
  );
}

/** One chat bubble: mine (violet, right) vs. theirs (card, left). Text is
 * rendered from Slack mrkdwn (links, emphasis, mentions); attached files show
 * as inline image thumbnails or named chips. */
function Bubble({
  message,
  watchUserId,
  watchName,
}: {
  message: DmMessage;
  watchUserId?: string;
  watchName?: string;
}) {
  const mine = message.fromMe;
  const hasText = message.text.trim().length > 0;
  return (
    <div className={cn("flex flex-col gap-0.5", mine ? "items-end" : "items-start")}>
      <div
        className={cn(
          "max-w-[80%] rounded-lg border px-3 py-1.5 text-sm whitespace-pre-wrap",
          mine
            ? "border-violet-500/30 bg-violet-500/15 text-foreground"
            : "border-border bg-card text-foreground",
        )}
      >
        {hasText && (
          <MrkdwnText text={message.text} watchUserId={watchUserId} watchName={watchName} />
        )}
        {message.files.length > 0 && <Attachments files={message.files} hasText={hasText} />}
      </div>
      <span className="px-1 font-mono text-[10.5px] text-muted-foreground/60">
        {fmtClock(message.ts)}
      </span>
    </div>
  );
}

/** A message's attached files: images inline, everything else as a named chip. */
function Attachments({ files, hasText }: { files: DmFile[]; hasText: boolean }) {
  return (
    <div className={cn("flex flex-col gap-1.5", hasText && "mt-1.5")}>
      {files.map((file) =>
        file.isImage ? (
          <ImageAttachment key={file.id} file={file} />
        ) : (
          <FileChip key={file.id} file={file} />
        ),
      )}
    </div>
  );
}

/** Resolve the best URL for a file's bytes: a thumbnail if Slack made one, else
 * the full private URL. */
function fileSrcUrl(file: DmFile): string {
  return file.thumbUrl || file.urlPrivate;
}

/** Open a file in the OS browser (permalink signs the user in; falls back to
 * the private URL). */
function openFile(file: DmFile) {
  void openExternalUrl(file.permalink || file.urlPrivate);
}

/**
 * An inline image thumbnail. The private Slack URL can't be loaded straight
 * into `<img>` (it needs the bearer token), so in the Tauri shell we fetch the
 * bytes via `slack_dm_file` and render a `data:` URI; in browser dev the mock
 * URL is used directly. A missing `files:read` scope degrades to a subtle
 * placeholder rather than failing.
 */
function ImageAttachment({ file }: { file: DmFile }) {
  const [src, setSrc] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    setSrc(null);
    setError(null);
    void (async () => {
      try {
        const url = fileSrcUrl(file);
        if (!isTauri()) {
          if (alive) setSrc(url);
          return;
        }
        const { mimetype, dataBase64 } = await slackDmFile(url);
        if (alive) setSrc(`data:${mimetype};base64,${dataBase64}`);
      } catch (e) {
        if (alive) setError(String(e));
      }
    })();
    return () => {
      alive = false;
    };
  }, [file]);

  if (error !== null) {
    const note = isFileScopeError(error)
      ? "image unavailable until Slack re-auth (files:read)"
      : "couldn't load image";
    return (
      <button
        type="button"
        onClick={() => openFile(file)}
        className="flex items-center gap-2 rounded-md border border-dashed border-border bg-muted/40 px-3 py-2 text-left text-xs text-muted-foreground hover:bg-muted/60"
      >
        <ImageOff className="size-4 shrink-0" />
        <span className="truncate">
          {file.name} — {note}
        </span>
      </button>
    );
  }
  if (!src) {
    return <div className="h-40 w-56 max-w-full animate-pulse rounded-md bg-muted" />;
  }
  return (
    <button type="button" onClick={() => openFile(file)} className="block">
      <img
        src={src}
        alt={file.name}
        className="max-h-64 max-w-full rounded-md border border-border object-contain"
      />
    </button>
  );
}

/** A non-image attachment as a named chip that opens in the browser. */
function FileChip({ file }: { file: DmFile }) {
  return (
    <button
      type="button"
      onClick={() => openFile(file)}
      className="flex items-center gap-2 rounded-md border border-border bg-background px-3 py-2 text-left text-xs hover:bg-muted/50"
    >
      <Paperclip className="size-4 shrink-0 text-muted-foreground" />
      <span className="truncate font-medium text-foreground">{file.name}</span>
    </button>
  );
}

/** A copy-to-clipboard button that flips to a check for a moment after copying. */
function CopyButton({ text, label }: { text: string; label: string }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch {
      toast.error("Couldn't copy to clipboard.");
    }
  };
  return (
    <Button
      size="sm"
      variant="outline"
      className="h-7 gap-1.5 px-2 text-xs"
      onClick={() => void copy()}
    >
      {copied ? <Check className="size-3.5 text-emerald-500" /> : <Copy className="size-3.5" />}
      {copied ? "Copied" : label}
    </Button>
  );
}

/** The app-manifest code block with a copy button. */
function ManifestBlock() {
  return (
    <div className="overflow-hidden rounded-md border border-border bg-muted/40">
      <div className="flex items-center justify-between border-b border-border bg-muted/60 px-2.5 py-1.5">
        <span className="font-mono text-[11px] text-muted-foreground">app manifest</span>
        <CopyButton text={APP_MANIFEST} label="Copy manifest" />
      </div>
      <pre className="max-h-52 overflow-auto p-3 font-mono text-[11px] leading-relaxed text-foreground">
        {APP_MANIFEST}
      </pre>
    </div>
  );
}

/** A link that opens in the OS browser (never the webview). */
function ExternalLinkText({ url, children }: { url: string; children: React.ReactNode }) {
  return (
    <button
      type="button"
      onClick={() => void openExternalUrl(url)}
      className="inline-flex items-center gap-0.5 font-medium text-violet-600 underline underline-offset-2 hover:text-violet-500 dark:text-violet-300"
    >
      {children}
      <ExternalLink className="size-3" />
    </button>
  );
}

/** One numbered step in the setup guide. */
function Step({
  n,
  title,
  children,
}: {
  n: number;
  title: string;
  children: React.ReactNode;
}) {
  return (
    <li className="flex gap-3">
      <span className="flex size-6 shrink-0 items-center justify-center rounded-full bg-violet-500/15 text-xs font-semibold text-violet-600 dark:text-violet-300">
        {n}
      </span>
      <div className="flex-1 text-[13px] leading-relaxed">
        <div className="mb-1 font-medium text-foreground">{title}</div>
        {children}
      </div>
    </li>
  );
}

/** Full setup walkthrough shown when Slack isn't configured yet: create the app
 * from a manifest, install it, copy the tokens, and open Settings to finish. */
function SetupGuide() {
  const { openSettingsTab } = useWorkspace();
  return (
    <ScrollArea className="min-h-0 flex-1">
      <div className="mx-auto w-full max-w-xl px-6 py-8">
        <div className="mb-4 flex items-center gap-2.5">
          <MessageCircle className="size-5 text-violet-500" />
          <h2 className="text-base font-semibold text-foreground">Connect a Slack DM</h2>
        </div>
        <p className="mb-5 text-[13px] leading-relaxed text-muted-foreground">
          Watch one direct message (e.g. your partner) and reply without leaving the app. A
          one-time Slack setup:
        </p>
        <ol className="flex flex-col gap-4">
          <Step n={1} title="Create a Slack app from the manifest">
            <p className="text-muted-foreground">
              Go to <ExternalLinkText url={SLACK_APPS_URL}>api.slack.com/apps</ExternalLinkText> →{" "}
              <span className="font-medium text-foreground">Create New App</span> →{" "}
              <span className="font-medium text-foreground">From a manifest</span>, choose your
              workspace, and paste this:
            </p>
            <div className="mt-2">
              <ManifestBlock />
            </div>
          </Step>
          <Step n={2} title="Install it to your workspace">
            <p className="text-muted-foreground">
              On the app's <span className="font-medium text-foreground">Install App</span> page,
              click Install and then <span className="font-medium text-foreground">Allow</span>.
            </p>
          </Step>
          <Step n={3} title="Copy the User OAuth Token">
            <p className="text-muted-foreground">
              From <span className="font-medium text-foreground">OAuth &amp; Permissions</span>,
              copy the <span className="font-mono">xoxp-…</span> User OAuth Token.
            </p>
          </Step>
          <Step n={4} title="Generate an app-level token (for live updates)">
            <p className="text-muted-foreground">
              Recommended: under{" "}
              <span className="font-medium text-foreground">
                Basic Information → App-Level Tokens
              </span>
              , generate a token with the <span className="font-mono">connections:write</span> scope
              (<span className="font-mono">xapp-…</span>). Without it, messages arrive on a 60-second
              poll instead of instantly.
            </p>
          </Step>
          <Step n={5} title="Paste both tokens and pick who to watch">
            <p className="text-muted-foreground">
              In Settings → Slack, paste the tokens and choose the person to watch.
            </p>
            <div className="mt-2">
              <Button
                size="sm"
                className="gap-1.5"
                onClick={() => openSettingsTab({ tab: "collectors", filter: "slack" })}
              >
                <Settings className="size-3.5" /> Open Slack settings
              </Button>
            </div>
          </Step>
        </ol>
      </div>
    </ScrollArea>
  );
}

/** Compact re-auth walkthrough shown when a configured token is rejected
 * (invalid_auth) — re-issue it and paste the fresh one. */
function ReauthNotice({ onRetry }: { onRetry: () => void }) {
  const { openSettingsTab } = useWorkspace();
  return (
    <div className="flex min-h-0 flex-1 items-center justify-center p-6">
      <div className="max-w-md rounded-lg border border-border bg-card p-6">
        <div className="mb-2 flex items-center gap-2">
          <KeyRound className="size-5 text-amber-500" />
          <h2 className="text-sm font-semibold text-foreground">Your Slack token expired</h2>
        </div>
        <p className="mb-3 text-[13px] leading-relaxed text-muted-foreground">
          Slack rejected the token (<span className="font-mono">invalid_auth</span>). Re-issue it and
          paste the fresh one:
        </p>
        <ol className="mb-4 flex flex-col gap-1.5 text-[13px] text-muted-foreground">
          <li>
            1. Open your app at{" "}
            <ExternalLinkText url={SLACK_APPS_URL}>api.slack.com/apps</ExternalLinkText> → OAuth
            &amp; Permissions.
          </li>
          <li>
            2. Reinstall if prompted, then copy the new <span className="font-mono">xoxp-…</span>{" "}
            token.
          </li>
          <li>3. Paste it in Settings → Slack and Save.</li>
        </ol>
        <div className="flex gap-2">
          <Button
            size="sm"
            className="gap-1.5"
            onClick={() => openSettingsTab({ tab: "collectors", filter: "slack" })}
          >
            <Settings className="size-3.5" /> Open Slack settings
          </Button>
          <Button size="sm" variant="outline" onClick={onRetry}>
            Retry
          </Button>
        </div>
      </div>
    </div>
  );
}

/** Shown when history fetch failed outright (e.g. a bad/expired token). A dead
 * token routes to the re-auth walkthrough; a missing scope to a scope hint. */
function FetchError({ error, onRetry }: { error: string; onRetry: () => void }) {
  if (isAuthError(error)) return <ReauthNotice onRetry={onRetry} />;
  const scope = isScopeError(error);
  return (
    <div className="flex min-h-0 flex-1 items-center justify-center p-6">
      <div className="max-w-sm rounded-lg border border-border bg-card p-6 text-center">
        {scope ? (
          <KeyRound className="mx-auto mb-3 size-6 text-amber-500" />
        ) : (
          <TriangleAlert className="mx-auto mb-3 size-6 text-red-500" />
        )}
        <h2 className="mb-1 text-sm font-semibold text-foreground">
          {scope ? "Token needs more access" : "Couldn't load the conversation"}
        </h2>
        <p className="mb-4 text-[13px] leading-relaxed break-words text-muted-foreground">
          {scope
            ? "Re-authorize your Slack token with the chat:write scope, then retry."
            : error}
        </p>
        <Button size="sm" variant="outline" onClick={onRetry}>
          Retry
        </Button>
      </div>
    </div>
  );
}
