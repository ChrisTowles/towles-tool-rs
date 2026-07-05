import { useEffect, useMemo, useRef, useState } from "react";
import {
  CalendarClock,
  ChevronDown,
  Folder,
  FolderGit2,
  FolderPlus,
  GitPullRequest,
  MoreVertical,
  Plus,
  TerminalSquare,
  Trash2,
} from "lucide-react";
import { TerminalView } from "@/components/terminal-view";
import { Button } from "@/components/ui/button";
import {
  Command,
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";
import {
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
async function abInvoke<T>(
  cmd: string,
  args: Record<string, unknown>,
): Promise<T | null> {
  if (!("__TAURI_INTERNALS__" in window)) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  try {
    return await invoke<T>(cmd, args);
  } catch {
    return null;
  }
}

type Selected = { folderDir: string; sessionId: string } | null;

/** A discoverable repo for the fuzzy add-repo picker (from `ab_discover_repos`). */
type RepoCandidate = { name: string; dir: string };

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
  // Add-repo picker: discovered repos under ~/code, fuzzy-searched by `repoQuery`.
  const [addRepoOpen, setAddRepoOpen] = useState(false);
  const [repoQuery, setRepoQuery] = useState("");
  const [candidates, setCandidates] = useState<RepoCandidate[]>([]);
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
  const labelFor = (s: SessionData) =>
    claudeTitleName(titles[s.id]) ?? sessionLabel(s);

  const repos = state.repos;

  // Index every session by id → its folder dir, for cwd + validation.
  const folderOf = useMemo(() => {
    const m = new Map<string, FolderData>();
    for (const r of repos)
      for (const f of r.folders) for (const s of f.sessions) m.set(s.id, f);
    return m;
  }, [repos]);

  function selectSession(folderDir: string, sessionId: string) {
    cwds.current[sessionId] = folderDir;
    setSelected({ folderDir, sessionId });
    setOpen((prev) => (prev.includes(sessionId) ? prev : [...prev, sessionId]));
  }

  async function newSession(folderDir: string) {
    const rec = await abInvoke<SessionData>("ab_add_session", {
      dir: folderDir,
      name: null,
    });
    if (rec) selectSession(folderDir, rec.id);
  }

  // Add a repo to the rail; backend re-emits state so it appears. Mirrors
  // `ttr agentboard repos add <path>`.
  async function addRepo(dir: string) {
    const path = dir.trim();
    if (!path) return;
    setAddRepoOpen(false);
    await abInvoke("ab_add_repo", { path });
  }

  // Drop a repo from the watched list (non-destructive — leaves it on disk).
  async function removeRepo(name: string) {
    await abInvoke("ab_remove_repo", { name });
  }

  // On opening the picker, (re)load the discoverable repos under ~/code.
  useEffect(() => {
    if (!addRepoOpen) return;
    setRepoQuery("");
    let active = true;
    void (async () => {
      const found = await abInvoke<RepoCandidate[]>("ab_discover_repos", {});
      if (active) setCandidates(found ?? []);
    })();
    return () => {
      active = false;
    };
  }, [addRepoOpen]);

  async function closeSession(sessionId: string) {
    await abInvoke("ab_close_session", { id: sessionId });
    setOpen((prev) => prev.filter((id) => id !== sessionId));
    setSelected((cur) => (cur?.sessionId === sessionId ? null : cur));
  }

  async function commitRename(sessionId: string, name: string) {
    setRenaming(null);
    const trimmed = name.trim();
    if (trimmed)
      await abInvoke("ab_rename_session", { id: sessionId, name: trimmed });
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
    const items: {
      key: string;
      kind: "pr" | "event";
      title: string;
      sub: string;
      onClick: () => void;
    }[] = [];
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

  const selectedFolder = selected
    ? folderOf.get(selected.sessionId)
    : undefined;

  return (
    <div className="flex h-full min-h-0">
      {/* Rail: header + attention strip + Repo → Folder → Session tree. */}
      <div className="flex w-80 shrink-0 flex-col border-r">
        <div className="flex items-center justify-between border-b px-3 py-2">
          <span className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
            Repos
          </span>
          <button
            type="button"
            onClick={() => setAddRepoOpen(true)}
            className="flex items-center gap-1 rounded-md px-1.5 py-1 text-xs font-medium text-violet-500 hover:bg-accent/50"
            title="Add a repo to the rail"
          >
            <FolderPlus className="size-3.5" /> Add repo
          </button>
        </div>

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
                  <span className="block truncate text-xs font-medium">
                    {a.title}
                  </span>
                  <span className="block truncate text-[11px] text-muted-foreground">
                    {a.sub}
                  </span>
                </span>
              </button>
            ))}
          </div>
        )}

        <ScrollArea className="flex-1">
          <div className="flex flex-col">
            {repos.length === 0 && (
              <div className="flex flex-col items-center gap-3 px-3 py-10 text-center">
                <FolderGit2 className="size-8 text-muted-foreground" />
                <p className="text-sm text-muted-foreground">
                  No repos on the rail yet.
                </p>
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => setAddRepoOpen(true)}
                >
                  <FolderPlus className="size-3.5" /> Add a repo
                </Button>
              </div>
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
                onRemoveRepo={removeRepo}
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

      <CommandDialog
        open={addRepoOpen}
        onOpenChange={setAddRepoOpen}
        title="Add repo"
        description="Fuzzy-search your git repos and add one to the rail."
        className="sm:max-w-2xl"
      >
        <Command>
          <CommandInput
            autoFocus
            value={repoQuery}
            onValueChange={setRepoQuery}
            placeholder="Search your git repos…"
          />
          <CommandList className="max-h-[60vh]">
            <CommandEmpty>
              {candidates.length === 0
                ? "No git repos found. Type an absolute path to add one."
                : "No match. Type an absolute path to add one."}
            </CommandEmpty>
            {candidates.length > 0 && (
              <CommandGroup heading="Discovered repos">
                {candidates.map((c) => (
                  <CommandItem
                    key={c.dir}
                    value={`${c.name} ${c.dir}`}
                    onSelect={() => void addRepo(c.dir)}
                  >
                    <FolderGit2 className="size-3.5 shrink-0 text-muted-foreground" />
                    <span className="flex-1 truncate">{c.name}</span>
                    <span className="truncate font-mono text-[11px] text-muted-foreground">
                      {c.dir}
                    </span>
                  </CommandItem>
                ))}
              </CommandGroup>
            )}
            {repoQuery.startsWith("/") && (
              <CommandGroup heading="Path">
                <CommandItem
                  value={repoQuery}
                  onSelect={() => void addRepo(repoQuery)}
                >
                  <FolderPlus className="size-3.5 shrink-0 text-violet-500" />
                  <span>Add path</span>
                  <span className="truncate font-mono text-[11px] text-muted-foreground">
                    {repoQuery}
                  </span>
                </CommandItem>
              </CommandGroup>
            )}
          </CommandList>
        </Command>
      </CommandDialog>
    </div>
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

/** Status dot mirroring `statusColor`; pulses while busy. */
function Dot({ session }: { session: SessionData }) {
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
  onRemoveRepo,
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
  onRemoveRepo: (name: string) => void;
  onRenameStart: (sessionId: string) => void;
  onRenameCommit: (sessionId: string, name: string) => void;
}) {
  const solo = isSoloRepo(repo);

  const sessionRows = (folder: FolderData) =>
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
    ));

  // Solo repo: collapse repo + folder into one header (repo · branch).
  if (solo) {
    const folder = repo.folders[0];
    const isCollapsed = collapsed[repo.key];
    return (
      <div className="border-b">
        <FolderHeader
          scope="repo"
          title={repo.name}
          branch={folder.branch}
          needs={repo.needs}
          collapsed={isCollapsed}
          onToggle={() => onToggle(repo.key)}
          onNewSession={() => onNewSession(folder.dir)}
          onRemoveRepo={() => onRemoveRepo(repo.name)}
        />
        {!isCollapsed && <div className="pb-2">{sessionRows(folder)}</div>}
      </div>
    );
  }

  // Multi-checkout repo: repo header, then each folder as a sub-header.
  const repoCollapsed = collapsed[repo.key];
  return (
    <div className="border-b">
      <div className="sticky top-0 z-10 flex w-full items-center gap-2 bg-card px-3 py-2 hover:bg-accent/50">
        <button
          type="button"
          onClick={() => onToggle(repo.key)}
          className="flex min-w-0 flex-1 items-center gap-2"
        >
          <Chevron collapsed={repoCollapsed} />
          <FolderGit2 className="size-3.5 shrink-0 text-muted-foreground" />
          <span className="truncate text-sm font-semibold">{repo.name}</span>
          {repo.needs > 0 && <NeedsBadge n={repo.needs} className="ml-auto" />}
        </button>
        <RepoMenu onRemove={() => onRemoveRepo(repo.name)} />
      </div>
      {!repoCollapsed &&
        repo.folders.map((folder) => {
          const key = `${repo.key}::${folder.dir}`;
          const fCollapsed = collapsed[key];
          return (
            <div key={folder.dir}>
              <FolderHeader
                scope="folder"
                title={folder.name}
                branch={folder.branch}
                needs={folder.needs}
                collapsed={fCollapsed}
                onToggle={() => onToggle(key)}
                onNewSession={() => onNewSession(folder.dir)}
              />
              {!fCollapsed && <div className="pb-1">{sessionRows(folder)}</div>}
            </div>
          );
        })}
    </div>
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
  onRemoveRepo,
}: {
  scope: "repo" | "folder";
  title: string;
  branch: string;
  needs: number;
  collapsed: boolean;
  onToggle: () => void;
  onNewSession: () => void;
  onRemoveRepo?: () => void;
}) {
  return (
    <div
      className={cn(
        "flex items-center gap-2 bg-card px-3 py-2 hover:bg-accent/50",
        scope === "repo" ? "sticky top-0 z-10" : "pl-6",
      )}
    >
      <button
        type="button"
        onClick={onToggle}
        className="flex min-w-0 flex-1 items-center gap-2"
      >
        <Chevron collapsed={collapsed} />
        {scope === "repo" ? (
          <FolderGit2 className="size-3.5 shrink-0 text-muted-foreground" />
        ) : (
          <Folder className="size-3.5 shrink-0 text-muted-foreground/70" />
        )}
        <span
          className={cn(
            "truncate",
            scope === "repo"
              ? "text-sm font-semibold"
              : "text-sm text-muted-foreground",
          )}
        >
          {title}
        </span>
        <span className="truncate font-mono text-[11px] text-muted-foreground">
          ⎇ {branch}
        </span>
      </button>
      {needs > 0 && <NeedsBadge n={needs} />}
      <button
        type="button"
        onClick={onNewSession}
        className="shrink-0 rounded p-0.5 text-muted-foreground hover:bg-accent hover:text-violet-500"
        title="New session (⌘D)"
      >
        <Plus className="size-3.5" />
      </button>
      {onRemoveRepo && <RepoMenu onRemove={onRemoveRepo} />}
    </div>
  );
}

/** Kebab menu on a repo header: currently just "Remove from rail". */
function RepoMenu({ onRemove }: { onRemove: () => void }) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        className="shrink-0 rounded p-0.5 text-muted-foreground hover:bg-accent hover:text-foreground"
        title="Repo actions"
      >
        <MoreVertical className="size-3.5" />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuItem variant="destructive" onSelect={onRemove}>
          <Trash2 className="size-3.5" /> Remove from rail
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
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
            if (e.key === "Enter")
              onRenameCommit((e.target as HTMLInputElement).value);
            if (e.key === "Escape") onRenameCommit(session.name);
          }}
          className="min-w-0 flex-1 rounded-sm border border-input bg-background px-1 text-sm outline-none"
        />
      ) : (
        <>
          <span className="truncate text-foreground">{label}</span>
          {label !== session.name && (
            <span className="shrink-0 font-mono text-[10.5px] text-muted-foreground/70">
              {session.name}
            </span>
          )}
          <span className="ml-auto truncate text-[11px] text-muted-foreground">
            {sessionStatusText(session)}
          </span>
          {needs && (
            <span className="size-1.5 shrink-0 rounded-full bg-amber-500" />
          )}
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
