import type { AgentDisplay, SessionData } from "../types";
import { useTheme } from "../theme/ThemeProvider";
import { SessionCard } from "./SessionCard";

export interface SessionListProps {
  sessions: SessionData[];
  focusedIdx: number;
  /** -1 unless the agents panel has focus for the selected card. */
  focusedAgentIdx: number;
  panelFocus: "sessions" | "agents";
  spinIdx: number;
  now: number;
  onSelect: (index: number) => void;
  onDismissAgent: (session: SessionData, agent: AgentDisplay) => void;
  onFocusAgent: (session: SessionData, agent: AgentDisplay, index: number) => void;
  onAddRepo: () => void;
}

/** The scrolling list of repo cards, or the empty state (UI-SPEC §5). */
export function SessionList(props: SessionListProps) {
  const { palette: P } = useTheme();

  if (props.sessions.length === 0) {
    return (
      <div className="ab-empty">
        <div className="ab-empty-title" style={{ color: P.subtext0 }}>
          No repos configured
        </div>
        <button
          type="button"
          className="ab-add-repo"
          style={{ color: P.text, backgroundColor: P.surface0, borderColor: P.surface2 }}
          onClick={props.onAddRepo}
        >
          + Add repo
        </button>
        <div className="ab-empty-hint" style={{ color: P.overlay0 }}>
          (add-repo dialog is a phase-5 placeholder)
        </div>
      </div>
    );
  }

  return (
    <div className="ab-list">
      {props.sessions.map((session, i) => (
        <SessionCard
          key={session.name}
          session={session}
          isFocused={i === props.focusedIdx}
          isCurrent={false}
          spinIdx={props.spinIdx}
          now={props.now}
          focusedAgentIdx={
            props.panelFocus === "agents" && i === props.focusedIdx ? props.focusedAgentIdx : -1
          }
          onSelect={() => props.onSelect(i)}
          onDismissAgent={(agent) => props.onDismissAgent(session, agent)}
          onFocusAgent={(agent, idx) => props.onFocusAgent(session, agent, idx)}
        />
      ))}
    </div>
  );
}
