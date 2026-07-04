# Porting spec — server state assembly & bridge (agentboard phase 3)

> **Update (2026-07-04).** The tmux server is **removed**; the `bridge`/state
> assembly core described here stays and now feeds only the Tauri app.


Source: slot-1 `packages/agentboard/src/runtime/server/index.ts` (1827) +
`shared.ts` types + tracker/metadata-store/session-order/git-info/port-scanner.
Spec taken 2026-07-02; covers the data-composition layer with tmux inputs
abstracted as "session source". **⚠ = flag; ◧ = TUI-vestigial** (broadcast but
never rendered by the current TUI — verified by grep, zero consumers).

## 1. `SessionData` assembly (`computeState`)

Per broadcast: gather sessions from all providers → sort → apply custom order
→ map each to `SessionData`.

**Ordering**: sessions sorted by `(createdAt asc, then name compare)`. Then
`sessionOrder.sync(names)` (drops stale, inserts new names alphabetically) and
`sessionOrder.apply(names)`: stored custom positions sort by index; unknowns
get `Infinity` and keep prior order via stable sort.

**Field-by-field source:**

| field | source | rule |
|---|---|---|
| `name` | session source | identity string (key for tracker/metadata/order) |
| `createdAt` | session source | epoch **seconds** |
| `dir` | session source | active pane cwd, fallback session path |
| `branch` | git-info | `rev-parse --abbrev-ref HEAD` |
| `isWorktree` ◧ | git-info | git-dir contains `/worktrees/` |
| `filesChanged` | git-info | changed vs pushed base + untracked count |
| `linesAdded`/`linesRemoved` | git-info | `diff --numstat <base>` sum (binary skipped) |
| `commitsDelta` | git-info | ahead(+)/behind(−) origin/main |
| `unseen` | tracker | any instance in session unseen |
| `panes` ◧ | session source | pane count |
| `ports` ◧ | port-scanner | sorted int[] |
| `windows` ◧ | session source | window count |
| `uptime` ◧ | derived | `formatUptime(createdAt)` → `"2d3h"`/`"5h20m"`/`"5m"`; NaN/negative → `""` |
| `agentState` | tracker + pane | `overrideTerminalIfPaneAlive(name, tracker.getState(name))` |
| `agents` | tracker + pane | `mergeAgentsWithPanePresence(name, tracker.getAgents(name))` |
| `eventTimestamps` ◧ | tracker | last 30 event ts (intended sparkline) |
| `metadata` | metadata store | `null` if status+progress+logs all empty |

After map: `metadataStore.pruneSessions(Set(current names))`.

**Derived details:**

- `agentState` = highest-priority agent via `tracker.getState`, priority
  `running(5) > question(4)=error(4) > interrupted(3) > waiting(2) > done(1)
  > idle(0)`.
- **"waiting" synthesis** (the one cross-source derivation that matters):
  `overrideTerminalIfPaneAlive` — if the picked state is terminal
  (`done/error/interrupted`) AND a live pane matches `(agent, threadId)` →
  rewritten to `waiting`. `mergeAgentsWithPanePresence` does the same
  per-agent AND (a) drops watcher agents whose `paneId` is set, status
  non-terminal, and pane gone (orphan guard), (b) adds synthetic agents for
  untracked panes. ⚠ No panes exist in the desktop app — replacement: drive
  `waiting` from pid-liveness (claude-pid: pid alive + terminal journal
  status ⇒ waiting).

## 2. Broadcast pipeline

Two debounce layers: `broadcastState()` is **microtask-coalesced** (many
callers in one tick → one rebuild); the watcher context's
`debouncedBroadcast()` is a **200ms** timer used only by `watcherCtx.emit`.

`broadcastStateImmediate()` runs, in order, every broadcast:

1. `tracker.pruneStuck(3min)`
2. `tracker.pruneTerminal()` (5min internal)
3. `tracker.pruneStale(12h)`
4. `tracker.pruneIdle(30s)`
5. `tracker.pruneSupersededByPane()`
6. `lastState = computeState()`
7. `syncGitWatchers(...)` (`.git/HEAD` fs-watch add/remove)
8. publish `lastState` to all clients

**Rebuild triggers**: watcher emit (200ms debounce); polls — git 1.5s, ports
10s, pane scan 3s (broadcast only on change); metadata HTTP mutations; client
commands; `.git/HEAD` fs-watch (200ms debounce → git cache invalidate).

**Recomputed vs cached**: git-info is cache-only read (5s TTL, stale served,
async background refresh — never blocks); ports read a 10s-poll snapshot;
tracker/metadata/order read live; `computeState` itself is full recompute (no
memo). **`handleFocus` fast-path**: mark-seen patches `unseen:false` in-place
on `lastState` and republishes without a full recompute (avoids spawning git
subprocesses) — mirror this: mark-seen must not force a rebuild.

**`state` payload** (`ServerState`): `{ type:"state", sessions:
SessionData[], theme: string|undefined, sidebarWidth: number,
preferredEditor: string, ts: number }`. `sidebarPosition` is config-only, NOT
broadcast. Other server→client messages: `session-viewed` (tmux focus
reconciliation — drop), `re-identify` (tmux — drop), `quit`, `resize`
(unused).

## 3. Client command semantics (app-internal effects only)

| command | effect | broadcast? |
|---|---|---|
| `mark-seen {name}` | `tracker.markSeen(name)` clears unseen for the session; false if nothing was unseen | only if changed |
| `dismiss-agent {session, agent, threadId?}` | `tracker.dismiss` removes the instance (`agent` or `agent:threadId`) + unseen flag; deletes empty session bucket | if removed |
| `reorder-session {name, delta}` | `sessionOrder.reorder`, delta ∈ up/down/top/bottom; persists to `~/.config/towles-tool/agentboard/session-order.json` | always |
| `set-theme {theme}` | set + `saveConfig({theme})` → shared settings `agentboard.theme` | always |
| `new-session` | tmux create → **replace with "add repo"** | always |
| `kill-session {name}` | tmux kill → **replace with "remove repo"**; note: tracker/metadata cleanup is implicit via the next `computeState`/`pruneSessions` dropping absent names — removing a repo from config must trigger the same | always |
| `refresh` | force broadcast | always |
| `switch-session` / `switch-index` / `identify-pane` / `focus-agent-pane` / `kill-agent-pane` / `quit` / `report-width` | tmux routing / lifecycle → **drop** | — |

## 4. Tracker pruning schedule

All prunes run on **every broadcast** (cadence = broadcast cadence, ≥1.5s git
poll floor with a client connected): `pruneStuck(3min)` running-and-unpinned;
`pruneTerminal(5min)` terminal, **seen AND unpinned** only; `pruneStale(12h)`
by `details.lastActivityAt ?? ts`; `pruneIdle(30s)` idle only;
`pruneSupersededByPane()` newest per `(paneId, agent)`.

"Pinned" = backed by a live pane. ⚠ Without the pane scanner **nothing is
pinned** and pruning is unguarded → the port must pin by pid-liveness
(claude-pid) or the 30s-idle/3min-stuck rules will eat live sessions.
`pruneSupersededByPane` is moot without paneIds.

## 5. Agent-facing HTTP metadata API

Localhost POST; `session` must be a non-empty string (400 else); mutate
`SessionMetadataStore` then broadcast; success = **204** (invalid JSON → 400).
Caps: `MAX_LOGS=50`, `MAX_MESSAGE_LENGTH=500`, status/progress-label ≤100,
log source ≤50. `tone` ∈ {neutral, info, success, warn, error}, invalid →
undefined.

| endpoint | body | semantics |
|---|---|---|
| `/set-status` | `{session, text: string\|null, tone?}` | null/undefined text clears; string → `{text≤100, tone, ts}`; non-string → 400 |
| `/set-progress` | `{session, current?, total?, percent?, label?, clear?}` | `clear:true` → null; else `{current,total,percent,label≤100, ts}` |
| `/log` | `{session, message, tone?, source?}` | non-empty message required (400); append `{message≤500, tone, source≤50, ts}`; ring last 50 |
| `/clear-log` | `{session}` | empties logs only |

`metadataStore.get` returns `null` when everything is empty (absent metadata
⇒ omitted from the card). This is the external agent/script integration
surface — keep it working.

## 6. Port decisions (adopted 2026-07-02)

- **Ports as-is**: `branch, filesChanged, linesAdded, linesRemoved,
  commitsDelta` (git on a dir), `unseen, agentState, agents` (tracker),
  `metadata`, the client-command semantics of mark-seen / dismiss-agent /
  reorder-session / set-theme, the prune schedule, and the metadata API
  payload shapes.
- **Replaced sources**: `name` + `dir` come from **repoPaths config** (label
  or dir basename; the config key for tracker/metadata/order);
  `new-session`/`kill-session` become add-repo/remove-repo (remove triggers
  tracker+metadata prune for that name); **waiting synthesis and prune
  pinning switch from pane-presence to pid-liveness** (claude-pid).
- **Dropped**: `panes`, `windows`, `uptime`, `createdAt` (only fed uptime),
  `isWorktree`, `eventTimestamps`, `ports` (revisit if the UI wants a ports
  column — needs a repo-cwd process scan without tmux), payload extras
  `sidebarWidth` (app window owns layout), `session-viewed`/`re-identify`/
  `resize` messages, and all tmux routing commands.
- **Bridge shape**: assemble trimmed SessionData from (repoPaths → name/dir)
  + git-info + tracker + metadata, ordered by session-order; recompute on
  watcher-emit (debounced) + git poll; expose mark-seen / dismiss-agent /
  reorder / set-theme / add-repo / remove-repo + the four metadata mutations
  as **Tauri commands**; emit the state snapshot as a **Tauri event**;
  metadata additionally reachable over a small localhost HTTP listener so
  existing scripts keep POSTing (phase 5).
