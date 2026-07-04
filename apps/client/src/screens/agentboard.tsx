import { useEffect, useMemo, useState } from "react";
import {
  CalendarClock,
  Columns2,
  GitPullRequest,
  Inbox,
  TerminalSquare,
  X,
} from "lucide-react";
import { TerminalView } from "@/components/terminal-view";
import { Button } from "@/components/ui/button";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";
import { statusColor, useAgentboardState, type SessionData } from "@/lib/agentboard";
import { fmtAge, fmtCountdown, useStoreSnapshot, type CalEvent, type PrItem } from "@/lib/data";
import { useWorkspace } from "@/lib/workspace";

const MAX_PANES = 4;

type FeedItem =
  | { kind: "agent"; key: string; ts: number; session: SessionData; status: string }
  | { kind: "pr"; key: string; ts: number; pr: PrItem; status: string }
  | { kind: "event"; key: string; ts: number; event: CalEvent; status: string };

const KIND_BORDER: Record<FeedItem["kind"], string> = {
  agent: "border-l-amber-500",
  pr: "border-l-red-500",
  event: "border-l-blue-500",
};

/**
 * Agentboard: attention inbox + split terminals. Left = a "Needs you" feed that
 * merges waiting/errored agent sessions (from the `agentboard://state` bridge),
 * failing/review-requested PRs and imminent calendar events (from the store
 * snapshot); agent state is REPORTED, never re-rendered — the real TUI lives in
 * the PTY. Right = side-by-side terminals for the selected session. Every
 * session's pane group stays mounted (toggled with `hidden`) so shells and
 * scrollback survive switching.
 */
export function AgentboardScreen() {
  const state = useAgentboardState();
  const { snapshot } = useStoreSnapshot();
  const { openTab } = useWorkspace();
  const now = Date.now();

  const [selected, setSelected] = useState<string | null>(null);
  // Ordered terminal pane ids per session (side-by-side splits).
  const [panes, setPanes] = useState<Record<string, number[]>>({});

  const byName = useMemo(() => {
    const m = new Map<string, SessionData>();
    for (const s of state.sessions) m.set(s.name, s);
    return m;
  }, [state.sessions]);

  function selectSession(name: string) {
    setSelected(name);
    setPanes((prev) => (prev[name] ? prev : { ...prev, [name]: [0] }));
  }

  function addPane(name: string) {
    setPanes((prev) => {
      const ids = prev[name] ?? [0];
      if (ids.length >= MAX_PANES) return prev;
      const next = ids.length ? Math.max(...ids) + 1 : 0;
      return { ...prev, [name]: [...ids, next] };
    });
  }

  function closePane(name: string, id: number) {
    setPanes((prev) => ({ ...prev, [name]: (prev[name] ?? []).filter((n) => n !== id) }));
  }

  // ⌘D splits the selected session (no-op when nothing is selected).
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "d" && selected) {
        e.preventDefault();
        addPane(selected);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [selected]);

  const feed = useMemo<FeedItem[]>(() => {
    const items: FeedItem[] = [];
    for (const s of state.sessions) {
      const st = s.agentState?.status;
      if (st === "waiting" || st === "error") {
        items.push({
          kind: "agent",
          key: `agent:${s.name}`,
          ts: s.agentState?.ts ?? 0,
          session: s,
          status: st === "waiting" ? "Waiting — needs your input" : "Errored — needs a look",
        });
      }
    }
    items.sort((a, b) => a.ts - b.ts); // oldest agent event first
    for (const p of snapshot.prs) {
      if (p.checks === "failing" || p.reviewState === "review_requested") {
        items.push({
          kind: "pr",
          key: `pr:${p.repo}#${p.number}`,
          ts: p.updatedTs,
          pr: p,
          status: p.checks === "failing" ? "Checks failing" : "Review requested",
        });
      }
    }
    for (const ev of snapshot.events) {
      const until = ev.startTs - now;
      if (until > 0 && until <= 30 * 60_000) {
        items.push({
          kind: "event",
          key: `event:${ev.id}`,
          ts: ev.startTs,
          event: ev,
          status: `Starts in ${fmtCountdown(until)}`,
        });
      }
    }
    return items;
  }, [state.sessions, snapshot.prs, snapshot.events, now]);

  // Everything that isn't shouting for attention, as compact one-liners.
  const quiet = useMemo(
    () =>
      state.sessions.filter(
        (s) => s.agentState?.status !== "waiting" && s.agentState?.status !== "error",
      ),
    [state.sessions],
  );

  const selectedSession = selected ? byName.get(selected) : undefined;
  const selectedPanes = selected ? (panes[selected] ?? []) : [];

  return (
    <div className="flex h-full min-h-0">
      {/* Needs-you feed + quiet sessions. */}
      <div className="flex w-80 shrink-0 flex-col border-r">
        <div className="flex items-center gap-2 px-3 py-2 text-xs font-medium text-muted-foreground">
          <Inbox className="size-4" />
          Needs you
        </div>
        <ScrollArea className="flex-1">
          <div className="flex flex-col gap-2 p-2">
            {state.sessions.length === 0 && feed.length === 0 && (
              <p className="px-2 py-6 text-center text-sm text-muted-foreground">
                No sessions yet. Start one with <span className="font-mono">ttr agentboard</span>.
              </p>
            )}
            {feed.length === 0 && state.sessions.length > 0 && (
              <p className="px-2 py-4 text-center text-xs text-muted-foreground">
                Nothing needs you right now.
              </p>
            )}
            {feed.map((item) => (
              <FeedCard
                key={item.key}
                item={item}
                now={now}
                onSelect={selectSession}
                openCalendar={() => openTab("email-calendar")}
              />
            ))}

            {quiet.length > 0 && (
              <div className="mt-2 flex flex-col gap-0.5 border-t pt-2">
                <div className="px-2 py-1 text-[10px] font-medium uppercase tracking-wide text-muted-foreground/70">
                  Quiet
                </div>
                {quiet.map((s) => (
                  <button
                    key={s.name}
                    type="button"
                    onClick={() => selectSession(s.name)}
                    className={cn(
                      "flex w-full items-center gap-2 rounded-md px-2 py-1 text-left text-sm",
                      selected === s.name ? "bg-accent text-accent-foreground" : "hover:bg-accent/50",
                    )}
                  >
                    <span
                      className={cn(
                        "size-2 shrink-0 rounded-full",
                        s.agentState?.status
                          ? statusColor(s.agentState.status)
                          : "bg-muted-foreground/30",
                      )}
                    />
                    <span className="min-w-0 flex-1 truncate">{s.name}</span>
                  </button>
                ))}
              </div>
            )}
          </div>
        </ScrollArea>
      </div>

      {/* Terminal area for the selected session. */}
      <div className="flex min-w-0 flex-1 flex-col">
        {selectedSession && (
          <div className="flex items-center gap-2 border-b px-2 py-1">
            <span className="truncate text-sm font-medium">{selectedSession.name}</span>
            <span className="min-w-0 flex-1 truncate font-mono text-xs text-muted-foreground">
              {selectedSession.dir}
            </span>
            <Button
              variant="ghost"
              size="sm"
              className="h-6 shrink-0 px-2"
              disabled={selectedPanes.length >= MAX_PANES}
              onClick={() => addPane(selectedSession.name)}
            >
              <Columns2 className="size-3" /> Split
            </Button>
          </div>
        )}

        {/* Every session's pane group stays mounted; only the selected one shows. */}
        <div className="relative min-h-0 flex-1">
          {Object.entries(panes).map(([name, ids]) => {
            const visible = selected === name && ids.length > 0;
            const dir = byName.get(name)?.dir;
            return (
              <div key={name} hidden={!visible} className="absolute inset-0">
                <ResizablePanelGroup orientation="horizontal" className="size-full">
                  {ids.map((id, i) => (
                    <PaneSlot key={id} first={i === 0}>
                      <div className="flex h-full flex-col">
                        <div className="flex h-6 shrink-0 items-center justify-between border-b px-2 text-xs text-muted-foreground">
                          <span>shell {i + 1}</span>
                          <button
                            type="button"
                            onClick={() => closePane(name, id)}
                            className="rounded p-0.5 hover:bg-accent"
                            aria-label="close pane"
                          >
                            <X className="size-3" />
                          </button>
                        </div>
                        <div className="min-h-0 flex-1">
                          <TerminalView
                            termId={`${name}:${id}`}
                            cwd={dir}
                            onExit={() => closePane(name, id)}
                          />
                        </div>
                      </div>
                    </PaneSlot>
                  ))}
                </ResizablePanelGroup>
              </div>
            );
          })}
          {(!selectedSession || selectedPanes.length === 0) && (
            <div className="flex h-full flex-col items-center justify-center gap-2 text-muted-foreground">
              <TerminalSquare className="size-10" />
              <p className="text-sm">Select a session.</p>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

/** A resizable pane plus the handle that precedes it (skipped for the first). */
function PaneSlot({ first, children }: { first: boolean; children: React.ReactNode }) {
  return (
    <>
      {!first && <ResizableHandle withHandle />}
      <ResizablePanel>{children}</ResizablePanel>
    </>
  );
}

function FeedCard({
  item,
  now,
  onSelect,
  openCalendar,
}: {
  item: FeedItem;
  now: number;
  onSelect: (name: string) => void;
  openCalendar: () => void;
}) {
  const meta =
    item.kind === "agent"
      ? { label: "Agent", icon: TerminalSquare, title: item.session.name, age: fmtAge(item.ts, now) }
      : item.kind === "pr"
        ? {
            label: "Pull request",
            icon: GitPullRequest,
            title: `${item.pr.repo.split("/").pop()} #${item.pr.number}`,
            age: fmtAge(item.ts, now),
          }
        : {
            label: "Calendar",
            icon: CalendarClock,
            title: item.event.title,
            age: fmtCountdown(item.event.startTs - now),
          };

  const onClick = () => {
    if (item.kind === "agent") onSelect(item.session.name);
    else if (item.kind === "pr") window.open(item.pr.url, "_blank", "noopener");
    else openCalendar();
  };

  const Icon = meta.icon;
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "flex flex-col gap-1 rounded-md border border-l-2 px-2.5 py-2 text-left hover:bg-accent/50",
        KIND_BORDER[item.kind],
      )}
    >
      <div className="flex items-center gap-1.5 text-[10px] font-medium uppercase tracking-wide text-muted-foreground">
        <Icon className="size-3" />
        {meta.label}
        <span className="ml-auto tabular-nums">{meta.age}</span>
      </div>
      <div className="truncate text-sm font-medium">{meta.title}</div>
      <div className="truncate text-xs text-muted-foreground">{item.status}</div>
    </button>
  );
}
