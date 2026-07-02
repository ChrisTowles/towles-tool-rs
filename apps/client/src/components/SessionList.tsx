import { useState } from "react";
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
  onAddRepo: (path: string) => void;
}

/** Inline add-repo input shown in the empty state. */
function EmptyState({ onAddRepo }: { onAddRepo: (path: string) => void }) {
  const { palette: P } = useTheme();
  const [path, setPath] = useState("");
  const submit = () => {
    const trimmed = path.trim();
    if (trimmed) {
      onAddRepo(trimmed);
      setPath("");
    }
  };
  return (
    <div className="ab-empty">
      <div className="ab-empty-title" style={{ color: P.subtext0 }}>
        No repos configured
      </div>
      <div className="ab-add-repo-row">
        <input
          className="ab-input"
          type="text"
          placeholder="/absolute/path/to/repo"
          value={path}
          onChange={(e) => setPath(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              submit();
            }
          }}
          style={{ color: P.text, backgroundColor: P.surface0, borderColor: P.surface2 }}
        />
        <button
          type="button"
          className="ab-add-repo"
          style={{ color: P.crust, backgroundColor: P.green }}
          onClick={submit}
          disabled={!path.trim()}
        >
          Add repo
        </button>
      </div>
    </div>
  );
}

/** The scrolling list of repo cards, or the empty state (UI-SPEC §5). */
export function SessionList(props: SessionListProps) {
  if (props.sessions.length === 0) {
    return <EmptyState onAddRepo={props.onAddRepo} />;
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
