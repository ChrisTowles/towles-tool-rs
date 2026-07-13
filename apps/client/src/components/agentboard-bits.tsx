import { useState, type ComponentProps, type ReactNode } from "react";
import { ChevronDown, GitCompare, GitPullRequest } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import {
  abInvoke,
  ctxPct,
  isCacheExpiring,
  isCold,
  needsCompact,
  statusColor,
  type AgentStatus,
  type FolderData,
  type FolderMetadata,
  type MetadataTone,
  type SessionData,
} from "@/lib/agentboard";
import type { PrItem } from "@/lib/data";
import { openExternalUrl } from "@/lib/open-url";
import { cn } from "@/lib/utils";

/**
 * Shared atoms for the Agentboard UI — one visual language for the rail rows,
 * folder headers, pane chrome, and the working-context band, so each surface
 * composes the same pieces instead of hand-rolling its own variants.
 */

/** A small square icon action that *reads as a button* (bordered, hover fill)
 * — shadcn outline button at icon-xs, mono glyph or lucide icon inside.
 * `title` renders as a real (Radix) tooltip: instant, styled, and — unlike a
 * native `title` attribute or CSS `:hover` reveal — reliable in the Tauri
 * WebKitGTK webview. It doubles as the `aria-label`, since the glyph alone
 * says nothing. Clicks never bubble into the row/header the button sits on. */
export function IconBtn({
  title,
  onClick,
  className,
  children,
  ...props
}: {
  title: string;
  onClick: () => void;
  className?: string;
  children: ReactNode;
} & Omit<ComponentProps<"button">, "onClick" | "title" | "children">) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="outline"
          size="icon-xs"
          aria-label={title}
          onClick={(e) => {
            e.stopPropagation();
            onClick();
          }}
          className={cn("font-mono text-xs text-muted-foreground", className)}
          {...props}
        >
          {children}
        </Button>
      </TooltipTrigger>
      <TooltipContent side="bottom">{title}</TooltipContent>
    </Tooltip>
  );
}

/** ✦ for an agent session, ❯ for a plain shell. */
export function Glyph({ agent }: { agent: boolean }) {
  return (
    <span
      className={cn(
        "w-4 shrink-0 text-center font-mono text-xs",
        agent ? "text-violet-500" : "text-muted-foreground/60",
      )}
    >
      {agent ? "✦" : "❯"}
    </span>
  );
}

/** Status dot mirroring `statusColor`; pulses while busy. A session with no
 * live PTY shows a hollow ring — the record exists but nothing is running.
 * "Look at this" is the row's amber border (`sessionCatchesEye`), not the
 * dot — a resting board stays still. */
export function Dot({ session }: { session: SessionData }) {
  if (!session.live) {
    return (
      <span
        title="not started"
        className="size-2 shrink-0 rounded-full border-[1.5px] border-muted-foreground/50 bg-transparent"
      />
    );
  }
  const st = session.agentState?.status;
  return (
    <span
      title={st ? `agent ${st}` : "shell running, no agent"}
      className={cn(
        "size-2 shrink-0 rounded-full",
        st ? statusColor(st) : "bg-muted-foreground/40",
        st === "busy" && "animate-pulse",
      )}
    />
  );
}

/** A status-colored micro-dot + count, e.g. "●3", for agent rollups (the rail
 * chip and the nav sidebar). Color always derives from `statusColor` so the
 * buckets can never drift from the `Dot` atom. */
export function DotCount({ status, n }: { status: AgentStatus; n: number }) {
  return (
    <span className="flex items-center gap-1 text-muted-foreground">
      <span className={cn("size-1.5 rounded-full", statusColor(status))} />
      {n}
    </span>
  );
}

export function Chevron({ collapsed }: { collapsed: boolean }) {
  return (
    <ChevronDown
      className={cn(
        "size-3.5 shrink-0 text-muted-foreground transition-transform",
        collapsed && "-rotate-90",
      )}
    />
  );
}

export function NeedsBadge({ n, className }: { n: number; className?: string }) {
  return (
    <span
      className={cn(
        "shrink-0 rounded-md border border-amber-500/50 bg-amber-500/10 px-1.5 font-mono text-[10.5px] text-amber-500",
        className,
      )}
    >
      {n} ⚑
    </span>
  );
}

/** Marks a tracked checkout whose directory no longer exists on disk — a
 * "ghost". Deliberately grayscale (dashed, dimmed): a missing dir is a dead
 * state, not the live-attention amber the needs-you math owns, so it reads as
 * "gone/inert" rather than "look at me". Grayscale tokens carry light + dark. */
export function GhostBadge() {
  return (
    <span
      className="shrink-0 rounded-md border border-dashed border-muted-foreground/40 px-1 font-mono text-[10px] text-muted-foreground/70"
      title="This checkout's directory is gone (moved or deleted). Untrack it, or restore the directory to bring it back."
    >
      ⚠ missing
    </span>
  );
}

/** Marks a folder as a git worktree checkout (linked to another checkout's
 * `.git`) — distinct from the `p/`/`w/` path-scope prefix, so a worktree's
 * WIP diff doesn't read as the repo's one canonical state. */
export function WorktreeBadge() {
  return (
    <span
      className="shrink-0 rounded-md border border-sky-500/40 bg-sky-500/10 px-1 font-mono text-[10px] text-sky-500"
      title="Git worktree checkout — a linked working tree, not the primary clone"
    >
      ⬡ wt
    </span>
  );
}

/** Commits ahead/behind origin/main, next to the branch name — `↑3 ↓2`.
 * Ahead (unmerged local commits) reads emerald like a diff `+`; behind (just
 * staleness, not an attention signal) stays a muted amber. Renders nothing
 * when even with main. */
export function AheadBehind({
  stats: { commitsAhead, commitsBehind },
}: {
  stats: Pick<FolderData, "commitsAhead" | "commitsBehind">;
}) {
  if (commitsAhead === 0 && commitsBehind === 0) return null;
  return (
    <span
      className="shrink-0 font-mono text-[10.5px]"
      title={`${commitsAhead} commit${commitsAhead === 1 ? "" : "s"} ahead of origin/main, ${commitsBehind} behind`}
    >
      {commitsAhead > 0 && (
        <span className="text-emerald-600 dark:text-emerald-400">↑{commitsAhead}</span>
      )}
      {commitsAhead > 0 && commitsBehind > 0 && " "}
      {commitsBehind > 0 && <span className="text-amber-600 dark:text-amber-400">↓{commitsBehind}</span>}
    </span>
  );
}

/** The diff entry point — a real, always-visible button (never hidden behind
 * a hover or dropped when the tree is clean, so the feature stays findable).
 * Clean folders read a quiet `diff`; dirty ones carry the ± tally. */
export function DiffButton({
  stats: { filesChanged, linesAdded, linesRemoved, commitsAhead },
  onOpen,
}: {
  stats: Pick<FolderData, "filesChanged" | "linesAdded" | "linesRemoved" | "commitsAhead">;
  onOpen: () => void;
}) {
  const clean = linesAdded === 0 && linesRemoved === 0;
  return (
    <button
      type="button"
      onClick={(e) => {
        e.stopPropagation();
        onOpen();
      }}
      className="flex h-5 shrink-0 items-center gap-1 rounded-md border border-border/70 px-1.5 font-mono text-[10.5px] text-muted-foreground transition-colors hover:border-border hover:bg-accent hover:text-foreground"
      title={
        clean
          ? "No working-tree changes — view diff"
          : `${filesChanged} file${filesChanged === 1 ? "" : "s"} changed, ${commitsAhead} commit${commitsAhead === 1 ? "" : "s"} ahead — view diff`
      }
    >
      <GitCompare className="size-3" />
      {clean ? (
        <span>diff</span>
      ) : (
        <>
          <span className="text-emerald-600 dark:text-emerald-400">+{linesAdded}</span>
          <span className="text-red-600 dark:text-red-400">−{linesRemoved}</span>
        </>
      )}
    </button>
  );
}

/** Clickable `#N` chip for the folder's open PR, tinted by its checks state
 * (red failing · green passing · yellow pending · gray none). Opens GitHub. */
export function PrChip({ pr }: { pr: PrItem }) {
  const tone =
    pr.checks === "failing"
      ? "border-red-500/50 bg-red-500/10 text-red-600 hover:bg-red-500/20 dark:text-red-400"
      : pr.checks === "passing"
        ? "border-emerald-500/40 bg-emerald-500/10 text-emerald-600 hover:bg-emerald-500/20 dark:text-emerald-400"
        : pr.checks === "pending"
          ? "border-yellow-500/40 bg-yellow-500/10 text-yellow-600 hover:bg-yellow-500/20 dark:text-yellow-400"
          : "border-border/70 text-muted-foreground hover:bg-accent hover:text-foreground";
  return (
    <button
      type="button"
      onClick={(e) => {
        e.stopPropagation();
        void openExternalUrl(pr.url);
      }}
      className={cn(
        "flex h-5 shrink-0 items-center gap-1 rounded-md border px-1.5 font-mono text-[10.5px] transition-colors",
        tone,
      )}
      title={`${pr.title} — checks ${pr.checks}${pr.reviewState === "review_requested" ? ", review requested" : ""}. Open on GitHub.`}
    >
      <GitPullRequest className="size-3" />#{pr.number}
    </button>
  );
}

/** Context/cache health for a live agent session, in the row's meta cluster.
 * Quiet mono text: `41% ◔4m` while warm (⧗ for a 1h cache), `41% ❄` when cold,
 * and an ice-washed `❄ 63% compact` pill when cold at/over the threshold. */
export function CacheBadge({
  session,
  now,
  compactPct,
  onCompact,
  long = false,
}: {
  session: SessionData;
  now: number;
  compactPct: number;
  /** When set, the ❄ compact pill is clickable and runs /compact directly. */
  onCompact?: () => void;
  /** Long form spells out "compact"; the rail uses the short `❄ N%`. */
  long?: boolean;
}) {
  const d = session.agentState?.details;
  if (!session.live || !d?.contextUsed || !d.contextMax) return null;
  const pct = ctxPct(d);
  const cold = isCold(d, now);

  if (needsCompact(d, now, compactPct)) {
    const pill =
      "shrink-0 rounded-md border border-sky-500/50 bg-sky-500/10 px-1.5 font-mono text-[10.5px] text-sky-500";
    const hint = `${pct}% of context used and the prompt cache expired — resuming re-reads everything.`;
    return onCompact ? (
      <button
        type="button"
        title={`${hint} Click to /compact.`}
        onClick={(e) => {
          e.stopPropagation();
          onCompact();
        }}
        className={cn(pill, "hover:bg-sky-500/20")}
      >
        ❄ {pct}%{long && " compact"}
      </button>
    ) : (
      <span title={`${hint} Consider /compact or a fresh session.`} className={pill}>
        ❄ {pct}%{long && " compact"}
      </span>
    );
  }

  const expiring = isCacheExpiring(d, now);
  const warmth = cold
    ? "❄"
    : `${d.cacheTtlMs === 3_600_000 ? "⧗" : "◔"}${fmtMins(d.cacheExpiresAt! - now)}`;
  return (
    <span
      title={
        cold
          ? "prompt cache expired"
          : expiring
            ? "prompt cache expires soon — any message re-warms it; a cold resume re-reads everything at full price"
            : "prompt cache warm — time left"
      }
      className={cn(
        "shrink-0 font-mono text-[10.5px]",
        expiring ? "text-amber-500" : "text-muted-foreground/70",
      )}
    >
      {pct}% {warmth}
    </span>
  );
}

/** Millis → whole minutes for the cache countdown, floored at 1 ("<1m" ≈ 1m). */
export function fmtMins(ms: number): string {
  return `${Math.max(1, Math.round(ms / 60_000))}m`;
}

/** Text colors for agent-pushed status/log tones. Every hue carries a `dark:`
 * pair — never a bare palette color. */
const TONE_TEXT: Record<MetadataTone, string> = {
  neutral: "text-muted-foreground",
  info: "text-sky-600 dark:text-sky-400",
  success: "text-emerald-600 dark:text-emerald-400",
  warn: "text-amber-600 dark:text-amber-400",
  error: "text-red-600 dark:text-red-400",
};

/** The agent's own status line (`ab_set_status`, also pushed over MCP) under a
 * folder header — what the agent *says* it's doing, next to what we *observe*
 * (the session dots). Read-only by design; recent `ab_log` lines ride along in
 * the tooltip. Renders nothing when no agent has pushed a status. */
export function AgentStatusLine({
  metadata,
  now,
}: {
  metadata: FolderMetadata | null | undefined;
  now: number;
}) {
  const status = metadata?.status;
  if (!status?.text) return null;
  const tone = status.tone ?? "neutral";
  const logs = (metadata?.logs ?? []).slice(-5);
  const age = Math.max(0, now - status.ts);
  const line = (
    <span className={cn("flex min-w-0 items-center gap-1.5 text-[11px]", TONE_TEXT[tone])}>
      <span className="shrink-0 opacity-60">▸</span>
      <span className="min-w-0 truncate">{status.text}</span>
      {age >= 60_000 && (
        <span className="shrink-0 font-mono text-[10px] text-muted-foreground/60">
          {fmtMins(age)} ago
        </span>
      )}
    </span>
  );
  if (logs.length === 0) return line;
  return (
    <Tooltip>
      <TooltipTrigger asChild>{line}</TooltipTrigger>
      <TooltipContent side="bottom" align="start" className="max-w-96">
        <div className="flex flex-col gap-0.5 font-mono text-[11px]">
          {logs.map((l, i) => (
            <span key={i} className="truncate">
              {l.message}
            </span>
          ))}
        </div>
      </TooltipContent>
    </Tooltip>
  );
}

/** The folder's user-authored purpose — the "why am I here". Click to edit
 * inline (Enter saves, Esc cancels; blank clears).
 *
 * `rail` variant: a faint one-liner under the folder header, shown only when a
 * note is set. When unset it renders nothing at all (not even on hover) so the
 * folder row keeps a fixed height and the rail never jumps as the mouse moves
 * across it — set a note from the folder's "…" menu instead.
 * `band` variant: lives in the working-context band — always visible, unset
 * state included, because the band exists to answer "where am I and why". */
export function PurposeRow({
  folder,
  variant = "rail",
}: {
  folder: FolderData;
  variant?: "rail" | "band";
}) {
  const [editing, setEditing] = useState(false);
  const purpose = folder.purpose?.trim() ?? "";
  const rail = variant === "rail";
  const pad = rail ? "py-0.5 pr-3 pl-9 text-[11px]" : "text-xs";

  async function commit(text: string) {
    setEditing(false);
    const trimmed = text.trim();
    if (trimmed === purpose) return;
    await abInvoke("ab_set_folder_purpose", { dir: folder.dir, text: trimmed || null });
  }

  if (editing) {
    return (
      <div className={cn(rail && "py-0.5 pr-3 pl-9")}>
        <input
          autoFocus
          defaultValue={purpose}
          placeholder="what are you working toward here?"
          onBlur={(e) => void commit(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void commit((e.target as HTMLInputElement).value);
            if (e.key === "Escape") setEditing(false);
          }}
          className={cn(
            "w-full rounded-sm border border-input bg-background px-1.5 py-0.5 outline-none",
            rail ? "text-[11px]" : "text-xs",
          )}
        />
      </div>
    );
  }

  if (!purpose) {
    // Rail: no note → no row (a stable-height folder that doesn't resize on
    // hover). The note is set from the folder's "…" menu.
    if (rail) return null;
    return (
      <button
        type="button"
        onClick={() => setEditing(true)}
        title="Edit folder purpose"
        className={cn(
          "block w-full truncate text-left text-muted-foreground/50 hover:text-muted-foreground",
          pad,
        )}
      >
        + what are you working toward here?
      </button>
    );
  }

  return (
    <button
      type="button"
      onClick={() => setEditing(true)}
      title="Edit folder purpose"
      className={cn(
        "block w-full truncate text-left text-muted-foreground hover:text-foreground",
        pad,
      )}
    >
      {purpose}
    </button>
  );
}
