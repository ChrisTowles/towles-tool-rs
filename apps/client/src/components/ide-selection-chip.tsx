import { cn } from "@/lib/utils";
import { ClaudeBadge } from "@/components/agentboard-bits";
import { formatLineRange, type MentionRange } from "@/lib/ide-selection";

/**
 * The floating "these lines are going to Claude" chip, shown over the file
 * viewer and the diff editor whenever text is selected.
 *
 * Selecting text already *streams* to the connected session as ambient
 * context; the chip's job is to say so, and to offer the explicit gesture —
 * `@ send`, which drops an `@file#L12-40` reference into the session's prompt.
 * Restored from the diff pane's original gutter UI, minus its dismiss button:
 * Monaco owns Esc and a click collapses the selection, so this self-dismisses.
 */
export function IdeSelectionChip({
  range,
  connected,
  onSend,
  label,
}: {
  range: MentionRange;
  connected: boolean;
  onSend: () => void;
  /** Filename, for the multi-diff — a bare "L12–40" is ambiguous when many
   * files share one scroll. */
  label?: string;
}) {
  return (
    <div className="absolute right-3 bottom-3 z-10 flex max-w-[calc(100%-1.5rem)] items-center gap-2 rounded-md border border-border bg-card px-2 py-1 whitespace-nowrap shadow-md">
      <span className="font-mono text-xs text-violet-500">✦</span>
      {label && (
        <span className="min-w-0 truncate font-mono text-[11px] text-muted-foreground" title={label}>
          {label}
        </span>
      )}
      <span className="font-mono text-[11px] text-foreground tabular-nums">
        {formatLineRange(range)}
      </span>
      <span className="truncate text-[11px] text-muted-foreground">
        {connected ? "live to claude" : "no claude connected"}
      </span>
      <button
        type="button"
        title={
          connected
            ? "Insert an @file#range reference into the Claude session's prompt"
            : "Run `claude` in this folder's terminal to connect it"
        }
        disabled={!connected}
        onClick={onSend}
        className={cn(
          "shrink-0 rounded-sm px-1.5 py-0.5 text-[11px] font-medium",
          connected
            ? "text-violet-500 hover:bg-accent"
            : "cursor-not-allowed text-muted-foreground/50",
        )}
      >
        @ send
      </button>
    </div>
  );
}

/** Shown in the same corner when a session is connected but nothing is
 * selected yet — otherwise the gesture is invisible until you find it. */
function IdeConnectedHint() {
  return (
    <ClaudeBadge
      title="Selecting lines shares them with the Claude session in this folder"
      className="pointer-events-none absolute right-3 bottom-3 z-10 max-w-[calc(100%-1.5rem)] gap-1.5 px-2 py-1"
    >
      <span className="font-mono text-xs">✦</span>
      <span className="truncate text-[11px]">claude is connected — select lines to share them</span>
    </ClaudeBadge>
  );
}

/**
 * The editor's whole Claude-selection surface: the chip once something is
 * selected, the discoverability hint while it isn't, and nothing at all until
 * the editor has loaded. Both editor components render exactly this, so the
 * empty-state policy lives in one place.
 */
export function IdeSelectionOverlay({
  selection,
  label,
  connected,
  loading,
  onSend,
}: {
  selection: MentionRange | null;
  label?: string;
  connected: boolean;
  loading: boolean;
  onSend: () => void;
}) {
  if (loading) return null;
  if (selection) {
    return (
      <IdeSelectionChip
        range={selection}
        label={label}
        connected={connected}
        onSend={onSend}
      />
    );
  }
  return connected ? <IdeConnectedHint /> : null;
}
