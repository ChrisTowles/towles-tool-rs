import { useEffect, useRef, useState } from "react";
import { KeyRound, MessageCircle, RefreshCw, Send, TriangleAlert } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";
import { fmtClock } from "@/lib/data";
import { isScopeError, slackDmSend, useSlackDm, type DmMessage } from "@/lib/slack";
import { openSettings } from "@/lib/open-settings";

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

  const messages = view?.messages ?? [];
  const lastTs = messages.at(-1)?.ts ?? 0;

  // Pin to the newest message as the thread grows (mount, refetch, send).
  useEffect(() => {
    endRef.current?.scrollIntoView({ block: "end" });
  }, [lastTs, view?.configured]);

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
      if (isScopeError(message)) {
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
        <SetupHint />
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
                <Bubble key={`${m.ts}-${i}`} message={m} />
              ))}
              <div ref={endRef} />
            </div>
          </ScrollArea>

          <div className="shrink-0 border-t border-border bg-card px-4 py-3">
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

/** One chat bubble: mine (violet, right) vs. theirs (card, left). */
function Bubble({ message }: { message: DmMessage }) {
  const mine = message.fromMe;
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
        {message.text}
      </div>
      <span className="px-1 font-mono text-[10.5px] text-muted-foreground/60">
        {fmtClock(message.ts)}
      </span>
    </div>
  );
}

/** Shown when the slack collector has no token/member id yet. */
function SetupHint() {
  return (
    <div className="flex min-h-0 flex-1 items-center justify-center p-6">
      <div className="max-w-sm rounded-lg border border-border bg-card p-6 text-center">
        <MessageCircle className="mx-auto mb-3 size-6 text-muted-foreground/60" />
        <h2 className="mb-1 text-sm font-semibold text-foreground">Connect a Slack DM</h2>
        <p className="mb-4 text-[13px] leading-relaxed text-muted-foreground">
          Add a Slack user token (<span className="font-mono">xoxp-…</span>) and the member ID of
          the person you want to message, then this panel shows the conversation and lets you
          reply. Sending also needs the token's <span className="font-mono">chat:write</span> scope.
        </p>
        <Button size="sm" onClick={() => void openSettings()}>
          Open Settings
        </Button>
      </div>
    </div>
  );
}

/** Shown when history fetch failed outright (e.g. a bad/expired token). */
function FetchError({ error, onRetry }: { error: string; onRetry: () => void }) {
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
