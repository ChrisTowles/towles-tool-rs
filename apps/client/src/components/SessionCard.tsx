import type { AgentDisplay, SessionData } from "../types";
import { useTheme } from "../theme/ThemeProvider";
import { truncate } from "../lib/truncate";
import { CAP_BRANCH, CAP_NAME } from "../lib/constants";
import { familyColor } from "../lib/familyColor";
import { toneColor } from "../lib/toneColor";
import {
  accentColor,
  hasDiff,
  metaSummary,
  metaTone,
  runningCount,
  statusColor,
  statusIcon,
} from "../lib/derived";
import { DiffStats } from "./DiffStats";
import { AgentRow } from "./AgentRow";

export interface SessionCardProps {
  session: SessionData;
  isFocused: boolean;
  isCurrent: boolean;
  spinIdx: number;
  now: number;
  /** Index of the keyboard-focused agent row, or -1 when the list panel has focus. */
  focusedAgentIdx: number;
  onSelect: () => void;
  onDismissAgent: (agent: AgentDisplay) => void;
  onFocusAgent: (agent: AgentDisplay, index: number) => void;
}

/** A repo card (UI-SPEC §1). */
export function SessionCard(props: SessionCardProps) {
  const { session, isFocused, isCurrent, spinIdx, now } = props;
  const theme = useTheme();
  const P = theme.palette;

  const accent = accentColor(session, P, { isCurrent, isFocused });
  const family = familyColor(session.name, P);
  const icon = statusIcon(session, spinIdx);
  const running = runningCount(session);

  const nameColor = isFocused ? P.text : isCurrent ? P.subtext1 : family;
  const bold = isFocused || isCurrent;

  const summary = metaSummary(session);
  const agents = session.agents ?? [];

  return (
    <div
      className="ab-card"
      style={{ backgroundColor: isFocused ? P.surface0 : "transparent" }}
      onMouseDown={props.onSelect}
    >
      {/* Accent bar */}
      <span className="ab-accent" style={{ color: accent === "transparent" ? "transparent" : accent }}>
        {accent === "transparent" ? " " : "▌"}
      </span>
      {accent === "transparent" && (
        <span className="ab-accent ab-dim" style={{ color: family }}>
          ▎
        </span>
      )}

      <div className="ab-card-body">
        {/* Header: name + diff + status cell */}
        <div className="ab-card-header">
          <span
            className="ab-card-name"
            style={{ color: nameColor, fontWeight: bold ? 700 : 400 }}
          >
            {truncate(session.name, CAP_NAME)}
          </span>
          {hasDiff(session) && <DiffStats session={session} />}
          <span className="ab-status-cell" style={{ color: statusColor(session, P, theme.status) }}>
            {icon ? ` ${icon}${running > 1 ? running : ""}` : ""}
          </span>
        </div>

        {/* Branch */}
        {session.branch && (
          <div className="ab-branch" style={{ color: isFocused ? P.pink : P.overlay0 }}>
            {truncate(session.branch, CAP_BRANCH)}
          </div>
        )}

        {/* Metadata summary */}
        {summary && (
          <div
            className="ab-meta-summary ab-dim"
            style={{ color: toneColor(metaTone(session), P) }}
          >
            {summary}
          </div>
        )}

        {/* Agents */}
        {agents.map((agent, i) => (
          <AgentRow
            key={`${agent.agent}:${agent.threadId ?? i}`}
            agent={agent}
            spinIdx={spinIdx}
            now={now}
            isKeyboardFocused={i === props.focusedAgentIdx}
            onDismiss={() => props.onDismissAgent(agent)}
            onFocus={() => props.onFocusAgent(agent, i)}
          />
        ))}
      </div>
    </div>
  );
}
