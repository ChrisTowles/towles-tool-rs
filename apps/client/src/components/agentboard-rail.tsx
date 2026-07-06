import { useState } from "react";
import {
  CircleDot,
  Folder,
  FolderGit2,
  MoreVertical,
  Plus,
  Trash2,
} from "lucide-react";
import {
  CacheBadge,
  Chevron,
  DiffButton,
  Dot,
  Glyph,
  IconBtn,
  NeedsBadge,
  PrChip,
  WorktreeBadge,
} from "@/components/agentboard-bits";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Slider } from "@/components/ui/slider";
import { cn } from "@/lib/utils";
import {
  abCreateIssue,
  abInvoke,
  agentRollup,
  claudeTitleName,
  fmtElapsed,
  isAgent,
  isSoloRepo,
  pathScope,
  prForFolder,
  sessionCatchesEye,
  sessionLabel,
  sessionStatusText,
  windowColor,
  windowOf,
  type FolderData,
  type Overlay,
  type RepoData,
  type Selected,
  type SessionActions,
  type SessionData,
  type StatePayload,
  type WindowsPayload,
} from "@/lib/agentboard";
import { toast } from "sonner";
import type { PrItem } from "@/lib/data";
import { openExternalUrl } from "@/lib/open-url";

/** The board-wide agent tally pinned atop the rail: total + non-zero status
 * buckets + a ❄ compact count, with the Agentboard settings (compact
 * threshold) behind the trailing ⚙. Quiet when the board is at rest. */
export function RollupChip({ state, now }: { state: StatePayload; now: number }) {
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

export function RepoGroup({
  repo,
  now,
  compactPct,
  prs,
  selected,
  activeFolderDir,
  collapsed,
  renaming,
  titles,
  overlays,
  wins,
  actions,
  onToggle,
  onSelectFolder,
  onSelect,
  onNewSession,
  onRemoveRepo,
  onRenameCommit,
  onOpenDiff,
}: {
  repo: RepoData;
  now: number;
  compactPct: number;
  prs: PrItem[];
  selected: Selected;
  activeFolderDir: string | null;
  collapsed: Record<string, boolean>;
  renaming: string | null;
  titles: Record<string, string>;
  overlays: Record<string, Overlay>;
  wins: WindowsPayload | null;
  actions: SessionActions;
  onToggle: (key: string) => void;
  onSelectFolder: (folderDir: string) => void;
  onSelect: (folderDir: string, sessionId: string) => void;
  onNewSession: (folderDir: string, launchClaude?: boolean) => void;
  onRemoveRepo: (dirs: string[], label: string) => void;
  onRenameCommit: (sessionId: string, name: string) => void;
  onOpenDiff: (dir: string, name: string) => void;
}) {
  const solo = isSoloRepo(repo);

  const sessionRows = (folder: FolderData) =>
    folder.sessions.length === 0 ? (
      <div className="flex items-center gap-2.5 py-1 pr-3 pl-9 text-[11px] italic text-muted-foreground/60">
        no sessions
        <button
          type="button"
          onClick={() => onNewSession(folder.dir, true)}
          className="not-italic text-violet-500 hover:underline"
        >
          ✦ start Claude
        </button>
        <span className="text-muted-foreground/40">·</span>
        <button
          type="button"
          onClick={() => onNewSession(folder.dir, false)}
          className="not-italic text-violet-500 hover:underline"
        >
          + shell
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
      <div className="border-b">
        <FolderHeader
          scope="repo"
          title={repo.name}
          path={folder.dir}
          branch={folder.branch}
          isWorktree={folder.isWorktree}
          filesChanged={folder.filesChanged}
          linesAdded={folder.linesAdded}
          linesRemoved={folder.linesRemoved}
          commitsDelta={folder.commitsDelta}
          progressPercent={folder.metadata?.progress?.percent}
          needs={repo.needs}
          pr={prForFolder(prs, repo.originUrl, folder.branch)}
          collapsed={isCollapsed}
          active={activeFolderDir === folder.dir}
          onToggle={() => {
            onToggle(repo.key);
            onSelectFolder(folder.dir);
          }}
          onNewSession={() => onNewSession(folder.dir)}
          onRemoveRepo={() => onRemoveRepo([folder.dir], repo.name)}
          onOpenDiff={() => onOpenDiff(folder.dir, folder.name)}
          dir={folder.dir}
        />
        {!isCollapsed && (
          <div className="group pb-2">
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
      <div className="sticky top-0 z-10 flex w-full items-center gap-2 border-b border-l-2 border-border border-l-transparent bg-card px-3 py-2 hover:bg-accent/50">
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
        <RepoMenu
          onRemove={() =>
            onRemoveRepo(
              repo.folders.map((f) => f.dir),
              repo.name,
            )
          }
          dir={repo.folders[0].dir}
        />
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
                path={folder.dir}
                branch={folder.branch}
                isWorktree={folder.isWorktree}
                filesChanged={folder.filesChanged}
                linesAdded={folder.linesAdded}
                linesRemoved={folder.linesRemoved}
                commitsDelta={folder.commitsDelta}
                progressPercent={folder.metadata?.progress?.percent}
                needs={folder.needs}
                pr={prForFolder(prs, repo.originUrl, folder.branch)}
                collapsed={fCollapsed}
                active={activeFolderDir === folder.dir}
                onToggle={() => {
                  onToggle(key);
                  onSelectFolder(folder.dir);
                }}
                onNewSession={() => onNewSession(folder.dir)}
                onRemoveRepo={() => onRemoveRepo([folder.dir], folder.name)}
                onOpenDiff={() => onOpenDiff(folder.dir, folder.name)}
                dir={folder.dir}
              />
              {!fCollapsed && (
                <div className="group pb-1">
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

/** The folder's user-authored purpose — the "why am I here". Click to edit
 * inline (Enter saves, Esc cancels; blank clears).
 *
 * `rail` variant: a faint one-liner under the folder header; when unset it
 * takes up no space at rest (the "+ purpose" hint only appears while hovering
 * the folder group), so a resting rail doesn't pad itself with blank lines.
 * `band` variant: lives in the working-context band — always visible, unset
 * state included, because the band exists to answer "where am I and why". */
export function PurposeRow({
  folder,
  variant = "rail",
}: {
  folder: FolderData;
  variant?: "rail" | "band";
}) {
  const [editing, setEditing] = useState(false);
  const purpose = folder.purpose?.trim() ?? "";
  const rail = variant === "rail";
  const pad = rail ? "py-0.5 pr-3 pl-9 text-[11px]" : "text-xs";

  async function commit(text: string) {
    setEditing(false);
    const trimmed = text.trim();
    if (trimmed === purpose) return;
    await abInvoke("ab_set_folder_purpose", { dir: folder.dir, text: trimmed || null });
  }

  if (editing) {
    return (
      <div className={cn(rail && "py-0.5 pr-3 pl-9")}>
        <input
          autoFocus
          defaultValue={purpose}
          placeholder="what are you working toward here?"
          onBlur={(e) => void commit(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void commit((e.target as HTMLInputElement).value);
            if (e.key === "Escape") setEditing(false);
          }}
          className={cn(
            "w-full rounded-sm border border-input bg-background px-1.5 py-0.5 outline-none",
            rail ? "text-[11px]" : "text-xs",
          )}
        />
      </div>
    );
  }

  if (!purpose) {
    return (
      <button
        type="button"
        onClick={() => setEditing(true)}
        title="Edit folder purpose"
        className={cn(
          "w-full truncate text-left text-muted-foreground/50",
          pad,
          rail ? "hidden group-hover:block" : "block hover:text-muted-foreground",
        )}
      >
        + what are you working toward here?
      </button>
    );
  }

  return (
    <button
      type="button"
      onClick={() => setEditing(true)}
      title="Edit folder purpose"
      className={cn(
        "block w-full truncate text-left text-muted-foreground hover:text-foreground",
        pad,
      )}
    >
      {purpose}
    </button>
  );
}

function FolderHeader({
  scope,
  title,
  path,
  branch,
  isWorktree,
  filesChanged,
  linesAdded,
  linesRemoved,
  commitsDelta,
  progressPercent,
  needs,
  pr,
  collapsed,
  active,
  onToggle,
  onNewSession,
  onRemoveRepo,
  onOpenDiff,
  dir,
}: {
  scope: "repo" | "folder";
  title: string;
  /** Full path on disk, shown in the kebab menu. */
  path: string;
  branch: string;
  isWorktree: boolean;
  filesChanged: number;
  linesAdded: number;
  linesRemoved: number;
  commitsDelta: number;
  progressPercent?: number | null;
  needs: number;
  /** The open PR for this folder's branch, when the store knows of one. */
  pr?: PrItem;
  collapsed: boolean;
  /** Whether this folder is the one currently shown in the main pane area. */
  active: boolean;
  onToggle: () => void;
  onNewSession: () => void;
  onRemoveRepo?: () => void;
  /** Opens the full-diff preview dialog for this folder. */
  onOpenDiff: () => void;
  /** A checkout dir for this repo, used to create a GitHub issue via `gh`. */
  dir: string;
}) {
  const scopePrefix = pathScope(path);
  return (
    // Two lines: name (line 1) with the git facts — branch, worktree, diff,
    // PR — grouped underneath (line 2), so a long branch never squeezes the
    // name and all git info reads as one cluster. The transparent border-l-2
    // is always present so the active violet edge never shifts content.
    <div
      className={cn(
        "border-b border-l-2 border-border border-l-transparent bg-card pr-2 hover:bg-accent/50",
        scope === "repo" ? "sticky top-0 z-10 pl-3" : "pl-6",
        active && "border-l-violet-500 bg-accent/60",
      )}
    >
      <div className="flex items-center gap-2 pt-1.5">
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
          {scopePrefix && (
            <span className="shrink-0 font-mono text-sm text-muted-foreground/60">
              {scopePrefix}
            </span>
          )}
          <span
            className={cn(
              "min-w-0 truncate",
              scope === "repo"
                ? "text-sm font-semibold"
                : "text-sm font-medium text-muted-foreground",
            )}
          >
            {title}
          </span>
        </button>
        {needs > 0 && <NeedsBadge n={needs} />}
        <IconBtn title="New session (⌘D)" onClick={onNewSession} className="hover:text-violet-500">
          <Plus className="size-3.5" />
        </IconBtn>
        {onRemoveRepo && <RepoMenu path={path} onRemove={onRemoveRepo} dir={dir} />}
      </div>
      {/* ml-11 lines the git row up under the name (chevron + icon + gaps). */}
      <div className="ml-11 flex items-center gap-1.5 pb-1.5">
        <span
          className="min-w-0 truncate font-mono text-[11px] text-muted-foreground"
          onClick={onToggle}
        >
          ⎇ {branch}
        </span>
        {isWorktree && <WorktreeBadge />}
        <DiffButton
          filesChanged={filesChanged}
          linesAdded={linesAdded}
          linesRemoved={linesRemoved}
          commitsDelta={commitsDelta}
          onOpen={onOpenDiff}
        />
        {pr && <PrChip pr={pr} />}
        {typeof progressPercent === "number" && (
          <span className="shrink-0 rounded-md border border-violet-500/40 bg-violet-500/10 px-1.5 font-mono text-[10.5px] text-violet-500">
            {Math.round(progressPercent)}%
          </span>
        )}
      </div>
    </div>
  );
}

/** Kebab menu on a repo/folder header: shows the full folder path (when
 * given), "Create issue…" (shells `gh issue create` in `dir`), and "Remove
 * from rail". */
function RepoMenu({
  path,
  onRemove,
  dir,
}: {
  path?: string;
  onRemove: () => void;
  dir: string;
}) {
  const [issueOpen, setIssueOpen] = useState(false);
  const [issueTitle, setIssueTitle] = useState("");

  async function createIssue() {
    const title = issueTitle.trim();
    if (!title) return;
    setIssueOpen(false);
    setIssueTitle("");
    try {
      const url = await abCreateIssue(dir, title);
      toast.success("Issue created", {
        action: { label: "Open", onClick: () => void openExternalUrl(url) },
      });
    } catch (e) {
      toast.error(String(e));
    }
  }

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant="outline"
            size="icon-xs"
            title="Repo actions"
            className="text-muted-foreground"
          >
            <MoreVertical className="size-3.5" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-auto min-w-56">
          {path && (
            <>
              <DropdownMenuLabel className="font-mono text-[11px] font-normal whitespace-nowrap text-muted-foreground">
                {path}
              </DropdownMenuLabel>
              <DropdownMenuSeparator />
            </>
          )}
          <DropdownMenuItem onSelect={() => setIssueOpen(true)} className="whitespace-nowrap">
            <CircleDot className="size-3.5" /> Create issue…
          </DropdownMenuItem>
          <DropdownMenuItem
            variant="destructive"
            onSelect={onRemove}
            className="whitespace-nowrap"
          >
            <Trash2 className="size-3.5" /> Remove from rail
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <Dialog open={issueOpen} onOpenChange={setIssueOpen}>
        <DialogContent showCloseButton={false}>
          <DialogHeader>
            <DialogTitle>New issue</DialogTitle>
          </DialogHeader>
          <Input
            autoFocus
            value={issueTitle}
            onChange={(e) => setIssueTitle(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") void createIssue();
            }}
            placeholder="Issue title…"
          />
        </DialogContent>
      </Dialog>
    </>
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
  const needs = sessionCatchesEye(eff);
  const agent = isAgent(eff);
  const grouped = wins ? windowOf(wins.windows, session.id) : undefined;
  // Prefer the live Claude terminal title (`✳ <title>`) when the PTY is open.
  const label = claudeTitleName(title) ?? sessionLabel(eff);
  // Hover-reveal is driven by JS state, not CSS `:hover` — the Tauri webview's
  // WebKitGTK doesn't reliably update `:hover` on real pointer movement, so
  // `group-hover` utilities never fire even though `matchMedia('(hover:
  // hover)')` reports true.
  const [hovered, setHovered] = useState(false);
  return (
    <div
      role="button"
      tabIndex={0}
      title={eff.purpose ? `✦ ${eff.purpose}` : undefined}
      onClick={onSelect}
      onDoubleClick={() => actions.renameStart(session.id)}
      onKeyDown={(e) => e.key === "Enter" && onSelect()}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      className={cn(
        "relative ml-1.5 flex cursor-pointer items-center gap-2.5 border-l-2 border-transparent py-1.5 pr-3 pl-9",
        hovered && "bg-accent",
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
            if (e.key === "Enter")
              onRenameCommit((e.target as HTMLInputElement).value);
            if (e.key === "Escape") onRenameCommit(session.name);
          }}
          className="min-w-0 flex-1 rounded-sm border border-input bg-background px-1 text-sm outline-none"
        />
      ) : (
        <>
          <span
            className={cn(
              "min-w-0 flex-1 truncate",
              eff.live ? "text-foreground" : "text-muted-foreground",
            )}
          >
            {label}
          </span>
          {label !== session.name && (
            <span className="shrink-0 font-mono text-[10.5px] text-muted-foreground/70">
              {session.name}
            </span>
          )}
          {!agent && eff.shellKind && (
            <span className="shrink-0 font-mono text-[10.5px] text-muted-foreground/50">
              {eff.shellKind}
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
              className="flex min-w-0 shrink items-center gap-1"
            >
              <span
                className={cn(
                  "size-2 shrink-0 rounded-[3px]",
                  windowColor(
                    wins?.windows.filter((w) => w.folderDir === grouped.folderDir) ?? [],
                    grouped.id,
                  ),
                )}
              />
              <span className="max-w-12 truncate font-mono text-[9.5px] text-muted-foreground/60">
                {grouped.name}
              </span>
            </span>
          )}
          {/* Meta cluster stays in the flow permanently — the lifecycle
              controls overlay it (absolute, opaque accent) instead of
              swapping it out, so hovering never reflows the row. */}
          <span className="ml-auto flex min-w-0 shrink items-center gap-2">
            <CacheBadge
              session={eff}
              now={now}
              compactPct={compactPct}
              onCompact={() => actions.compactClaude(eff)}
            />
            {eff.live && (
              <span
                className="shrink-0 font-mono text-[10.5px] text-muted-foreground/70"
                title="running for"
              >
                {fmtElapsed(now - eff.createdAt)}
              </span>
            )}
            <span className="min-w-0 truncate text-[11px] text-muted-foreground">
              {sessionStatusText(eff)}
            </span>
          </span>
          {needs && (
            <span className="size-1.5 shrink-0 rounded-full bg-amber-500" />
          )}
          {(active || hovered) && (
            <span className="absolute inset-y-0 right-2 z-10 flex items-center gap-1 bg-accent pl-1.5">
              <RowControls session={eff} folderDir={folderDir} grouped={!!grouped} actions={actions} />
            </span>
          )}
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
    className = "hover:text-foreground",
  ) => (
    <IconBtn title={title} onClick={onClick} className={className}>
      {label}
    </IconBtn>
  );

  return (
    <>
      {!session.live && btn("▶", "start shell", () => actions.start(folderDir, session), "hover:text-green-500")}
      {(!session.live || !agent) &&
        btn("✦", "start Claude here", () => actions.startClaude(folderDir, session), "text-violet-500 hover:text-violet-400")}
      {session.live && agent && (
        <>
          {btn("■", "stop Claude (shell survives)", () => actions.stopClaude(session), "hover:text-red-500")}
          {atPrompt && btn("⤿", "compact context (/compact)", () => actions.compactClaude(session), "hover:text-sky-500")}
          {btn("↻", "start over — fresh Claude session", () => actions.restartClaude(folderDir, session), "hover:text-orange-500")}
        </>
      )}
      {grouped &&
        btn("⊟", "ungroup — remove pane from its window", () => actions.ungroup(session.id), "hover:text-sky-500")}
      {btn("✎", "rename", () => actions.renameStart(session.id))}
      {btn("✕", "close session", () => actions.close(session.id), "hover:text-red-500")}
    </>
  );
}
