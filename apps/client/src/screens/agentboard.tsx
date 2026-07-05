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
import {
  agentRollup,
  claudeTitleName,
  isAgent,
  isSoloRepo,
  sessionLabel,
  sessionNeeds,
  sessionStatusText,
  statusColor,
  useAgentboardState,
  type FolderData,
  type RepoData,
  type SessionData,
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

type Selected = { folderDir: string; sessionId: string } | null;

/**
 * Agentboard — the Folder Rail. Left: repos → folders (checkouts) → PTY sessions,
 * with a compact needs-you strip (failing PRs + next meeting) pinned on top.
 * Right: the selected session's terminal, with the folder's sessions as tabs.
 * A session IS a PTY; "agent" (✦) is a badge on a session where Claude is
 * detected running — status is reported, never re-rendered (the real TUI is the
 * PTY). Every opened session's terminal stays mounted (toggled `hidden`) so
 * scrollback survives switching. ⌘D = new session in the folder, ⌘W = close.
 */
export function AgentboardScreen() {
  const state = useAgentboardState();
  const { snapshot } = useStoreSnapshot();
  const { openTab } = useWorkspace();
  const now = Date.now();

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

  // Index every session by id → its folder dir, for cwd + validation.
  const folderOf = useMemo(() => {
    const m = new Map<string, FolderData>();
    for (const r of repos) for (const f of r.folders) for (const s of f.sessions) m.set(s.id, f);
    return m;
  }, [repos]);

  function selectSession(folderDir: string, sessionId: string) {
    cwds.current[sessionId] = folderDir;
    setSelected({ folderDir, sessionId });
    setOpen((prev) => (prev.includes(sessionId) ? prev : [...prev, sessionId]));
  }

  async function newSession(folderDir: string) {
    const rec = await abInvoke<SessionData>("ab_add_session", { dir: folderDir, name: null });
    if (rec) selectSession(folderDir, rec.id);
  }

  async function closeSession(sessionId: string) {
    await abInvoke("ab_close_session", { id: sessionId });
    setOpen((prev) => prev.filter((id) => id !== sessionId));
    setSelected((cur) => (cur?.sessionId === sessionId ? null : cur));
  }

  async function commitRename(sessionId: string, name: string) {
    setRenaming(null);
    const trimmed = name.trim();
    if (trimmed) await abInvoke("ab_rename_session", { id: sessionId, name: trimmed });
  }

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

  const selectedFolder = selected ? folderOf.get(selected.sessionId) : undefined;

  return (
    <div className="flex h-full min-h-0">
      {/* Rail: rollup tally + attention strip + Repo → Folder → Session tree. */}
      <div className="flex w-80 shrink-0 flex-col border-r">
        <RollupChip repos={repos} />
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
                selected={selected}
                collapsed={collapsed}
                renaming={renaming}
                titles={titles}
                onToggle={(k) => setCollapsed((c) => ({ ...c, [k]: !c[k] }))}
                onSelect={selectSession}
                onNewSession={newSession}
                onRenameStart={setRenaming}
                onRenameCommit={commitRename}
              />
            ))}
          </div>
        </ScrollArea>
      </div>

      {/* Terminal area for the selected session. */}
      <div className="flex min-w-0 flex-1 flex-col">
        {selectedFolder && selected && (
          <div className="flex items-center gap-2 border-b bg-card px-2 py-1">
            <span className="shrink-0 truncate text-sm font-medium">
              {selectedFolder.name}
            </span>
            <span className="shrink-0 font-mono text-[11px] text-muted-foreground">
              ⎇ {selectedFolder.branch}
            </span>
            <div className="ml-2 flex min-w-0 flex-1 items-center gap-1 overflow-x-auto">
              {selectedFolder.sessions.map((s) => (
                <button
                  key={s.id}
                  type="button"
                  onClick={() => selectSession(selectedFolder.dir, s.id)}
                  className={cn(
                    "flex shrink-0 items-center gap-1.5 rounded-md px-2 py-1 text-[11px]",
                    s.id === selected.sessionId
                      ? "bg-accent text-foreground"
                      : "text-muted-foreground hover:bg-accent/50",
                  )}
                >
                  <Glyph agent={isAgent(s)} />
                  {labelFor(s)}
                  <Dot session={s} />
                </button>
              ))}
              <button
                type="button"
                onClick={() => void newSession(selectedFolder.dir)}
                className="flex shrink-0 items-center gap-1 rounded-md px-2 py-1 text-[11px] text-violet-500 hover:bg-accent/50"
              >
                <Plus className="size-3" /> session
              </button>
            </div>
            <button
              type="button"
              onClick={() => void closeSession(selected.sessionId)}
              className="shrink-0 rounded-md px-2 py-1 text-[11px] text-muted-foreground hover:bg-accent/50"
              title="Close session (⌘W)"
            >
              Close ⌘W
            </button>
          </div>
        )}

        {/* Every opened session's terminal stays mounted; only the selected shows. */}
        <div className="relative min-h-0 flex-1 p-3.5">
          {open.map((id) => (
            <div
              key={id}
              hidden={selected?.sessionId !== id}
              className="absolute inset-3.5 overflow-hidden rounded-lg border bg-[#07090c]"
            >
              <TerminalView
                termId={id}
                cwd={cwds.current[id]}
                onExit={() => closeSession(id)}
                onTitle={onTitle}
              />
            </div>
          ))}
          {!selected && (
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

/** The board-wide agent tally pinned atop the rail: total + non-zero status
 * buckets. Quiet ("no agents running") when the board is at rest. */
function RollupChip({ repos }: { repos: RepoData[] }) {
  const r = agentRollup(repos);
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
        </>
      )}
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
  selected,
  collapsed,
  renaming,
  titles,
  onToggle,
  onSelect,
  onNewSession,
  onRenameStart,
  onRenameCommit,
}: {
  repo: RepoData;
  selected: Selected;
  collapsed: Record<string, boolean>;
  renaming: string | null;
  titles: Record<string, string>;
  onToggle: (key: string) => void;
  onSelect: (folderDir: string, sessionId: string) => void;
  onNewSession: (folderDir: string) => void;
  onRenameStart: (sessionId: string) => void;
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
          title={titles[s.id]}
          active={selected?.sessionId === s.id}
          renaming={renaming === s.id}
          onSelect={() => onSelect(folder.dir, s.id)}
          onRenameStart={() => onRenameStart(s.id)}
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
  title,
  active,
  renaming,
  onSelect,
  onRenameStart,
  onRenameCommit,
}: {
  session: SessionData;
  title?: string;
  active: boolean;
  renaming: boolean;
  onSelect: () => void;
  onRenameStart: () => void;
  onRenameCommit: (name: string) => void;
}) {
  const needs = sessionNeeds(session);
  // Prefer the live Claude terminal title (`✳ <title>`) when the PTY is open.
  const label = claudeTitleName(title) ?? sessionLabel(session);
  return (
    <div
      role="button"
      tabIndex={0}
      onClick={onSelect}
      onDoubleClick={onRenameStart}
      onKeyDown={(e) => e.key === "Enter" && onSelect()}
      className={cn(
        "ml-1.5 flex cursor-pointer items-center gap-2.5 border-l-2 border-transparent py-1.5 pr-3 pl-7 hover:bg-accent/50",
        active && "border-l-violet-500 bg-accent",
        needs && "border-l-amber-500",
      )}
    >
      <Glyph agent={isAgent(session)} />
      <Dot session={session} />
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
          <span className={cn("truncate", session.live ? "text-foreground" : "text-muted-foreground")}>
            {label}
          </span>
          {label !== session.name && (
            <span className="shrink-0 font-mono text-[10.5px] text-muted-foreground/70">
              {session.name}
            </span>
          )}
          <span className="ml-auto truncate text-[11px] text-muted-foreground">
            {sessionStatusText(session)}
          </span>
          {needs && <span className="size-1.5 shrink-0 rounded-full bg-amber-500" />}
        </>
      )}
    </div>
  );
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
