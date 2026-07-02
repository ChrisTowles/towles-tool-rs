// Derived display helpers — the logic that SessionCard.tsx / StatusBar.tsx
// inline as reactive accessors, extracted here as pure functions over
// `(session/agent, palette, statusColors, now, flags)` so they can be unit
// tested (UI-SPEC §6). Behaviour matches slot-1 verbatim.

import type { AgentDisplay, AgentStatus, MetadataTone, SessionData } from "../types";
import type { Theme, ThemePalette } from "./themes";
import {
  DONE_ICON,
  ERROR_ICON,
  IDLE_ICON,
  INTERRUPTED_ICON,
  UNSEEN_ICON,
} from "./constants";
import { liveStatusIcon, unseenTerminalColor } from "./statusVisuals";

const TERMINAL: AgentStatus[] = ["done", "error", "interrupted"];

export function isTerminalStatus(status: AgentStatus): boolean {
  return TERMINAL.includes(status);
}

// --- Session-level ---

/** The session's headline status (highest-priority agent, or idle). */
export function sessionStatus(session: SessionData): AgentStatus {
  return session.agentState?.status ?? "idle";
}

/** How many of the session's agents are currently running. */
export function runningCount(session: SessionData): number {
  return session.agents?.filter((a) => a.status === "running").length ?? 0;
}

/** True when the session is flagged unseen AND its headline status is terminal. */
export function isUnseenTerminal(session: SessionData): boolean {
  return session.unseen && isTerminalStatus(sessionStatus(session));
}

export interface SessionFlags {
  isCurrent: boolean;
  isFocused: boolean;
}

/**
 * Accent-bar color. Precedence (UI-SPEC §1A): isCurrent→green ·
 * unseenTerminal→unseenTerminalColor · error→red · interrupted→peach ·
 * running→yellow · waiting→blue · question→green · isFocused→lavender · else
 * transparent.
 */
export function accentColor(
  session: SessionData,
  palette: ThemePalette,
  flags: SessionFlags,
): string {
  if (flags.isCurrent) return palette.green;
  if (isUnseenTerminal(session)) return unseenTerminalColor(sessionStatus(session), palette);
  const s = sessionStatus(session);
  if (s === "error") return palette.red;
  if (s === "interrupted") return palette.peach;
  if (s === "running") return palette.yellow;
  if (s === "waiting") return palette.blue;
  if (s === "question") return palette.green;
  if (flags.isFocused) return palette.lavender;
  return "transparent";
}

/** The status-cell glyph (spinner/waiting/question, or unseen dot, or ""). */
export function statusIcon(session: SessionData, spinIdx: number): string {
  const live = liveStatusIcon(sessionStatus(session), spinIdx);
  if (live) return live;
  return isUnseenTerminal(session) ? UNSEEN_ICON : "";
}

/** The status-cell color. */
export function statusColor(
  session: SessionData,
  palette: ThemePalette,
  statusColors: Theme["status"],
): string {
  if (isUnseenTerminal(session)) return unseenTerminalColor(sessionStatus(session), palette);
  return statusColors[sessionStatus(session)];
}

/** Whether any DiffStats span would render. */
export function hasDiff(session: SessionData): boolean {
  const { linesAdded, linesRemoved, commitsDelta, filesChanged } = session;
  return !!(linesAdded || linesRemoved || commitsDelta || filesChanged);
}

/** The joined metadata-summary text (empty when there is nothing to show). */
export function metaSummary(session: SessionData): string {
  const meta = session.metadata;
  if (!meta) return "";
  const parts: string[] = [];
  if (meta.status) parts.push(meta.status.text);
  if (meta.progress) {
    if (meta.progress.current != null && meta.progress.total != null) {
      parts.push(`${meta.progress.current}/${meta.progress.total}`);
    } else if (meta.progress.percent != null) {
      parts.push(`${Math.round(meta.progress.percent * 100)}%`);
    }
    if (meta.progress.label) parts.push(meta.progress.label);
  }
  return parts.join(" · ");
}

export function metaTone(session: SessionData): MetadataTone | undefined {
  return session.metadata?.status?.tone;
}

// --- Agent-level ---

export function agentIsTerminal(agent: AgentDisplay): boolean {
  return isTerminalStatus(agent.status);
}

export function agentIsUnseen(agent: AgentDisplay): boolean {
  return agentIsTerminal(agent) && agent.unseen === true;
}

/** AgentRow line-1 glyph (UI-SPEC §1 AgentRow). */
export function agentIcon(agent: AgentDisplay, spinIdx: number): string {
  if (agentIsUnseen(agent)) return UNSEEN_ICON;
  if (agentIsTerminal(agent)) {
    if (agent.status === "done") return DONE_ICON;
    if (agent.status === "error") return ERROR_ICON;
    return INTERRUPTED_ICON;
  }
  return liveStatusIcon(agent.status, spinIdx) || IDLE_ICON;
}

/** AgentRow line-1 icon color. */
export function agentColor(
  agent: AgentDisplay,
  palette: ThemePalette,
  statusColors: Theme["status"],
): string {
  if (agentIsTerminal(agent)) {
    if (agentIsUnseen(agent)) return unseenTerminalColor(agent.status, palette);
    if (agent.status === "error") return palette.red;
    if (agent.status === "interrupted") return palette.peach;
    return palette.green;
  }
  return statusColors[agent.status];
}

/**
 * Cache-line label (UI-SPEC §1 CacheLine). `expiresAt = cacheExpiresAt ??
 * (lastActivityAt + 1h)`; `minutesLeft = ceil((expiresAt - now)/60000)`;
 * `"cache expired"` if ≤0 else `"cache {minutesLeft}m"`. null when neither
 * timestamp is available.
 */
export function cacheLabel(agent: AgentDisplay, now: number): string | null {
  const details = agent.details;
  if (!details) return null;
  const expiresAt =
    details.cacheExpiresAt ??
    (details.lastActivityAt != null ? details.lastActivityAt + 60 * 60 * 1000 : null);
  if (expiresAt == null) return null;
  const minutesLeft = Math.ceil((expiresAt - now) / 60_000);
  return minutesLeft <= 0 ? "cache expired" : `cache ${minutesLeft}m`;
}

// --- Board-level aggregates (StatusBar, UI-SPEC §2) ---

export interface BoardCounts {
  sessionCount: number;
  runningCount: number;
  errorCount: number;
  unseenCount: number;
}

export function boardCounts(sessions: SessionData[]): BoardCounts {
  let running = 0;
  let error = 0;
  let unseen = 0;
  for (const s of sessions) {
    for (const a of s.agents ?? []) {
      if (a.status === "running") running++;
      if (a.status === "error") error++;
    }
    if (s.unseen) unseen++;
  }
  return {
    sessionCount: sessions.length,
    runningCount: running,
    errorCount: error,
    unseenCount: unseen,
  };
}

/** Whether any agent anywhere is running (drives the spinner tick). */
export function anyAgentRunning(sessions: SessionData[]): boolean {
  return sessions.some((s) => (s.agents ?? []).some((a) => a.status === "running"));
}
