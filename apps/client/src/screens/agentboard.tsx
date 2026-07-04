import { useMemo, useState } from "react";
import { Plus, TerminalSquare, X } from "lucide-react";
import { TerminalView } from "@/components/terminal-view";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";
import { statusColor, useAgentboardState, type SessionData } from "@/lib/agentboard";

/** Parent directory of a session's folder, used to group sessions. */
function parentDir(dir: string): string {
  const trimmed = dir.replace(/\/+$/, "");
  const idx = trimmed.lastIndexOf("/");
  return idx > 0 ? trimmed.slice(0, idx) : trimmed || "/";
}

/** Last few path segments, for compact display. */
function shortPath(p: string, segments = 2): string {
  const parts = p.replace(/\/+$/, "").split("/").filter(Boolean);
  const tail = parts.slice(-segments).join("/");
  return parts.length > segments ? `…/${tail}` : `/${tail}`;
}

type Group = { parent: string; sessions: SessionData[] };

function groupByFolder(sessions: SessionData[]): Group[] {
  const map = new Map<string, SessionData[]>();
  for (const s of sessions) {
    const parent = parentDir(s.dir);
    const bucket = map.get(parent) ?? [];
    bucket.push(s);
    map.set(parent, bucket);
  }
  return [...map.entries()]
    .map(([parent, groupSessions]) => ({ parent, sessions: groupSessions }))
    .sort((a, b) => a.parent.localeCompare(b.parent));
}

function SessionRow({
  session,
  active,
  onSelect,
}: {
  session: SessionData;
  active: boolean;
  onSelect: () => void;
}) {
  const status = session.agentState?.status;
  return (
    <button
      type="button"
      onClick={onSelect}
      className={cn(
        "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm",
        active ? "bg-accent text-accent-foreground" : "hover:bg-accent/50",
      )}
    >
      <span
        className={cn(
          "size-2 shrink-0 rounded-full",
          status ? statusColor(status) : "bg-muted-foreground/30",
        )}
      />
      <span className="min-w-0 flex-1 truncate font-medium">{session.name}</span>
      {session.unseen && <span className="size-1.5 shrink-0 rounded-full bg-blue-500" />}
      {session.branch && (
        <span className="max-w-24 shrink-0 truncate font-mono text-xs text-muted-foreground">
          {session.branch}
        </span>
      )}
    </button>
  );
}

/**
 * Agentboard: the app-native (non-tmux) board. Left = live sessions grouped by
 * folder, from the `ab_get_state` / `agentboard://state` bridge. Right = one or
 * more embedded terminals for the selected session, each spawned in that
 * session's folder. Every opened terminal stays mounted (toggled with `hidden`)
 * so its shell and scrollback survive switching sessions/tabs.
 */
export function AgentboardScreen() {
  const state = useAgentboardState();
  const groups = useMemo(() => groupByFolder(state.sessions), [state.sessions]);

  const [selected, setSelected] = useState<string | null>(null);
  // Terminal ids opened per session, and which one is active.
  const [opened, setOpened] = useState<Record<string, number[]>>({});
  const [activeTerm, setActiveTerm] = useState<Record<string, number>>({});

  const byName = useMemo(() => {
    const m = new Map<string, SessionData>();
    for (const s of state.sessions) m.set(s.name, s);
    return m;
  }, [state.sessions]);

  function selectSession(name: string) {
    setSelected(name);
    setOpened((prev) => (prev[name] ? prev : { ...prev, [name]: [0] }));
    setActiveTerm((prev) => (name in prev ? prev : { ...prev, [name]: 0 }));
  }

  function addTerminal(name: string) {
    setOpened((prev) => {
      const ids = prev[name] ?? [0];
      const next = ids.length ? Math.max(...ids) + 1 : 0;
      setActiveTerm((a) => ({ ...a, [name]: next }));
      return { ...prev, [name]: [...ids, next] };
    });
  }

  function closeTerminal(name: string, id: number) {
    setOpened((prev) => {
      const ids = (prev[name] ?? []).filter((n) => n !== id);
      setActiveTerm((a) => (a[name] === id ? { ...a, [name]: ids[ids.length - 1] ?? -1 } : a));
      return { ...prev, [name]: ids };
    });
  }

  const selectedSession = selected ? byName.get(selected) : undefined;
  const selectedIds = selected ? (opened[selected] ?? []) : [];

  return (
    <div className="flex h-full min-h-0">
      {/* Session list, grouped by folder. */}
      <div className="flex w-64 shrink-0 flex-col border-r">
        <div className="flex items-center gap-2 px-3 py-2 text-xs font-medium text-muted-foreground">
          <TerminalSquare className="size-4" />
          Sessions
        </div>
        <ScrollArea className="flex-1">
          <div className="flex flex-col gap-3 p-2">
            {groups.length === 0 && (
              <p className="px-2 py-6 text-center text-sm text-muted-foreground">
                No sessions yet.
              </p>
            )}
            {groups.map((group) => (
              <div key={group.parent} className="flex flex-col gap-0.5">
                <div className="truncate px-2 py-1 font-mono text-xs text-muted-foreground/70">
                  {shortPath(group.parent)}
                </div>
                {group.sessions.map((session) => (
                  <SessionRow
                    key={session.name}
                    session={session}
                    active={selected === session.name}
                    onSelect={() => selectSession(session.name)}
                  />
                ))}
              </div>
            ))}
          </div>
        </ScrollArea>
      </div>

      {/* Terminal area for the selected session. */}
      <div className="flex min-w-0 flex-1 flex-col">
        {selected && (
          <div className="flex items-center gap-1 border-b px-2 py-1">
            <span className="mr-2 truncate font-mono text-xs text-muted-foreground">
              {selectedSession?.dir ?? selected}
            </span>
            <div className="flex items-center gap-1">
              {selectedIds.map((id, i) => (
                <div
                  key={id}
                  className={cn(
                    "group flex items-center gap-1 rounded-md pl-2 pr-1 py-1 text-xs",
                    activeTerm[selected] === id
                      ? "bg-accent text-accent-foreground"
                      : "hover:bg-accent/50",
                  )}
                >
                  <button
                    type="button"
                    onClick={() => setActiveTerm((a) => ({ ...a, [selected]: id }))}
                  >
                    shell {i + 1}
                  </button>
                  <button
                    type="button"
                    onClick={() => closeTerminal(selected, id)}
                    className="rounded p-0.5 opacity-0 hover:bg-background group-hover:opacity-100"
                    aria-label="close terminal"
                  >
                    <X className="size-3" />
                  </button>
                </div>
              ))}
            </div>
            <Button
              variant="ghost"
              size="sm"
              className="ml-1 h-6 px-2"
              onClick={() => addTerminal(selected)}
            >
              <Plus className="size-3" /> terminal
            </Button>
          </div>
        )}

        {/* All opened terminals across all sessions render here, kept mounted
            and stacked; only the active terminal of the selected session shows. */}
        <div className="relative min-h-0 flex-1">
          {Object.entries(opened).flatMap(([name, ids]) =>
            ids.map((id) => {
              const visible = selected === name && activeTerm[name] === id;
              return (
                <div key={`${name}:${id}`} hidden={!visible} className="absolute inset-0">
                  <TerminalView
                    termId={`${name}:${id}`}
                    cwd={byName.get(name)?.dir}
                    onExit={() => closeTerminal(name, id)}
                  />
                </div>
              );
            }),
          )}
          {!selected && (
            <div className="flex h-full flex-col items-center justify-center gap-2 text-muted-foreground">
              <TerminalSquare className="size-10" />
              <p className="text-sm">Select a session to open a terminal in its folder.</p>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
