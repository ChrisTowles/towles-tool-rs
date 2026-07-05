import { useEffect, useMemo, useRef, useState } from "react";
import {
  CalendarClock,
  ChevronDown,
  Folder,
  FolderGit2,
  GitPullRequest,
  Plus,
  TerminalSquare,
} from "lucide-react";
import { TerminalView } from "@/components/terminal-view";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Slider } from "@/components/ui/slider";
import {
  agentRollup,
  claudeTitleName,
  ctxPct,
  type AgentStatus,
  isAgent,
  isCold,
  isSoloRepo,
  needsCompact,
  sessionLabel,
  sessionNeeds,
  sessionStatusText,
  statusColor,
  useAgentboardState,
  type FolderData,
  type RepoData,
  type SessionData,
  type StatePayload,
  type WindowsPayload,
  windowColor,
  windowOf,
} from "@/lib/agentboard";
import { fmtCountdown, useStoreSnapshot } from "@/lib/data";
import { useWorkspace } from "@/lib/workspace";

/** Invoke a Tauri `ab_*` command; no-op (null) in bare-browser dev. */
async function abInvoke<T>(cmd: string, args: Record<string, unknown>): Promise<T | null> {
  if (!("__TAURI_INTERNALS__" in window)) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  try {
    return await invoke<T>(cmd, args);
  } catch {
    return null;
  }
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Write raw bytes into a session's PTY. False when the PTY isn't running. */
async function termWrite(termId: string, data: string): Promise<boolean> {
  if (!("__TAURI_INTERNALS__" in window)) return false;
  const { invoke } = await import("@tauri-apps/api/core");
  try {
    await invoke("term_write", { termId, data });
    return true;
  } catch {
    return false;
  }
}

/** Write, retrying while the PTY spawns (a just-mounted terminal takes a beat
 * before `term_start` registers it). Gives up after ~3s. */
async function termWriteRetry(termId: string, data: string): Promise<boolean> {
  for (let i = 0; i < 20; i++) {
    if (await termWrite(termId, data)) return true;
    await sleep(150);
  }
  return false;
}

/** The lifecycle actions a session row can trigger. All are PTY writes — the
 * agent is whatever runs in the real shell, never a re-rendered proxy. */
type SessionActions = {
  /** Mount + spawn the session's shell (no Claude). */
  start: (folderDir: string, s: SessionData) => void;
  /** Ensure the shell is live, then launch Claude in it. */
  startClaude: (folderDir: string, s: SessionData) => void;
  /** Interrupt Claude (Ctrl-C) then exit it (Ctrl-D). The shell survives. */
  stopClaude: (s: SessionData) => void;
  /** Send `/compact` to a Claude sitting at its prompt. */
  compactClaude: (s: SessionData) => void;
  /** Stop Claude, then launch a fresh session in the same shell. */
  restartClaude: (folderDir: string, s: SessionData) => void;
  close: (sessionId: string) => void;
  renameStart: (sessionId: string) => void;
  /** Remove the session's pane from its window (session stays in the rail). */
  ungroup: (sessionId: string) => void;
  /** Focus the window a session's group tag points at. */
  focusWindow: (windowId: string) => void;
};

/** Percent-rect for one pane in the active window's tiling: side-by-side up to
 * three across, a 2-column grid from four panes on. */
type PaneRect = { left: number; top: number; width: number; height: number };

function paneRects(n: number): PaneRect[] {
  if (n <= 0) return [];
  if (n <= 3) {
    const w = 100 / n;
    return Array.from({ length: n }, (_, i) => ({ left: i * w, top: 0, width: w, height: 100 }));
  }
  const rows = Math.ceil(n / 2);
  const h = 100 / rows;
  return Array.from({ length: n }, (_, i) => {
    const lastRowSolo = n % 2 === 1 && i === n - 1;
    return {
      left: lastRowSolo ? 0 : (i % 2) * 50,
      top: Math.floor(i / 2) * h,
      width: lastRowSolo ? 100 : 50,
      height: h,
    };
  });
}

/** Optimistic status shown for ~2.5s after a lifecycle action, until the
 * watcher's ground truth catches up on its next scan. */
type Overlay = { status: AgentStatus; until: number };

type Selected = { folderDir: string; sessionId: string } | null;

/** Guarantee at least one window and a valid `activeWindow`. */
function normalizeWins(w: WindowsPayload): WindowsPayload {
  let windows = w.windows;
  if (windows.length === 0) {
    windows = [{ id: `w${Date.now()}`, name: "main", panes: [] }];
  }
  const active = windows.some((win) => win.id === w.activeWindow)
    ? w.activeWindow
    : windows[0].id;
  return { windows, activeWindow: active };
}

/** Wall clock ticking every `intervalMs` — drives cache-warmth countdowns.
 * 30s granularity keeps the rail calm (badges show minutes, not seconds). */
function useNow(intervalMs: number): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(t);
  }, [intervalMs]);
  return now;
}

/**
 * Agentboard — the Folder Rail. Left: rollup tally + needs-you strip + the
 * repos → folders (checkouts) → PTY sessions tree. Right: in-app *windows* —
 * each a named tiling of session panes (side-by-side up to 3, then a 2-col
 * grid), switched via the window strip. Clicking a rail session opens it as a
 * pane in the active window; the colored square on a row is its window's
 * group tag. A session IS a PTY; "agent" (✦) is a badge on a session where
 * Claude is detected running — status is reported, never re-rendered (the
 * real TUI is the PTY). All opened terminals live in one flat mounted pool
 * (hidden when not in the active window) so scrollback survives switching and
 * regrouping. Layout persists via debounced `ab_save_windows`. ⌘D = new
 * session in the selected folder, ⌘W = close the selected session.
 */
export function AgentboardScreen() {
  const state = useAgentboardState();
  const { snapshot } = useStoreSnapshot();
  const { openTab } = useWorkspace();
  const now = useNow(30_000);

  const [selected, setSelected] = useState<Selected>(null);
  // Session ids whose PTY is mounted (kept alive for scrollback), + their cwd.
  const [open, setOpen] = useState<string[]>([]);
  const cwds = useRef<Record<string, string>>({});
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const [renaming, setRenaming] = useState<string | null>(null);
  // Live PTY window titles keyed by session id (Claude emits `✳ <title>`);
  // preferred over the backend label for sessions whose terminal is open.
  const [titles, setTitles] = useState<Record<string, string>>({});
  const onTitle = (id: string, title: string) =>
    setTitles((m) => (m[id] === title ? m : { ...m, [id]: title }));
  // The label to lead a session row/tab with: the live Claude terminal title
  // when present, else the backend-derived task/shell name.
  const labelFor = (s: SessionData) => claudeTitleName(titles[s.id]) ?? sessionLabel(s);

  const repos = state.repos;

  // Index every session by id → its folder / its data, for cwd + pane chrome.
  const folderOf = useMemo(() => {
    const m = new Map<string, FolderData>();
    for (const r of repos) for (const f of r.folders) for (const s of f.sessions) m.set(s.id, f);
    return m;
  }, [repos]);
  const sessionById = useMemo(() => {
    const m = new Map<string, SessionData>();
    for (const r of repos) for (const f of r.folders) for (const s of f.sessions) m.set(s.id, s);
    return m;
  }, [repos]);

  // --- Window layout (Tier 5): frontend-owned, hydrated once, saved debounced.
  const [wins, setWins] = useState<WindowsPayload | null>(null);
  const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    // Hydrate from the first real payload (mock or ab_get_state); after that
    // the local copy is the live truth and only flows outward.
    if (wins === null && state.ts > 0) setWins(normalizeWins(state.windows));
  }, [wins, state.ts, state.windows]);

  function updateWins(fn: (w: WindowsPayload) => WindowsPayload) {
    setWins((prev) => {
      const next = normalizeWins(fn(prev ?? { windows: [], activeWindow: "" }));
      if (saveTimer.current) clearTimeout(saveTimer.current);
      saveTimer.current = setTimeout(() => {
        void abInvoke("ab_save_windows", { payload: next });
      }, 400);
      return next;
    });
  }

  const activeWin = wins?.windows.find((w) => w.id === wins.activeWindow) ?? wins?.windows[0];

  function addPaneToActive(sessionId: string) {
    updateWins((w) => {
      if (w.windows.some((win) => win.panes.includes(sessionId))) return w;
      return {
        ...w,
        windows: w.windows.map((win) =>
          win.id === w.activeWindow ? { ...win, panes: [...win.panes, sessionId] } : win,
        ),
      };
    });
  }

  function removePane(sessionId: string) {
    updateWins((w) => ({
      ...w,
      windows: w.windows.map((win) => ({
        ...win,
        panes: win.panes.filter((p) => p !== sessionId),
      })),
    }));
  }

  function selectSession(folderDir: string, sessionId: string) {
    cwds.current[sessionId] = folderDir;
    setSelected({ folderDir, sessionId });
    setOpen((prev) => (prev.includes(sessionId) ? prev : [...prev, sessionId]));
    addPaneToActive(sessionId);
  }

  async function newSession(folderDir: string) {
    const rec = await abInvoke<SessionData>("ab_add_session", { dir: folderDir, name: null });
    if (rec) selectSession(folderDir, rec.id);
  }

  async function closeSession(sessionId: string) {
    await abInvoke("ab_close_session", { id: sessionId });
    setOpen((prev) => prev.filter((id) => id !== sessionId));
    setSelected((cur) => (cur?.sessionId === sessionId ? null : cur));
    removePane(sessionId);
  }

  async function commitRename(sessionId: string, name: string) {
    setRenaming(null);
    const trimmed = name.trim();
    if (trimmed) await abInvoke("ab_rename_session", { id: sessionId, name: trimmed });
  }

  // Optimistic lifecycle overlays (sessionId → forced status until ts). The
  // 2s watcher scan re-renders with ground truth; overlays just cover the gap.
  const [overlays, setOverlays] = useState<Record<string, Overlay>>({});
  const setOverlay = (id: string, status: AgentStatus) =>
    setOverlays((m) => ({ ...m, [id]: { status, until: Date.now() + 2_500 } }));

  const actions: SessionActions = {
    start: (folderDir, s) => {
      // Selecting mounts the TerminalView, whose effect spawns the PTY.
      selectSession(folderDir, s.id);
    },
    startClaude: (folderDir, s) => {
      selectSession(folderDir, s.id);
      setOverlay(s.id, "busy");
      void termWriteRetry(s.id, "claude\r");
    },
    stopClaude: (s) => {
      setOverlay(s.id, "interrupted");
      void (async () => {
        await termWrite(s.id, "\x03"); // interrupt the current turn
        await sleep(150);
        await termWrite(s.id, "\x04"); // Ctrl-D at the empty prompt exits Claude
      })();
    },
    compactClaude: (s) => {
      setOverlay(s.id, "busy");
      void termWrite(s.id, "/compact\r");
    },
    restartClaude: (folderDir, s) => {
      selectSession(folderDir, s.id);
      setOverlay(s.id, "busy");
      void (async () => {
        await termWrite(s.id, "\x03");
        await sleep(150);
        await termWrite(s.id, "\x04");
        await sleep(300);
        await termWriteRetry(s.id, "claude\r"); // fresh session (not --continue)
      })();
    },
    close: (sessionId) => void closeSession(sessionId),
    renameStart: setRenaming,
    ungroup: removePane,
    focusWindow: (windowId) => updateWins((w) => ({ ...w, activeWindow: windowId })),
  };

  // ⌘D = new session in the selected folder; ⌘W = close the selected session.
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey) || !selected) return;
      if (e.key === "d") {
        e.preventDefault();
        void newSession(selected.folderDir);
      } else if (e.key === "w") {
        e.preventDefault();
        void closeSession(selected.sessionId);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [selected]);

  // Compact attention strip: failing/review PRs + the next imminent meeting.
  const attention = useMemo(() => {
    const items: { key: string; kind: "pr" | "event"; title: string; sub: string; onClick: () => void }[] =
      [];
    for (const p of snapshot.prs) {
      if (p.checks === "failing" || p.reviewState === "review_requested") {
        items.push({
          key: `pr:${p.repo}#${p.number}`,
          kind: "pr",
          title: `${p.repo.split("/").pop()} #${p.number}`,
          sub: p.checks === "failing" ? "Checks failing" : "Review requested",
          onClick: () => window.open(p.url, "_blank", "noopener"),
        });
      }
    }
    const soon = snapshot.events
      .filter((e) => e.startTs > now && e.startTs - now <= 30 * 60_000)
      .sort((a, b) => a.startTs - b.startTs)[0];
    if (soon) {
      items.push({
        key: `event:${soon.id}`,
        kind: "event",
        title: soon.title,
        sub: `Starts in ${fmtCountdown(soon.startTs - now)}`,
        onClick: () => openTab("cockpit"),
      });
    }
    return items;
  }, [snapshot.prs, snapshot.events, now, openTab]);

  return (
    <div className="flex h-full min-h-0">
      {/* Rail: rollup tally + attention strip + Repo → Folder → Session tree. */}
      <div className="flex w-80 shrink-0 flex-col border-r">
        <RollupChip state={state} now={now} />
        {attention.length > 0 && (
          <div className="flex flex-col gap-1 border-b p-2">
            {attention.map((a) => (
              <button
                key={a.key}
                type="button"
                onClick={a.onClick}
                className={cn(
                  "flex items-center gap-2 rounded-md border border-l-2 px-2 py-1.5 text-left hover:bg-accent/50",
                  a.kind === "pr" ? "border-l-red-500" : "border-l-blue-500",
                )}
              >
                {a.kind === "pr" ? (
                  <GitPullRequest className="size-3.5 shrink-0 text-muted-foreground" />
                ) : (
                  <CalendarClock className="size-3.5 shrink-0 text-muted-foreground" />
                )}
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-xs font-medium">{a.title}</span>
                  <span className="block truncate text-[11px] text-muted-foreground">{a.sub}</span>
                </span>
              </button>
            ))}
          </div>
        )}

        <ScrollArea className="flex-1">
          <div className="flex flex-col">
            {repos.length === 0 && (
              <p className="px-3 py-6 text-center text-sm text-muted-foreground">
                No repos yet. Add one with{" "}
                <span className="font-mono">ttr agentboard repos add</span>.
              </p>
            )}
            {repos.map((repo) => (
              <RepoGroup
                key={repo.key}
                repo={repo}
                now={now}
                compactPct={state.compactRecommendPercent}
                selected={selected}
                collapsed={collapsed}
                renaming={renaming}
                titles={titles}
                overlays={overlays}
                wins={wins}
                actions={actions}
                onToggle={(k) => setCollapsed((c) => ({ ...c, [k]: !c[k] }))}
                onSelect={selectSession}
                onNewSession={newSession}
                onRenameCommit={commitRename}
              />
            ))}
          </div>
        </ScrollArea>
      </div>

      {/* Main area: window strip + the active window's panes tiled side-by-side. */}
      <div className="flex min-w-0 flex-1 flex-col">
        {wins && activeWin && (
          <div className="flex items-center gap-1 border-b bg-card px-2 py-1">
            {wins.windows.map((w) => (
              <button
                key={w.id}
                type="button"
                onClick={() => actions.focusWindow(w.id)}
                className={cn(
                  "flex shrink-0 items-center gap-1.5 rounded-md px-2 py-1 text-[11px]",
                  w.id === activeWin.id
                    ? "bg-accent text-foreground"
                    : "text-muted-foreground hover:bg-accent/50",
                )}
              >
                <span className={cn("size-2 rounded-[3px]", windowColor(wins.windows, w.id))} />
                {w.name}
                <span className="font-mono text-[10px] text-muted-foreground/60">
                  {w.panes.length}⊞
                </span>
                {wins.windows.length > 1 && (
                  <span
                    role="button"
                    title="close window (panes ungroup; sessions stay in the rail)"
                    onClick={(e) => {
                      e.stopPropagation();
                      updateWins((cur) => ({
                        ...cur,
                        windows: cur.windows.filter((x) => x.id !== w.id),
                      }));
                    }}
                    className="text-muted-foreground/50 hover:text-red-500"
                  >
                    ✕
                  </span>
                )}
              </button>
            ))}
            <button
              type="button"
              onClick={() =>
                updateWins((cur) => {
                  const id = `w${Date.now()}`;
                  return {
                    windows: [
                      ...cur.windows,
                      { id, name: `window ${cur.windows.length + 1}`, panes: [] },
                    ],
                    activeWindow: id,
                  };
                })
              }
              className="flex shrink-0 items-center gap-1 rounded-md px-2 py-1 text-[11px] text-violet-500 hover:bg-accent/50"
            >
              <Plus className="size-3" /> window
            </button>
            {selected && (
              <button
                type="button"
                onClick={() => void closeSession(selected.sessionId)}
                className="ml-auto shrink-0 rounded-md px-2 py-1 font-mono text-[10.5px] text-muted-foreground hover:bg-accent/50"
                title="Close session (⌘W)"
              >
                Close ⌘W
              </button>
            )}
          </div>
        )}

        {/* One flat pool of mounted terminals (never remounted — a remount
            would respawn the shell). The active window's pane order assigns
            each a percent-rect; panes in other windows stay hidden. */}
        <div className="relative min-h-0 flex-1 p-2">
          {(() => {
            const panes = activeWin?.panes ?? [];
            const rects = paneRects(panes.length);
            const rectFor = (id: string) => {
              const i = panes.indexOf(id);
              return i < 0 ? undefined : rects[i];
            };
            const paneStyle = (r: PaneRect) => ({
              left: `${r.left}%`,
              top: `${r.top}%`,
              width: `${r.width}%`,
              height: `${r.height}%`,
            });
            return (
              <>
                {open.map((id) => {
                  const r = rectFor(id);
                  const s = sessionById.get(id);
                  return (
                    <div
                      key={id}
                      hidden={!r}
                      style={r ? paneStyle(r) : undefined}
                      className="absolute p-1.5"
                    >
                      <div
                        onClick={() =>
                          selectSession(folderOf.get(id)?.dir ?? cwds.current[id] ?? "", id)
                        }
                        className={cn(
                          "flex h-full flex-col overflow-hidden rounded-lg border bg-[#07090c]",
                          selected?.sessionId === id && "border-violet-500/60",
                        )}
                      >
                        {s && (
                          <PaneHeader
                            session={s}
                            folder={folderOf.get(id)}
                            label={labelFor(s)}
                            now={now}
                            compactPct={state.compactRecommendPercent}
                            onUngroup={() => actions.ungroup(id)}
                          />
                        )}
                        <div className="min-h-0 flex-1">
                          <TerminalView
                            termId={id}
                            cwd={folderOf.get(id)?.dir ?? cwds.current[id]}
                            onExit={() => closeSession(id)}
                            onTitle={onTitle}
                          />
                        </div>
                      </div>
                    </div>
                  );
                })}
                {/* Panes restored from disk but not started this run. */}
                {panes
                  .filter((id) => !open.includes(id))
                  .map((id) => {
                    const r = rectFor(id);
                    const s = sessionById.get(id);
                    const dir = folderOf.get(id)?.dir;
                    return (
                      <div key={id} style={r ? paneStyle(r) : undefined} className="absolute p-1.5">
                        <div className="flex h-full flex-col items-center justify-center gap-2 rounded-lg border border-dashed text-muted-foreground">
                          <span className="text-sm">{s ? labelFor(s) : "session"}</span>
                          {s && dir ? (
                            <div className="flex gap-3 font-mono text-xs">
                              <button
                                type="button"
                                onClick={() => actions.start(dir, s)}
                                className="hover:text-green-500"
                              >
                                ▶ shell
                              </button>
                              <button
                                type="button"
                                onClick={() => actions.startClaude(dir, s)}
                                className="text-violet-500 hover:text-violet-400"
                              >
                                ✦ Claude
                              </button>
                              <button
                                type="button"
                                onClick={() => actions.ungroup(id)}
                                className="hover:text-red-500"
                              >
                                ⊟ remove
                              </button>
                            </div>
                          ) : (
                            <button
                              type="button"
                              onClick={() => actions.ungroup(id)}
                              className="font-mono text-xs hover:text-red-500"
                            >
                              session gone — ⊟ remove pane
                            </button>
                          )}
                        </div>
                      </div>
                    );
                  })}
                {panes.length === 0 && (
                  <div className="flex h-full flex-col items-center justify-center gap-2 text-muted-foreground">
                    <TerminalSquare className="size-10" />
                    <p className="text-sm">
                      Empty window — click a session in the rail to open it here.
                    </p>
                  </div>
                )}
              </>
            );
          })()}
        </div>
      </div>
    </div>
  );
}

/** One pane's chrome: glyph · dot · name · folder⎇branch · cache badge · ⊟. */
function PaneHeader({
  session,
  folder,
  label,
  now,
  compactPct,
  onUngroup,
}: {
  session: SessionData;
  folder?: FolderData;
  label: string;
  now: number;
  compactPct: number;
  onUngroup: () => void;
}) {
  return (
    <div className="flex shrink-0 items-center gap-2 border-b bg-card px-2 py-1">
      <Glyph agent={isAgent(session)} />
      <Dot session={session} />
      <span className="truncate text-xs text-foreground">{label}</span>
      {folder && (
        <span className="truncate font-mono text-[10px] text-muted-foreground">
          {folder.name} ⎇ {folder.branch}
        </span>
      )}
      <span className="ml-auto flex shrink-0 items-center gap-2">
        <CacheBadge session={session} now={now} compactPct={compactPct} />
        <button
          type="button"
          title="remove pane (session stays in the rail)"
          onClick={(e) => {
            e.stopPropagation();
            onUngroup();
          }}
          className="font-mono text-xs text-muted-foreground/60 hover:text-red-500"
        >
          ⊟
        </button>
      </span>
    </div>
  );
}

/** The board-wide agent tally pinned atop the rail: total + non-zero status
 * buckets + a ❄ compact count, with the Agentboard settings (compact
 * threshold) behind the trailing ⚙. Quiet when the board is at rest. */
function RollupChip({ state, now }: { state: StatePayload; now: number }) {
  const threshold = state.compactRecommendPercent;
  const r = agentRollup(state.repos, now, threshold);
  // Track the slider locally while dragging; commit on release.
  const [draft, setDraft] = useState<number | null>(null);
  const pct = draft ?? threshold;

  return (
    <div className="flex items-center gap-2.5 border-b bg-card px-3 py-2 font-mono text-[11px]">
      {r.total === 0 ? (
        <span className="text-muted-foreground/60">no agents running</span>
      ) : (
        <>
          <span className="text-foreground">
            {r.total} agent{r.total !== 1 && "s"}
          </span>
          {r.busy > 0 && <RollupBucket className="bg-yellow-500" n={r.busy} />}
          {r.waiting > 0 && <RollupBucket className="bg-blue-500" n={r.waiting} />}
          {r.error > 0 && <RollupBucket className="bg-red-500" n={r.error} />}
          {r.compact > 0 && (
            <span className="text-sky-500" title="cold sessions worth compacting">
              ❄{r.compact}
            </span>
          )}
        </>
      )}
      <Popover>
        <PopoverTrigger asChild>
          <button
            type="button"
            title="Agentboard settings"
            className="ml-auto text-muted-foreground/60 hover:text-foreground"
          >
            ⚙
          </button>
        </PopoverTrigger>
        <PopoverContent align="end" className="w-72">
          <div className="flex flex-col gap-3">
            <div className="text-sm font-medium">Agentboard settings</div>
            <div className="text-xs text-muted-foreground">
              Recommend compacting a cold session at or above{" "}
              <span className="font-mono text-sky-500">{pct}%</span> context.
            </div>
            <Slider
              min={10}
              max={90}
              step={5}
              value={[pct]}
              onValueChange={([v]) => setDraft(v)}
              onValueCommit={([v]) => {
                setDraft(null);
                void abInvoke("ab_set_compact_percent", { percent: v });
              }}
            />
            <div className="text-[11px] text-muted-foreground/70">
              Past this threshold, a session whose prompt cache expired shows the ❄ compact
              nudge. Stored in the shared towles-tool settings file.
            </div>
          </div>
        </PopoverContent>
      </Popover>
    </div>
  );
}

function RollupBucket({ className, n }: { className: string; n: number }) {
  return (
    <span className="flex items-center gap-1 text-muted-foreground">
      <span className={cn("size-1.5 rounded-full", className)} />
      {n}
    </span>
  );
}

/** ✦ for an agent session, ❯ for a plain shell. */
function Glyph({ agent }: { agent: boolean }) {
  return (
    <span
      className={cn(
        "w-4 shrink-0 text-center font-mono text-xs",
        agent ? "text-violet-500" : "text-muted-foreground",
      )}
    >
      {agent ? "✦" : "❯"}
    </span>
  );
}

/** Status dot mirroring `statusColor`; pulses while busy. A session with no
 * live PTY shows a hollow ring — the record exists but nothing is running. */
function Dot({ session }: { session: SessionData }) {
  if (!session.live) {
    return (
      <span className="size-2 shrink-0 rounded-full border-[1.5px] border-muted-foreground/50 bg-transparent" />
    );
  }
  const st = session.agentState?.status;
  return (
    <span
      className={cn(
        "size-2 shrink-0 rounded-full",
        st ? statusColor(st) : "bg-muted-foreground/40",
        st === "busy" && "animate-pulse",
      )}
    />
  );
}

function RepoGroup({
  repo,
  now,
  compactPct,
  selected,
  collapsed,
  renaming,
  titles,
  overlays,
  wins,
  actions,
  onToggle,
  onSelect,
  onNewSession,
  onRenameCommit,
}: {
  repo: RepoData;
  now: number;
  compactPct: number;
  selected: Selected;
  collapsed: Record<string, boolean>;
  renaming: string | null;
  titles: Record<string, string>;
  overlays: Record<string, Overlay>;
  wins: WindowsPayload | null;
  actions: SessionActions;
  onToggle: (key: string) => void;
  onSelect: (folderDir: string, sessionId: string) => void;
  onNewSession: (folderDir: string) => void;
  onRenameCommit: (sessionId: string, name: string) => void;
}) {
  const solo = isSoloRepo(repo);

  const sessionRows = (folder: FolderData) =>
    folder.sessions.length === 0 ? (
      <div className="flex items-center gap-2 py-1.5 pr-3 pl-9 text-[11px] italic text-muted-foreground/60">
        no sessions
        <button
          type="button"
          onClick={() => onNewSession(folder.dir)}
          className="not-italic text-violet-500 hover:underline"
        >
          + session
        </button>
      </div>
    ) : (
      folder.sessions.map((s) => (
        <SessionRow
          key={s.id}
          session={s}
          folderDir={folder.dir}
          now={now}
          compactPct={compactPct}
          title={titles[s.id]}
          active={selected?.sessionId === s.id}
          renaming={renaming === s.id}
          overlay={overlays[s.id]}
          wins={wins}
          actions={actions}
          onSelect={() => onSelect(folder.dir, s.id)}
          onRenameCommit={(name) => onRenameCommit(s.id, name)}
        />
      ))
    );

  // Solo repo: collapse repo + folder into one header (repo · branch).
  if (solo) {
    const folder = repo.folders[0];
    const isCollapsed = collapsed[repo.key];
    return (
      <div className="group/folder border-b">
        <FolderHeader
          scope="repo"
          title={repo.name}
          branch={folder.branch}
          needs={repo.needs}
          collapsed={isCollapsed}
          onToggle={() => onToggle(repo.key)}
          onNewSession={() => onNewSession(folder.dir)}
        />
        {!isCollapsed && (
          <div className="pb-2">
            <PurposeRow folder={folder} />
            {sessionRows(folder)}
          </div>
        )}
      </div>
    );
  }

  // Multi-checkout repo: repo header, then each folder as a sub-header.
  const repoCollapsed = collapsed[repo.key];
  return (
    <div className="border-b">
      <button
        type="button"
        onClick={() => onToggle(repo.key)}
        className="sticky top-0 z-10 flex w-full items-center gap-2 bg-card px-3 py-2 hover:bg-accent/50"
      >
        <Chevron collapsed={repoCollapsed} />
        <FolderGit2 className="size-3.5 shrink-0 text-muted-foreground" />
        <span className="truncate text-sm font-semibold">{repo.name}</span>
        {repo.needs > 0 && <NeedsBadge n={repo.needs} className="ml-auto" />}
      </button>
      {!repoCollapsed &&
        repo.folders.map((folder) => {
          const key = `${repo.key}::${folder.dir}`;
          const fCollapsed = collapsed[key];
          return (
            <div key={folder.dir} className="group/folder">
              <FolderHeader
                scope="folder"
                title={folder.name}
                branch={folder.branch}
                needs={folder.needs}
                collapsed={fCollapsed}
                onToggle={() => onToggle(key)}
                onNewSession={() => onNewSession(folder.dir)}
              />
              {!fCollapsed && (
                <div className="pb-1">
                  <PurposeRow folder={folder} />
                  {sessionRows(folder)}
                </div>
              )}
            </div>
          );
        })}
    </div>
  );
}

/** The folder's user-authored purpose: a faint one-liner under the header.
 * Click to edit inline (Enter saves, Esc cancels; blank clears). When unset,
 * a "+ purpose" hint appears only while hovering the folder group, so a
 * resting rail stays quiet. */
function PurposeRow({ folder }: { folder: FolderData }) {
  const [editing, setEditing] = useState(false);
  const purpose = folder.purpose?.trim() ?? "";

  async function commit(text: string) {
    setEditing(false);
    const trimmed = text.trim();
    if (trimmed === purpose) return;
    await abInvoke("ab_set_folder_purpose", { dir: folder.dir, text: trimmed || null });
  }

  if (editing) {
    return (
      <div className="py-0.5 pr-3 pl-9">
        <input
          autoFocus
          defaultValue={purpose}
          placeholder="what are you working toward here?"
          onBlur={(e) => void commit(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void commit((e.target as HTMLInputElement).value);
            if (e.key === "Escape") setEditing(false);
          }}
          className="w-full rounded-sm border border-input bg-background px-1.5 py-0.5 text-[11px] outline-none"
        />
      </div>
    );
  }

  return (
    <button
      type="button"
      onClick={() => setEditing(true)}
      title="Edit folder purpose"
      className={cn(
        "block w-full truncate py-0.5 pr-3 pl-9 text-left text-[11px]",
        purpose
          ? "text-muted-foreground hover:text-foreground"
          : "text-transparent group-hover/folder:text-muted-foreground/50",
      )}
    >
      {purpose || "+ what are you working toward here?"}
    </button>
  );
}

function FolderHeader({
  scope,
  title,
  branch,
  needs,
  collapsed,
  onToggle,
  onNewSession,
}: {
  scope: "repo" | "folder";
  title: string;
  branch: string;
  needs: number;
  collapsed: boolean;
  onToggle: () => void;
  onNewSession: () => void;
}) {
  return (
    <div
      className={cn(
        "group flex items-center gap-2 bg-card px-3 py-2 hover:bg-accent/50",
        scope === "repo" ? "sticky top-0 z-10" : "pl-6",
      )}
    >
      <button type="button" onClick={onToggle} className="flex min-w-0 flex-1 items-center gap-2">
        <Chevron collapsed={collapsed} />
        {scope === "repo" ? (
          <FolderGit2 className="size-3.5 shrink-0 text-muted-foreground" />
        ) : (
          <Folder className="size-3.5 shrink-0 text-muted-foreground/70" />
        )}
        <span
          className={cn(
            "truncate",
            scope === "repo" ? "text-sm font-semibold" : "text-sm text-muted-foreground",
          )}
        >
          {title}
        </span>
        <span className="truncate font-mono text-[11px] text-muted-foreground">⎇ {branch}</span>
      </button>
      {needs > 0 && <NeedsBadge n={needs} />}
      <button
        type="button"
        onClick={onNewSession}
        className="shrink-0 rounded p-0.5 text-muted-foreground opacity-0 hover:text-violet-500 group-hover:opacity-100"
        title="New session"
      >
        <Plus className="size-3.5" />
      </button>
    </div>
  );
}

function SessionRow({
  session,
  folderDir,
  now,
  compactPct,
  title,
  active,
  renaming,
  overlay,
  wins,
  actions,
  onSelect,
  onRenameCommit,
}: {
  session: SessionData;
  folderDir: string;
  now: number;
  compactPct: number;
  title?: string;
  active: boolean;
  renaming: boolean;
  overlay?: Overlay;
  wins: WindowsPayload | null;
  actions: SessionActions;
  onSelect: () => void;
  onRenameCommit: (name: string) => void;
}) {
  // Apply the optimistic lifecycle overlay (start/stop just happened) until
  // the watcher's next scan delivers ground truth.
  const eff: SessionData =
    overlay && overlay.until > Date.now()
      ? {
          ...session,
          live: true,
          agentState: {
            agent: "claude-code",
            session: "",
            ts: now,
            ...session.agentState,
            status: overlay.status,
          },
        }
      : session;
  const needs = sessionNeeds(eff);
  const agent = isAgent(eff);
  const grouped = wins ? windowOf(wins.windows, session.id) : undefined;
  // Prefer the live Claude terminal title (`✳ <title>`) when the PTY is open.
  const label = claudeTitleName(title) ?? sessionLabel(eff);
  return (
    <div
      role="button"
      tabIndex={0}
      onClick={onSelect}
      onDoubleClick={() => actions.renameStart(session.id)}
      onKeyDown={(e) => e.key === "Enter" && onSelect()}
      className={cn(
        "group/row ml-1.5 flex cursor-pointer items-center gap-2.5 border-l-2 border-transparent py-1.5 pr-3 pl-7 hover:bg-accent/50",
        active && "border-l-violet-500 bg-accent",
        needs && "border-l-amber-500",
      )}
    >
      <Glyph agent={agent} />
      <Dot session={eff} />
      {renaming ? (
        <input
          autoFocus
          defaultValue={session.name}
          onClick={(e) => e.stopPropagation()}
          onBlur={(e) => onRenameCommit(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") onRenameCommit((e.target as HTMLInputElement).value);
            if (e.key === "Escape") onRenameCommit(session.name);
          }}
          className="min-w-0 flex-1 rounded-sm border border-input bg-background px-1 text-sm outline-none"
        />
      ) : (
        <>
          <span className={cn("truncate", eff.live ? "text-foreground" : "text-muted-foreground")}>
            {label}
          </span>
          {label !== session.name && (
            <span className="shrink-0 font-mono text-[10.5px] text-muted-foreground/70">
              {session.name}
            </span>
          )}
          {grouped && (
            <span
              role="button"
              title={`in window “${grouped.name}” — click to focus it`}
              onClick={(e) => {
                e.stopPropagation();
                actions.focusWindow(grouped.id);
              }}
              className={cn(
                "size-2 shrink-0 rounded-[3px]",
                windowColor(wins?.windows ?? [], grouped.id),
              )}
            />
          )}
          {/* Resting: cache + status. Hover: the lifecycle controls. */}
          <span className="ml-auto flex shrink-0 items-center gap-2 group-hover/row:hidden">
            <CacheBadge session={eff} now={now} compactPct={compactPct} />
            <span className="truncate text-[11px] text-muted-foreground">
              {sessionStatusText(eff)}
            </span>
          </span>
          <span className="ml-auto hidden shrink-0 items-center gap-2 group-hover/row:flex">
            <RowControls session={eff} folderDir={folderDir} grouped={!!grouped} actions={actions} />
          </span>
          {needs && <span className="size-1.5 shrink-0 rounded-full bg-amber-500" />}
        </>
      )}
    </div>
  );
}

/** Hover-reveal lifecycle controls for a session row. Which buttons show
 * depends on the session's state:
 *   not started → ▶ shell · ✦ Claude
 *   live shell  → ✦ Claude
 *   live agent  → ■ stop · ⤿ compact (at prompt) · ↻ restart
 * plus ✎ rename and ✕ close, always. */
function RowControls({
  session,
  folderDir,
  grouped,
  actions,
}: {
  session: SessionData;
  folderDir: string;
  grouped: boolean;
  actions: SessionActions;
}) {
  const agent = isAgent(session);
  const st = session.agentState?.status;
  // `/compact` only lands when Claude is at its prompt, not mid-turn.
  const atPrompt = st === "waiting" || st === "idle" || st === "complete";
  const btn = (
    label: string,
    title: string,
    onClick: () => void,
    className = "text-muted-foreground hover:text-foreground",
  ) => (
    <button
      type="button"
      title={title}
      onClick={(e) => {
        e.stopPropagation();
        onClick();
      }}
      className={cn("w-4 text-center font-mono text-xs", className)}
    >
      {label}
    </button>
  );

  return (
    <>
      {!session.live && btn("▶", "start shell", () => actions.start(folderDir, session), "text-muted-foreground hover:text-green-500")}
      {(!session.live || !agent) &&
        btn("✦", "start Claude here", () => actions.startClaude(folderDir, session), "text-violet-500 hover:text-violet-400")}
      {session.live && agent && (
        <>
          {btn("■", "stop Claude (shell survives)", () => actions.stopClaude(session), "text-muted-foreground hover:text-red-500")}
          {atPrompt && btn("⤿", "compact context (/compact)", () => actions.compactClaude(session), "text-muted-foreground hover:text-sky-500")}
          {btn("↻", "start over — fresh Claude session", () => actions.restartClaude(folderDir, session), "text-muted-foreground hover:text-orange-500")}
        </>
      )}
      {grouped &&
        btn("⊟", "ungroup — remove pane from its window", () => actions.ungroup(session.id), "text-muted-foreground hover:text-sky-500")}
      {btn("✎", "rename", () => actions.renameStart(session.id))}
      {btn("✕", "close session", () => actions.close(session.id), "text-muted-foreground hover:text-red-500")}
    </>
  );
}

/** Context/cache health for a live agent session, in the row's meta cluster.
 * Quiet mono text: `41% ◔4m` while warm (⧗ for a 1h cache), `41% ❄` when cold,
 * and an ice-washed `❄ 63% compact` pill when cold at/over the threshold. */
function CacheBadge({
  session,
  now,
  compactPct,
}: {
  session: SessionData;
  now: number;
  compactPct: number;
}) {
  const d = session.agentState?.details;
  if (!session.live || !d?.contextUsed || !d.contextMax) return null;
  const pct = ctxPct(d);
  const cold = isCold(d, now);

  if (needsCompact(d, now, compactPct)) {
    return (
      <span
        title={`${pct}% of context used and the prompt cache expired — resuming re-reads everything. Consider /compact or a fresh session.`}
        className="shrink-0 rounded-md border border-sky-500/50 bg-sky-500/10 px-1.5 font-mono text-[10.5px] text-sky-500"
      >
        ❄ {pct}% compact
      </span>
    );
  }

  const warmth = cold ? "❄" : `${d.cacheTtlMs === 3_600_000 ? "⧗" : "◔"}${fmtMins(d.cacheExpiresAt! - now)}`;
  return (
    <span
      title={cold ? "prompt cache expired" : "prompt cache warm — time left"}
      className="shrink-0 font-mono text-[10.5px] text-muted-foreground/70"
    >
      {pct}% {warmth}
    </span>
  );
}

/** Millis → whole minutes for the cache countdown, floored at 1 ("<1m" ≈ 1m). */
function fmtMins(ms: number): string {
  return `${Math.max(1, Math.round(ms / 60_000))}m`;
}

function NeedsBadge({ n, className }: { n: number; className?: string }) {
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

function Chevron({ collapsed }: { collapsed: boolean }) {
  return (
    <ChevronDown
      className={cn(
        "size-3.5 shrink-0 text-muted-foreground transition-transform",
        collapsed && "-rotate-90",
      )}
    />
  );
}
