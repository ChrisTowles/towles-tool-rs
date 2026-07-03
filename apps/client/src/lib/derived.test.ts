import { describe, it, expect } from "vitest";
import type { AgentDisplay, SessionData } from "../types";
import { BUILTIN_THEMES } from "./themes";
import {
  accentColor,
  agentColor,
  agentIcon,
  anyAgentRunning,
  boardCounts,
  cacheLabel,
  hasDiff,
  isUnseenTerminal,
  metaSummary,
  busyCount,
  sessionStatus,
  statusColor,
  statusIcon,
} from "./derived";
import { SPINNERS, UNSEEN_ICON } from "./constants";

const theme = BUILTIN_THEMES["catppuccin-mocha"];
const P = theme.palette;
const SC = theme.status;

function agent(over: Partial<AgentDisplay> = {}): AgentDisplay {
  return { agent: "claude", session: "s", status: "idle", ts: 0, ...over };
}

function session(over: Partial<SessionData> = {}): SessionData {
  return {
    name: "s",
    dir: "/tmp/s",
    branch: "main",
    filesChanged: 0,
    linesAdded: 0,
    linesRemoved: 0,
    commitsDelta: 0,
    unseen: false,
    agentState: null,
    agents: [],
    metadata: null,
    ...over,
  };
}

describe("sessionStatus / busyCount", () => {
  it("defaults to idle without an agentState", () => {
    expect(sessionStatus(session())).toBe("idle");
  });
  it("counts running agents", () => {
    const s = session({ agents: [agent({ status: "busy" }), agent({ status: "complete" })] });
    expect(busyCount(s)).toBe(1);
  });
});

describe("accentColor precedence", () => {
  it("isCurrent wins over everything → green", () => {
    const s = session({ agentState: agent({ status: "error" }), unseen: true });
    expect(accentColor(s, P, { isCurrent: true, isFocused: false })).toBe(P.green);
  });
  it("unseen terminal beats plain status", () => {
    const s = session({ agentState: agent({ status: "error" }), unseen: true });
    expect(accentColor(s, P, { isCurrent: false, isFocused: false })).toBe(P.red);
  });
  it("busy→yellow, waiting→blue", () => {
    expect(
      accentColor(session({ agentState: agent({ status: "busy" }) }), P, {
        isCurrent: false,
        isFocused: false,
      }),
    ).toBe(P.yellow);
    expect(
      accentColor(session({ agentState: agent({ status: "waiting" }) }), P, {
        isCurrent: false,
        isFocused: false,
      }),
    ).toBe(P.blue);
  });
  it("focused idle→lavender, unfocused idle→transparent", () => {
    expect(accentColor(session(), P, { isCurrent: false, isFocused: true })).toBe(P.lavender);
    expect(accentColor(session(), P, { isCurrent: false, isFocused: false })).toBe("transparent");
  });
});

describe("statusIcon / statusColor", () => {
  it("shows spinner frame while running", () => {
    expect(statusIcon(session({ agentState: agent({ status: "busy" }) }), 2)).toBe(SPINNERS[2]);
  });
  it("shows the unseen dot for an unseen terminal session", () => {
    const s = session({ agentState: agent({ status: "complete" }), unseen: true });
    expect(statusIcon(s, 0)).toBe(UNSEEN_ICON);
    expect(statusColor(s, P, SC)).toBe(P.teal);
  });
  it("is empty for a seen idle session", () => {
    expect(statusIcon(session(), 0)).toBe("");
  });
  it("uses the theme status color for a seen busy session", () => {
    expect(statusColor(session({ agentState: agent({ status: "busy" }) }), P, SC)).toBe(
      SC.busy,
    );
  });
});

describe("isUnseenTerminal / hasDiff", () => {
  it("is false when unseen but status is non-terminal", () => {
    expect(isUnseenTerminal(session({ agentState: agent({ status: "busy" }), unseen: true }))).toBe(
      false,
    );
  });
  it("detects any non-zero diff stat", () => {
    expect(hasDiff(session({ linesAdded: 3 }))).toBe(true);
    expect(hasDiff(session({ commitsDelta: -1 }))).toBe(true);
    expect(hasDiff(session())).toBe(false);
  });
});

describe("metaSummary", () => {
  it("returns empty when metadata is null", () => {
    expect(metaSummary(session())).toBe("");
  });
  it("joins status, current/total, and label with ' · '", () => {
    const s = session({
      metadata: {
        status: { text: "building", tone: "info", ts: 0 },
        progress: { current: 2, total: 5, label: "step", ts: 0 },
        logs: [],
      },
    });
    expect(metaSummary(s)).toBe("building · 2/5 · step");
  });
  it("renders percent when current/total are absent", () => {
    const s = session({
      metadata: {
        status: null,
        progress: { percent: 0.42, ts: 0 },
        logs: [],
      },
    });
    expect(metaSummary(s)).toBe("42%");
  });
});

describe("agentIcon / agentColor", () => {
  it("uses the unseen dot for unseen terminal agents", () => {
    const a = agent({ status: "error", unseen: true });
    expect(agentIcon(a, 0)).toBe(UNSEEN_ICON);
    expect(agentColor(a, P, SC)).toBe(P.red);
  });
  it("uses ✓ / ✗ / ⚠ for seen terminal agents", () => {
    expect(agentIcon(agent({ status: "complete" }), 0)).toBe("✓");
    expect(agentIcon(agent({ status: "error" }), 0)).toBe("✗");
    expect(agentIcon(agent({ status: "interrupted" }), 0)).toBe("⚠");
    expect(agentColor(agent({ status: "complete" }), P, SC)).toBe(P.green);
    expect(agentColor(agent({ status: "interrupted" }), P, SC)).toBe(P.peach);
  });
  it("falls back to ○ for idle", () => {
    expect(agentIcon(agent({ status: "idle" }), 0)).toBe("○");
  });
  it("shows the spinner for a running agent", () => {
    expect(agentIcon(agent({ status: "busy" }), 5)).toBe(SPINNERS[5]);
  });
});

describe("cacheLabel", () => {
  const now = 1_000_000;
  it("is null without details", () => {
    expect(cacheLabel(agent(), now)).toBeNull();
  });
  it("uses cacheExpiresAt directly", () => {
    const a = agent({ details: { cacheExpiresAt: now + 5 * 60_000 } });
    expect(cacheLabel(a, now)).toBe("cache 5m");
  });
  it("derives a 1h window from lastActivityAt", () => {
    const a = agent({ details: { lastActivityAt: now } });
    expect(cacheLabel(a, now)).toBe("cache 60m");
  });
  it("says expired when the window has passed", () => {
    const a = agent({ details: { cacheExpiresAt: now - 1 } });
    expect(cacheLabel(a, now)).toBe("cache expired");
  });
  it("ceils partial minutes", () => {
    const a = agent({ details: { cacheExpiresAt: now + 90_000 } });
    expect(cacheLabel(a, now)).toBe("cache 2m");
  });
});

describe("boardCounts / anyAgentRunning", () => {
  const sessions = [
    session({ agents: [agent({ status: "busy" }), agent({ status: "error" })], unseen: true }),
    session({ agents: [agent({ status: "busy" })] }),
    session({ agents: [agent({ status: "complete" })], unseen: true }),
  ];
  it("sums running/error over agents and counts unseen sessions", () => {
    expect(boardCounts(sessions)).toEqual({
      sessionCount: 3,
      busyCount: 2,
      errorCount: 1,
      unseenCount: 2,
    });
  });
  it("detects any running agent", () => {
    expect(anyAgentRunning(sessions)).toBe(true);
    expect(anyAgentRunning([session({ agents: [agent({ status: "complete" })] })])).toBe(false);
  });
});
