import { useEffect, useRef } from "react";
import { Check, MessageCircleHeart } from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { dmsNeedingAttention, fmtAge, storeDmDismiss, useStoreSnapshot } from "@/lib/data";
import { useNow } from "@/lib/now";
import { openExternalUrl } from "@/lib/open-url";
import { isTauri } from "@/lib/tauri";
import { useWorkspace } from "@/lib/workspace";

/** Unanswered this long → the banner turns amber-urgent. */
const WARN_MS = 5 * 60_000;
/** Unanswered this long → the banner pulses and the OS taskbar flashes. */
const ALARM_MS = 10 * 60_000;

/**
 * Full-width strip under the day bar for a watched Slack DM (the `slack:dm`
 * collector — e.g. a message from your wife). Quiet rose on arrival; the age
 * turns amber after 5 unanswered minutes; after 10 the whole strip pulses and
 * the window requests OS-level attention (taskbar flash). Clears itself when
 * you reply in Slack (the collector sees your message as the newest) or when
 * "Handled" is pressed.
 */
export function DmBanner() {
  const { snapshot } = useStoreSnapshot();
  const { openTab } = useWorkspace();
  const now = useNow();
  // The message ts we already flashed the taskbar for — flash once per message.
  const flashedTs = useRef(0);

  const dm = dmsNeedingAttention(snapshot)[0];
  const age = dm ? now - dm.ts : 0;
  const alarm = !!dm && age >= ALARM_MS;
  const warn = !!dm && age >= WARN_MS;

  useEffect(() => {
    if (!alarm || !dm || flashedTs.current === dm.ts || !isTauri()) return;
    flashedTs.current = dm.ts;
    void (async () => {
      try {
        const { getCurrentWindow, UserAttentionType } = await import("@tauri-apps/api/window");
        await getCurrentWindow().requestUserAttention(UserAttentionType.Critical);
      } catch {
        // Best-effort: the in-app pulse still carries the escalation.
      }
    })();
  }, [alarm, dm]);

  if (!dm) return null;

  return (
    <div
      className={cn(
        "flex shrink-0 items-center gap-2.5 border-b border-l-2 border-l-rose-500 bg-rose-500/5 px-3 py-1.5 text-xs",
        alarm && "animate-pulse bg-rose-500/15",
      )}
    >
      <MessageCircleHeart className="size-4 shrink-0 text-rose-500" />
      <span className="shrink-0 font-medium text-foreground">{dm.fromName}</span>
      <span className="min-w-0 truncate text-muted-foreground">{dm.text}</span>
      <span
        className={cn(
          "shrink-0 font-mono text-[11px] text-muted-foreground/60",
          warn && "font-medium text-amber-600 dark:text-amber-500",
          alarm && "text-rose-500 dark:text-rose-400",
        )}
      >
        {fmtAge(dm.ts, now)}
      </span>

      <div className="flex-1" />

      <Button
        variant="outline"
        size="sm"
        className="h-6 border-rose-500/40 px-2 text-xs text-rose-600 hover:text-rose-600 dark:text-rose-400 dark:hover:text-rose-400"
        onClick={() => openTab("slack")}
      >
        Reply here
      </Button>
      {dm.url && (
        <Button
          variant="ghost"
          size="sm"
          className="h-6 px-2 text-xs text-muted-foreground"
          onClick={() => void openExternalUrl(dm.url!)}
        >
          Open in Slack
        </Button>
      )}
      <Button
        variant="ghost"
        size="sm"
        className="h-6 px-2 text-xs text-muted-foreground"
        onClick={() => void storeDmDismiss(dm.channel, dm.ts)}
      >
        <Check className="size-3.5" />
        Handled
      </Button>
    </div>
  );
}
