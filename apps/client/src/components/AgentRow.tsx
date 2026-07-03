import { useEffect, useRef, useState } from "react";
import type { AgentDisplay } from "../types";
import { useTheme } from "../theme/ThemeProvider";
import { formatElapsed } from "../lib/elapsed";
import { shortModel } from "../lib/shortModel";
import { collapseWS, truncate } from "../lib/truncate";
import { CAP_LOOP_REASON, CAP_SUBAGENT_DESC, CAP_THREAD_NAME } from "../lib/constants";
import { agentColor, agentIcon, agentIsUnseen, cacheLabel } from "../lib/derived";

export interface AgentRowProps {
  agent: AgentDisplay;
  spinIdx: number;
  now: number;
  isKeyboardFocused: boolean;
  onDismiss: () => void;
  onFocus: () => void;
}

/** A single agent instance (UI-SPEC §1 AgentRow). */
export function AgentRow({ agent, spinIdx, now, isKeyboardFocused, onDismiss, onFocus }: AgentRowProps) {
  const theme = useTheme();
  const P = theme.palette;
  const [flash, setFlash] = useState(false);
  const [dismissHover, setDismissHover] = useState(false);
  const flashTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => () => {
    if (flashTimer.current) clearTimeout(flashTimer.current);
  }, []);

  const triggerFlash = () => {
    setFlash(true);
    if (flashTimer.current) clearTimeout(flashTimer.current);
    flashTimer.current = setTimeout(() => setFlash(false), 150);
  };

  const iconColor = agentColor(agent, P, theme.status);
  const unseen = agentIsUnseen(agent);
  const details = agent.details;
  const running = agent.status === "busy";
  const model = running && details?.model ? shortModel(details.model) : "";
  const tool = running ? details?.lastTool : undefined;
  const subagents = running ? details?.subagents ?? [] : [];
  const loop = details?.loop && details.loop.nextWakeAt > now ? details.loop : undefined;
  const cache = cacheLabel(agent, now);
  const showElapsed = running && details?.lastActivityAt != null;

  const bg = flash
    ? P.surface2
    : isKeyboardFocused
      ? P.surface1
      : "transparent";

  return (
    <div
      className="ab-agent-row"
      style={{ backgroundColor: bg }}
      onMouseDown={(e) => {
        e.stopPropagation();
        triggerFlash();
        onFocus();
      }}
    >
      <div className="ab-agent-line ab-agent-header">
        <span className="ab-agent-icon" style={{ color: iconColor }}>
          {agentIcon(agent, spinIdx)}
        </span>
        {agent.threadName && (
          <span
            className="ab-agent-thread"
            style={{ color: unseen ? iconColor : P.overlay0 }}
          >
            {truncate(collapseWS(agent.threadName), CAP_THREAD_NAME)}
          </span>
        )}
        {showElapsed && (
          <span
            className="ab-agent-elapsed ab-dim"
            style={{ color: isKeyboardFocused ? P.subtext0 : P.overlay1 }}
          >
            {formatElapsed(now - (details?.lastActivityAt ?? now))}
          </span>
        )}
        <button
          type="button"
          className="ab-agent-dismiss"
          title="dismiss agent"
          style={{ color: dismissHover ? P.red : P.overlay0 }}
          onMouseDown={(e) => {
            e.preventDefault();
            e.stopPropagation();
            onDismiss();
          }}
          onMouseOver={() => setDismissHover(true)}
          onMouseOut={() => setDismissHover(false)}
        >
          ✕
        </button>
      </div>

      {running && details && (model || tool) && (
        <div className="ab-agent-line">
          {model && (
            <span className="ab-dim" style={{ color: P.subtext0 }}>
              {model}
            </span>
          )}
          {tool && (
            <>
              <span className="ab-dim" style={{ color: P.overlay0 }}>
                {model ? " · " : ""}
              </span>
              <span className="ab-dim" style={{ color: P.teal }}>
                {"⟶ "}
              </span>
              <span style={{ color: P.subtext0 }}>{tool}</span>
            </>
          )}
        </div>
      )}

      {subagents.length > 0 && (
        <>
          <div className="ab-agent-line">
            <span className="ab-dim" style={{ color: P.mauve }}>
              {"⚡ "}
            </span>
            <span style={{ color: P.subtext0 }}>
              {subagents.length} agent{subagents.length === 1 ? "" : "s"}
            </span>
          </div>
          {subagents.map((sa, i) => (
            <div className="ab-agent-line" key={i}>
              <span className="ab-dim" style={{ color: P.overlay0 }}>
                {"  ↳ "}
              </span>
              {sa.agentType && (
                <span className="ab-dim" style={{ color: P.teal }}>
                  {sa.agentType}
                </span>
              )}
              {sa.description && (
                <>
                  <span className="ab-dim" style={{ color: P.overlay0 }}>
                    {sa.agentType ? " · " : ""}
                  </span>
                  <span style={{ color: P.subtext0 }}>
                    {truncate(collapseWS(sa.description), CAP_SUBAGENT_DESC)}
                  </span>
                </>
              )}
            </div>
          ))}
        </>
      )}

      {loop && (
        <div className="ab-agent-line">
          <span className="ab-dim" style={{ color: P.lavender }}>
            {"⟳ "}
          </span>
          <span style={{ color: P.subtext0 }}>
            loops in {formatElapsed(loop.nextWakeAt - now)}
          </span>
          {loop.reason && (
            <span className="ab-dim" style={{ color: P.overlay0 }}>
              {" · "}
              {truncate(collapseWS(loop.reason), CAP_LOOP_REASON)}
            </span>
          )}
        </div>
      )}

      {cache && (
        <div className="ab-agent-line ab-dim" style={{ color: P.overlay0 }}>
          {cache}
        </div>
      )}
    </div>
  );
}
