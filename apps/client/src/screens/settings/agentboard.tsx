import { useCallback, useEffect, useRef, useState } from "react";
import { FolderGit2, FolderPlus, GripVertical } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { useAgentboardState, type RepoCandidate, type RepoData } from "@/lib/agentboard";
import {
  applyRepoOrder,
  orderSettled,
  reorderDirs,
  sameOrder,
  showAddPath,
  untrackedCandidates,
} from "@/lib/repo-manager";
import {
  hasRepoColor,
  normalizeHex,
  repoAccentStyles,
  repoIcon,
  REPO_ICONS,
  REPO_PALETTE,
  type RepoIdentityStyle,
  type RepoMeta,
} from "@/lib/repo-identity";
import { liveSessionIds, trackRepo, untrackRepo } from "@/lib/repo-actions";
import { uiAction } from "@/lib/ui-action";
import { invoke } from "@/lib/tauri";
import { matchesFilter } from "@/lib/settings-filter";
import { NotInTauri } from "@/lib/errors";
import type { UserSettings } from "@/lib/settings";
import { DEFAULT_TERMINAL_FONT_SIZE, clampTerminalFontSize } from "@/lib/terminal-prefs";
import { cn } from "@/lib/utils";
import { PromptImproversEditor } from "./collectors";
import {
  CadenceRow,
  DEFAULT_COMPACT_RECOMMEND_PERCENT,
  ToggleRow,
  type FilterRow,
  type FilterSection,
  type Flush,
  type Update,
} from "./common";

export function agentboardSections(
  settings: UserSettings | null,
  update: Update,
  flush: Flush,
): FilterSection[] {
  const rows: FilterRow[] = [
    {
      label: "Scan roots",
      keywords: ["repo", "discovery", "directory", "picker", "add repo"],
      node: <AgentboardSettings />,
    },
    {
      label: "Repos",
      keywords: [
        "repo",
        "track",
        "untrack",
        "add repo",
        "remove",
        "rail",
        "order",
        "reorder",
        "icon",
        "color",
        "tint",
      ],
      node: <RepoManager />,
    },
  ];
  if (settings) {
    rows.push(
      {
        label: "Prompt improvers",
        keywords: [
          "prompt",
          "improver",
          "improve",
          "goal",
          "plan",
          "brainstorm",
          "template",
          "new task",
        ],
        node: (
          <PromptImproversEditor
            improvers={settings.promptImprovers ?? []}
            onChange={(improvers, opts) =>
              update((s) => ({ ...s, promptImprovers: improvers }), opts)
            }
            onCommit={() => void flush()}
          />
        ),
      },
      {
        label: "Needs-you notifications",
        keywords: ["notification", "desktop", "needs you", "alert"],
        node: (
          <ToggleRow
            label="Needs-you notifications"
            description="Desktop notification when an agent session flips to needs-you while the app is unfocused. Status only — act in the session's terminal."
            checked={settings.agentboard?.notifyNeedsYou ?? true}
            onCheckedChange={(v) =>
              update((s) => ({
                ...s,
                agentboard: { ...s.agentboard, notifyNeedsYou: v },
              }))
            }
          />
        ),
      },
      {
        label: "Meeting-start notifications",
        keywords: ["notification", "desktop", "meeting", "countdown", "alert"],
        node: (
          <ToggleRow
            label="Meeting-start notifications"
            description="Desktop notification when the next meeting's countdown reaches zero, while the app is unfocused."
            checked={settings.agentboard?.notifyMeetingStart ?? true}
            onCheckedChange={(v) =>
              update((s) => ({
                ...s,
                agentboard: { ...s.agentboard, notifyMeetingStart: v },
              }))
            }
          />
        ),
      },
      {
        label: "Review-requested notifications",
        keywords: ["notification", "desktop", "pr", "review", "alert"],
        node: (
          <ToggleRow
            label="Review-requested notifications"
            description="Desktop notification when a PR newly needs your review, while the app is unfocused."
            checked={settings.agentboard?.notifyReviewRequested ?? true}
            onCheckedChange={(v) =>
              update((s) => ({
                ...s,
                agentboard: { ...s.agentboard, notifyReviewRequested: v },
              }))
            }
          />
        ),
      },
      {
        label: "CI-failing notifications",
        keywords: ["notification", "desktop", "pr", "ci", "checks", "failing", "alert"],
        node: (
          <ToggleRow
            label="CI-failing notifications"
            description="Desktop notification when one of your PRs' checks flip to failing, while the app is unfocused."
            checked={settings.agentboard?.notifyChecksFailed ?? true}
            onCheckedChange={(v) =>
              update((s) => ({
                ...s,
                agentboard: { ...s.agentboard, notifyChecksFailed: v },
              }))
            }
          />
        ),
      },
      {
        label: "Stale-collector notifications",
        keywords: ["notification", "desktop", "collector", "stale", "health", "alert"],
        node: (
          <ToggleRow
            label="Stale-collector notifications"
            description="Desktop notification when a collector stops refreshing or keeps failing (expired gh auth, revoked Slack token)."
            checked={settings.agentboard?.notifyStaleCollector ?? true}
            onCheckedChange={(v) =>
              update((s) => ({
                ...s,
                agentboard: { ...s.agentboard, notifyStaleCollector: v },
              }))
            }
          />
        ),
      },
      {
        label: "Compaction recommendation",
        keywords: ["context", "compact", "percent", "threshold", "session", "usage"],
        node: (
          <CadenceRow
            label="Compaction recommendation"
            description="Flag a session for compaction once its context usage exceeds this percentage."
            unit="%"
            value={
              settings.agentboard?.compactRecommendPercent ?? DEFAULT_COMPACT_RECOMMEND_PERCENT
            }
            onValue={(n) =>
              update(
                (s) => ({
                  ...s,
                  agentboard: {
                    ...s.agentboard,
                    compactRecommendPercent: Math.min(100, Math.max(1, n)),
                  },
                }),
                { defer: true },
              )
            }
            onCommit={() => void flush()}
          />
        ),
      },
      {
        label: "Copy on select",
        keywords: ["terminal", "clipboard", "selection", "copy"],
        node: (
          <ToggleRow
            label="Copy on select"
            description="Copy the terminal selection to the clipboard as soon as you finish selecting, without Ctrl/⌘+Shift+C."
            checked={settings.agentboard?.copyOnSelect ?? true}
            onCheckedChange={(v) =>
              update((s) => ({
                ...s,
                agentboard: { ...s.agentboard, copyOnSelect: v },
              }))
            }
          />
        ),
      },
      {
        label: "Terminal font size",
        keywords: ["terminal", "font", "size", "zoom", "text"],
        node: (
          <CadenceRow
            label="Terminal font size"
            description="Font size (px) for the app's terminals. Zoom in/out live with Ctrl/⌘ +/- (Ctrl/⌘ 0 resets)."
            unit="px"
            value={settings.agentboard?.terminalFontSize ?? DEFAULT_TERMINAL_FONT_SIZE}
            onValue={(n) =>
              update(
                (s) => ({
                  ...s,
                  agentboard: { ...s.agentboard, terminalFontSize: clampTerminalFontSize(n) },
                }),
                { defer: true },
              )
            }
            onCommit={() => void flush()}
          />
        ),
      },
      {
        label: "Shortcuts work in terminal",
        keywords: ["shortcut", "keyboard", "terminal", "focus", "hotkey", "jump", "needs you"],
        node: (
          <ToggleRow
            label="Shortcuts work in terminal"
            description="Board-wide shortcuts (jump to next/prev session needing you, close/split session, toggle diff/rail) fire even while a terminal has focus, instead of being sent to the shell."
            checked={settings.agentboard?.shortcutsWorkInTerminal ?? true}
            onCheckedChange={(v) =>
              update((s) => ({
                ...s,
                agentboard: { ...s.agentboard, shortcutsWorkInTerminal: v },
              }))
            }
          />
        ),
      },
    );
  }
  return [{ rows }];
}

/**
 * Scan-root editor for repo discovery. Reads/writes `scanRoots`
 * in `~/.config/towles-tool/agentboard/repos.json` over the `ab_*` Tauri
 * commands (no shared settings file, no zod — pure Rust round-trip). One root
 * per line; empty falls back to `~/code`.
 */
function AgentboardSettings() {
  const [roots, setRoots] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pending = useRef<string | null>(null);

  useEffect(() => {
    void invoke<string[]>("ab_get_scan_roots").then((r) => setRoots(r.unwrapOr([]).join("\n")));
  }, []);

  // Autosave, like the rest of this screen. Deliberately does *not* write the
  // normalized list back into the textarea: this fires mid-typing, and replacing
  // the value would eat the blank line you just opened and jump the cursor.
  const persist = useCallback(async () => {
    if (timer.current !== null) {
      clearTimeout(timer.current);
      timer.current = null;
    }
    const raw = pending.current;
    if (raw === null) return;
    pending.current = null;
    const list = raw
      .split("\n")
      .map((s) => s.trim())
      .filter(Boolean);
    const stored = await invoke("ab_set_scan_roots", { roots: list });
    if (stored.isErr()) {
      if (!NotInTauri.is(stored.error)) {
        toast.error(`Couldn't save scan roots — ${stored.error.message}`);
      }
      return;
    }
    setSaved(true);
    window.setTimeout(() => setSaved(false), 1500);
  }, []);

  const edit = (next: string) => {
    setRoots(next);
    pending.current = next;
    if (timer.current !== null) clearTimeout(timer.current);
    timer.current = setTimeout(() => void persist(), 600);
  };

  // Commit a pending edit if the pane unmounts (Radix drops it on tab switch).
  const persistRef = useRef(persist);
  persistRef.current = persist;
  useEffect(
    () => () => {
      void persistRef.current();
    },
    [],
  );

  if (roots === null) {
    return <div className="text-sm text-muted-foreground">Loading…</div>;
  }

  return (
    <div className="flex flex-col gap-3">
      <div>
        <div className="text-sm font-medium">Scan roots</div>
        <p className="text-sm text-muted-foreground">
          One directory per line. The repo list below scans these for git repos. Leave empty to use{" "}
          <span className="font-mono">~/code</span>. A leading <span className="font-mono">~</span>{" "}
          expands to your home directory.
        </p>
      </div>
      <Textarea
        value={roots}
        onChange={(e) => edit(e.target.value)}
        onBlur={() => void persist()}
        rows={5}
        placeholder="~/code"
        className="font-mono text-xs"
        spellCheck={false}
      />
      {saved && <span className="text-xs text-muted-foreground">Saved.</span>}
    </div>
  );
}

/**
 * The **one** place repos are managed. Track/untrack, drag to set the rail's
 * order, and give a repo its own glyph and color — all against the same
 * agentboard snapshot the rail renders. (There used to be a second surface, a
 * "Manage repos" command dialog on the Agentboard screen; it was deleted, and
 * its rail button now deep-links here.)
 *
 * Identity is only offered for *tracked* repos: a discovered-but-untracked
 * candidate has nowhere to render an icon, so those rows carry a Track action
 * and nothing else rather than dead controls.
 */
function RepoManager() {
  const { repos } = useAgentboardState();
  const [candidates, setCandidates] = useState<RepoCandidate[]>([]);
  const [query, setQuery] = useState("");
  const [confirm, setConfirm] = useState<{
    dir: string;
    name: string;
    /** Live session ids closed on confirm — see `untrack`. */
    sessionIds: string[];
  } | null>(null);
  // Optimistic order, held only until a poll reports the same sequence — a
  // dropped row must not snap back for the length of the IPC round-trip.
  const [order, setOrder] = useState<string[] | null>(null);
  const [dragDir, setDragDir] = useState<string | null>(null);
  const [dropBefore, setDropBefore] = useState<string | null>(null);

  const refresh = async () => {
    setCandidates((await invoke<RepoCandidate[]>("ab_discover_repos")).unwrapOr([]));
  };

  // This pane only exists while the Agentboard tab is the selected one (Radix
  // unmounts the other panes), so a mount is exactly "the tab was shown".
  useEffect(() => {
    void refresh();
  }, []);

  const snapshotDirs = repos.map((r) => r.dir);
  const ordered = applyRepoOrder(repos, order);
  const trackedDirs = new Set(snapshotDirs);
  // Drop the optimistic overlay once the snapshot reflects the drag.
  const settled = orderSettled(order, snapshotDirs);
  useEffect(() => {
    if (settled) setOrder(null);
  }, [settled]);

  const visibleRepos = ordered.filter((r) => matchesFilter(query, r.name, [r.dir]));
  const visibleCandidates = untrackedCandidates(candidates, trackedDirs).filter((c) =>
    matchesFilter(query, c.name, [c.dir]),
  );

  const track = async (path: string) => {
    if (await trackRepo(path, "settings")) await refresh();
  };

  const untrack = async (dir: string, name: string, sessionIds: string[] = []) => {
    if (await untrackRepo(dir, name, sessionIds, "settings")) await refresh();
  };

  // Untracking a repo whose sessions are still running stops them, so that
  // case confirms first (same guard the deleted dialog carried).
  const requestUntrack = (repo: RepoData) => {
    const liveIds = liveSessionIds(repo);
    if (liveIds.length === 0) {
      void untrack(repo.dir, repo.name);
      return;
    }
    setConfirm({ dir: repo.dir, name: repo.name, sessionIds: liveIds });
  };

  const drop = (beforeDir: string | "end") => {
    const dragged = dragDir;
    setDragDir(null);
    setDropBefore(null);
    if (!dragged) return;
    const current = ordered.map((r) => r.dir);
    const next = reorderDirs(current, dragged, beforeDir);
    if (sameOrder(current, next)) return;
    setOrder(next);
    uiAction("repo.reordered", "settings");
    void invoke("ab_set_repo_order", { dirs: next }).then((res) => {
      if (res.isErr() && !NotInTauri.is(res.error)) {
        toast.error(`Couldn't save the repo order — ${res.error.message}`);
        setOrder(null);
      }
    });
  };

  return (
    // A bottom rule + generous gap: this block ends in a list of rows, and the
    // settings rows that follow look just like them without a hard break.
    <div className="flex flex-col gap-3 border-b border-border pb-5">
      <div>
        <div className="text-sm font-medium">Repos</div>
        <p className="text-sm text-muted-foreground">
          Everything about the rail's repo list lives here: which repos are tracked, the order they
          sit in (drag a row), and each one's glyph and color so you can pick it out — especially in
          the collapsed icon strip — without reading names. Identity is decoration only: it never
          changes a status signal, and a repo waiting on you still shows amber.
        </p>
      </div>

      <Input
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        placeholder="Search repos, or type an absolute path…"
        spellCheck={false}
        aria-label="Search repos"
      />

      {repos.length === 0 && (
        <p className="text-sm text-muted-foreground/70">
          No repos tracked yet — track one from the list below.
        </p>
      )}

      <section aria-label="Tracked repos" className="flex flex-col gap-1">
        <h4 className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
          On the rail
        </h4>
        <div className="flex flex-col overflow-hidden rounded-md border border-border">
          {visibleRepos.map((repo) => (
            <RepoIdentityRow
              key={repo.key}
              repo={repo}
              dragging={dragDir === repo.dir}
              dropTarget={dropBefore === repo.dir}
              onDragStart={() => setDragDir(repo.dir)}
              onDragOverRow={() => setDropBefore(repo.dir)}
              onDropRow={() => drop(repo.dir)}
              onDragEnd={() => {
                setDragDir(null);
                setDropBefore(null);
              }}
              onUntrack={() => requestUntrack(repo)}
            />
          ))}
          {dragDir && (
            <div
              onDragOver={(e) => {
                e.preventDefault();
                setDropBefore(null);
              }}
              onDrop={(e) => {
                e.preventDefault();
                drop("end");
              }}
              className="m-1 h-6 rounded-md border border-dashed border-border/70"
            />
          )}
        </div>
      </section>

      {showAddPath(query, candidates, trackedDirs) && (
        <Button
          variant="outline"
          size="sm"
          className="self-start"
          onClick={() => void track(query.trim())}
        >
          <FolderPlus className="size-3.5" /> Add path {query.trim()}
        </Button>
      )}

      {visibleCandidates.length > 0 && (
        <section aria-label="Repos not tracked" className="flex flex-col gap-1">
          <h4 className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
            Found under your scan roots ({visibleCandidates.length})
          </h4>
          <p className="text-xs text-muted-foreground/70">
            Not on the rail. Track one to give it a glyph, a color, and a place in the order — or
            search above to narrow this list.
          </p>
          {/* Filled + bordered so this list reads as its own block: it sits
              between the tracked list and the notification settings below,
              and without containment its rows look like more settings. */}
          <div className="flex max-h-64 flex-col overflow-y-auto rounded-md border border-dashed border-border bg-muted/30">
            {visibleCandidates.map((c) => (
              <div
                key={c.dir}
                className="flex items-center gap-3 border-t border-border/60 px-2 py-2 first:border-t-0"
              >
                <FolderGit2 aria-hidden className="size-4 shrink-0 text-muted-foreground" />
                <div className="flex min-w-0 flex-1 flex-col">
                  <span className="truncate text-sm">{c.name}</span>
                  <span className="truncate font-mono text-xs text-muted-foreground">{c.dir}</span>
                </div>
                <Button
                  variant="outline"
                  size="sm"
                  className="px-2 text-xs"
                  onClick={() => void track(c.dir)}
                >
                  Track
                </Button>
              </div>
            ))}
          </div>
        </section>
      )}

      <AlertDialog open={confirm !== null} onOpenChange={(open) => !open && setConfirm(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Untrack {confirm?.name} from the rail?</AlertDialogTitle>
            <AlertDialogDescription>
              {confirm?.sessionIds.length}{" "}
              {confirm?.sessionIds.length === 1 ? "session is" : "sessions are"} still running.
              Untracking will stop {confirm?.sessionIds.length === 1 ? "it" : "them"}.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                if (confirm) void untrack(confirm.dir, confirm.name, confirm.sessionIds);
                setConfirm(null);
              }}
            >
              Stop &amp; untrack
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

function RepoIdentityRow({
  repo,
  dragging,
  dropTarget,
  onDragStart,
  onDragOverRow,
  onDropRow,
  onDragEnd,
  onUntrack,
}: {
  repo: RepoData;
  dragging: boolean;
  dropTarget: boolean;
  onDragStart: () => void;
  onDragOverRow: () => void;
  onDropRow: () => void;
  onDragEnd: () => void;
  onUntrack: () => void;
}) {
  // Local state is the truth once you have edited: the agentboard snapshot that
  // seeded it arrives on a poll, and re-syncing from it mid-edit would fight
  // the user's own clicks.
  const [meta, setMeta] = useState<RepoMeta | undefined>(repo.meta);
  const [hex, setHex] = useState(repo.meta?.color ?? "");
  const [hexError, setHexError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const dir = repo.dir;
  const Icon = repoIcon(meta);
  const accent = repoAccentStyles(meta);
  // The latest edit, readable synchronously. `meta` state lags an in-flight
  // commit by a full await, and `ab_set_repo_meta` replaces the identity
  // *wholesale* — so building the next edit off the render closure would let a
  // second click (pick an icon, then flick Tint before the first IPC lands)
  // send a payload missing the first field and silently erase it.
  const latest = useRef<RepoMeta | undefined>(repo.meta);

  const commit = async (next: RepoMeta | null, action: string, detail?: string) => {
    latest.current = next ?? undefined;
    const res = await invoke("ab_set_repo_meta", {
      dir,
      icon: next?.icon ?? null,
      color: next?.color ?? null,
      style: next?.style ?? null,
    });
    if (res.isErr()) {
      if (!NotInTauri.is(res.error)) {
        toast.error(`Couldn't save ${repo.name} — ${res.error.message}`);
      }
      // Roll the optimistic ref back so the next edit doesn't build on a value
      // the backend rejected.
      latest.current = meta;
      return;
    }
    setMeta(next ?? undefined);
    uiAction(action, "settings", detail);
    setSaved(true);
    window.setTimeout(() => setSaved(false), 1500);
  };

  const setIcon = (name: string) =>
    void commit({ ...latest.current, icon: name }, "repo.icon_set", name);
  const setColor = (raw: string, detail: string) => {
    // Rust stores a malformed color as null, which would silently blank the
    // repo — so a bad value never leaves the client.
    const canonical = normalizeHex(raw);
    if (!canonical) {
      setHexError("Use #rgb or #rrggbb");
      return;
    }
    setHexError(null);
    setHex(canonical);
    void commit({ ...latest.current, color: canonical }, "repo.color_set", detail);
  };

  // Autosave the typed hex, like every other control on this screen. A partial
  // value is *silently* ignored rather than reported: this runs while you're
  // still typing, and "#3b" is half-finished, not wrong. The error surfaces on
  // blur (below) and on Enter, where the input really is final.
  const hexTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const editHex = (raw: string) => {
    setHex(raw);
    setHexError(null);
    if (hexTimer.current !== null) clearTimeout(hexTimer.current);
    hexTimer.current = setTimeout(() => {
      if (normalizeHex(raw)) setColor(raw, "hex");
    }, 600);
  };
  const commitHex = (detail: string) => {
    if (hexTimer.current !== null) {
      clearTimeout(hexTimer.current);
      hexTimer.current = null;
    }
    // Leaving the field empty isn't an error — it just means "no custom color".
    if (hex.trim() === "") {
      setHexError(null);
      return;
    }
    setColor(hex, detail);
  };
  useEffect(
    () => () => {
      if (hexTimer.current !== null) clearTimeout(hexTimer.current);
    },
    [],
  );
  const setStyle = (tint: boolean) => {
    const style: RepoIdentityStyle = tint ? "tint" : "accent";
    void commit({ ...latest.current, style }, "repo.style_set", style);
  };
  const reset = () => {
    setHex("");
    setHexError(null);
    void commit(null, "repo.identity_reset");
  };

  return (
    <div
      // Give the row the same repo-identity decoration as the rail header — the
      // colored left edge and (for `tint`) the soft wash — so a repo's chosen
      // color is visible where you set it, not only after the fact on the rail.
      // Both are `undefined` for a repo with no color, so plain repos stay
      // plain. The row isn't sticky, so the wash mixes into `transparent`
      // (letting the panel show through) rather than an opaque base.
      style={{ ...accent.edgeStyle, ...accent.surfaceStyle }}
      onDragOver={(e) => {
        e.preventDefault();
        e.dataTransfer.dropEffect = "move";
        onDragOverRow();
      }}
      onDrop={(e) => {
        e.preventDefault();
        onDropRow();
      }}
      className={cn(
        // `border-l-2 … border-l-transparent` reserves the edge so `edgeStyle`'s
        // inline `borderLeftColor` has a border to paint and unthemed rows don't
        // shift width — the same idiom the rail header uses.
        "flex items-center gap-2 border-t border-l-2 border-border border-l-transparent px-2 py-2 first:border-t-transparent",
        dragging && "opacity-50",
        dropTarget && "border-t-violet-500",
      )}
    >
      <span
        draggable
        onDragStart={(e) => {
          e.dataTransfer.effectAllowed = "move";
          // Firefox/WebKit refuse to start a drag with an empty payload.
          e.dataTransfer.setData("text/plain", dir);
          onDragStart();
        }}
        onDragEnd={onDragEnd}
        aria-label={`Reorder ${repo.name}`}
        title="Drag to reorder"
        className="shrink-0 cursor-grab text-muted-foreground active:cursor-grabbing"
      >
        <GripVertical className="size-4" />
      </span>
      <Icon
        aria-hidden
        className={cn("size-4 shrink-0", !hasRepoColor(meta) && "text-muted-foreground")}
        style={accent.iconStyle}
      />
      <div className="flex min-w-0 flex-1 flex-col">
        <span className="truncate text-sm">{repo.name}</span>
        <span className="truncate font-mono text-xs text-muted-foreground">{dir}</span>
      </div>
      {saved && <span className="shrink-0 text-xs text-muted-foreground">Saved.</span>}

      <Popover>
        <PopoverTrigger asChild>
          <Button variant="outline" size="sm" className="px-2 text-xs">
            Icon
          </Button>
        </PopoverTrigger>
        <PopoverContent className="w-56 p-2" align="end">
          <div className="grid grid-cols-8 gap-1">
            {Object.entries(REPO_ICONS).map(([name, Choice]) => (
              <button
                key={name}
                type="button"
                title={name}
                aria-label={name}
                aria-pressed={meta?.icon === name}
                onClick={() => setIcon(name)}
                className={cn(
                  "flex size-6 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground",
                  meta?.icon === name && "bg-accent text-foreground",
                )}
              >
                <Choice className="size-3.5" style={accent.iconStyle} />
              </button>
            ))}
          </div>
        </PopoverContent>
      </Popover>

      <Popover
        onOpenChange={(open) => {
          // A rejected hex is only reported inside this popover, so a stale
          // error would greet you on reopen with nothing explaining it.
          if (!open) {
            setHexError(null);
            setHex(latest.current?.color ?? "");
          }
        }}
      >
        <PopoverTrigger asChild>
          <Button variant="outline" size="sm" className="px-2 text-xs">
            Color
          </Button>
        </PopoverTrigger>
        <PopoverContent className="w-56 p-2" align="end">
          <div className="grid grid-cols-5 gap-1.5">
            {REPO_PALETTE.map((swatch) => (
              <button
                key={swatch}
                type="button"
                title={swatch}
                aria-label={swatch}
                aria-pressed={meta?.color === swatch}
                onClick={() => setColor(swatch, "palette")}
                style={{ backgroundColor: swatch }}
                className={cn(
                  "size-6 rounded-md border border-border",
                  meta?.color === swatch && "ring-2 ring-ring ring-offset-1 ring-offset-background",
                )}
              />
            ))}
          </div>
          <div className="mt-2 flex items-center gap-1.5">
            <Input
              value={hex}
              onChange={(e) => editHex(e.target.value)}
              onBlur={() => commitHex("hex")}
              onKeyDown={(e) => {
                if (e.key === "Enter") commitHex("hex");
              }}
              placeholder="#3b82f6"
              spellCheck={false}
              aria-label="Custom color"
              aria-invalid={hexError !== null}
              className="h-7 flex-1 font-mono text-xs"
            />
          </div>
          {hexError && <p className="mt-1 text-xs text-red-500">{hexError}</p>}
        </PopoverContent>
      </Popover>

      <label className="flex shrink-0 items-center gap-1.5 text-xs text-muted-foreground">
        <Switch
          checked={(meta?.style ?? "accent") === "tint"}
          onCheckedChange={setStyle}
          aria-label={`Tint the ${repo.name} row background`}
        />
        Tint
      </label>
      <Button variant="ghost" size="sm" className="px-2 text-xs" onClick={reset}>
        Reset
      </Button>
      <Button
        variant="ghost"
        size="sm"
        className="px-2 text-xs text-muted-foreground hover:text-foreground"
        onClick={onUntrack}
      >
        Untrack
      </Button>
    </div>
  );
}
