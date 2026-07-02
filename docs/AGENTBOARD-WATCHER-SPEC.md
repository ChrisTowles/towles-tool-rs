# Porting spec — claude-code agent watcher (agentboard phase 2)

Source: slot-1 `packages/agentboard/src/runtime/agents/watchers/claude-code.ts`
(659) + `claude-usage.ts` (78) + `claude-pid.ts` (57), contract
`runtime/contracts/agent-watcher.ts`. Spec taken 2026-07-02 from a full source
read; written so the Rust implementer needn't reopen the TS. **⚠ = flagged
TS behavior** (see Port decisions at the end).

## 0. Contract & wiring

`AgentWatcher` = `{ name: string; start(ctx); stop() }`. `ctx`
(AgentWatcherContext):

- `resolveSession(projectDir) -> sessionName | null` — server maps a decoded
  project dir to a session (longest-prefix match + dash-reencode fallback).
- `emit(event: AgentEvent)` — server does
  `tracker.applyEvent(event, {seed: !watchersSeeded})` then a **200ms-debounced**
  broadcast.

`name = "claude-code"`. Constants: `POLL_MS=2000`, `STALE_MS=5*60_000` (5min),
`JOURNAL_IDLE_TIMEOUT_MS=120_000` (2min, from shared.ts),
`JSONL_SUFFIX=".jsonl"`.

Per-thread in-memory state `SessionState`:
`{ status, fileSize, threadName?, projectDir?, usage?, lastTool?, subagents?,
subagentSig?, loop? }`, keyed by `threadId` (= JSONL filename stem). Two
independent "seeded" flags: **watcher-local `seeded`** (false until first full
scan completes) and **server-global `watchersSeeded`** (false for first 3s →
seed emits get marked unseen).

## 1. Watch setup

Root: `~/.claude/projects/`. Layout: `<encoded-project-dir>/<session-id>.jsonl`
(+ optional `<session-id>/subagents/agent-*.jsonl`).

- **start()**: `setupWatchers()`; one-shot scan after 50ms; then scan every 2s.
- **setupWatchers()**: for each subdir with ≥1 `.jsonl` modified within
  STALE_MS → `watchDir()`; also fs-watch the **projectsDir top level** for
  `rename` events → watch newly-created dirs. **Per-directory watches, NOT
  recursive.**
- **watchDir(dir)**: idempotent (`watchedDirs` set). Watch callback: on any
  `.jsonl` filename → `processFile()`. Watches are also added lazily during
  scan for any dir that currently has recent files.
- **Polling every 2s is the reliable path**; fs watches are a low-latency
  supplement. A `scanning` guard prevents overlap (re-entrant scan returns
  immediately).
- **New session discovery**: purely by re-readdir each scan; the STALE_MS
  filter means files untouched >5min are **skipped entirely** (not processed,
  not watched) — their last-emitted tracker state lingers until tracker
  pruning.

**Project-dir encoding** (`decodeProjectDir`): `encoded.replace(/-/g, "/")`.
Claude encodes `/`→`-` with **no escaping of literal dashes**.
⚠ **Lossy/ambiguous**: `/home/u/my-project` → `-home-u-my-project` → decodes
to `/home/u/my/project` (wrong). The TS relies on the server's
`resolveSession` to compensate (it re-encodes each known session dir and
prefix-matches the still-encoded input).

## 2. Incremental read algorithm

`processFile(filePath, projectDir)`:

1. `stat` → `size` (return on error). `threadId = stem(filePath)`.
   `prev = sessions.get(threadId)`.
2. Always compute subagents first:
   `subagentsDir = <filePath minus .jsonl>/subagents`;
   `subagents = readActiveSubagents(...)`; `subagentSig`.
3. **Branch A — no growth (`prev && size === prev.fileSize`)**:
   - If `seeded && prev.status === "running"`: liveness check → if pid
     known-and-dead **OR** file mtime older than 2min → set status `idle`,
     emit idle, return.
   - Else if `subagentSig` changed → emit with **`prev.status`** (subagent set
     shifted while journal static). ⚠ re-emitting a `done` here refreshes
     tracker `ts`, resetting terminal-prune timers — a churning workflow can
     keep a done card alive.
   - return.
4. **Branch B — seed mode (`!seeded`)**: read **whole file**,
   `parseJournalLines`, `scanEntries("idle", undefined)`, extract
   usage/lastTool/loop. If final status `running` but mtime >2min stale →
   `idle`. Store state. **No emit** (deferred to seed finalize).
5. **Branch C — seeded & grew**: `offset = prev?.fileSize ?? 0`;
   **`if (size <= offset) return`**; read bytes `[offset, size)`; parse;
   `scanEntries(prev.status ?? "idle", prev.threadName)`;
   `usage = new ?? prev.usage`, `lastTool = new ?? prev.lastTool`,
   `loop = new ?? prev.loop`; if `running` → pid-dead → `idle`; store state;
   **emit iff** `status !== prevStatus || subagentSig changed ||
   loop.nextWakeAt changed`.

**Offset tracking**: `fileSize` is the raw byte size, stored as the next read
offset. **Partial-line handling**: `parseJournalLines` splits on `\n` and
silently drops any line that fails JSON parse (including a trailing partial
line). ⚠ **Bug**: offset advances to `size` even when the tail was a partial
line, so a line split across two reads (content flushed before its `\n`) is
**lost entirely** — never re-parsed. **Truncation/rotation**: if a file
shrinks (`size < prev.fileSize`), Branch A misses (sizes differ) and Branch
C's `size <= offset` returns early — the watcher is **stuck**, never
re-reading; `fileSize` stays stale. ⚠

**Entry types examined**: only `message.role` ∈ {user, assistant} and, for
assistant, `message.content[]` items with `type: "tool_use"` (and `.name`) or
`type: "text"`. `type: "thinking"` and other content types are ignored (§3).
`timestamp` is used for usage/loop time math.

## 3. Status derivation — decision table

`determineStatus(entry) -> AgentStatus | null` (content normalized: array
kept; string → `[{type:text}]`; else `[]`):

| role | content | → status |
|---|---|---|
| (missing role) | — | `null` (ignored) |
| `user` | any | `running` |
| `assistant` | 0 `tool_use` items (text-only, empty, or string) | `done` |
| `assistant` | ≥1 `tool_use`, **all** named `AskUserQuestion` | `question` |
| `assistant` | ≥1 `tool_use`, any non-AskUserQuestion | `running` |
| other (`system`, …) | — | `null` (ignored) |

`scanEntries` folds entries in order: `status = determineStatus(e) ?? status`
(null keeps prior); captures `threadName` from the **first** qualifying user
message only (skipping system-like `<...>`/`{...}` first lines).

⚠ **This watcher only ever emits {running, done, question, idle}.** It never
emits `waiting`/`error`/`interrupted` — `waiting` is synthesized by the server
when a pane process is alive under a terminal journal status; `error`/
`interrupted` come from other watchers (amp/codex).

⚠ **thinking-only assistant entries read as `done`** (no tool_use, no text →
`done`).

**Status→idle timing**: there is no `working` status. The only automatic
demotion is running→idle via liveness (§5): pid dead, or (Branch A) journal
mtime >2min. `done`/`question` are never auto-demoted by the watcher — tracker
pruning handles them (idle-prune 30s touches only `idle`; terminal-prune 5min
for *seen* `done`; stale-prune 12h).

**lastTool** (`extractLastTool`): scan entries newest→oldest; first assistant
`tool_use` with a `.name` that is **not** `AskUserQuestion`; returns the first
tool of that turn if multiple; else undefined.

**model / usage tokens** (`extractUsageSummary`): newest→oldest, first
assistant entry with `message.usage` and a parseable `timestamp`:

- `model = message.usage.model ?? ""` (from `message.model`).
- `contextUsed = input + output + cache_read + cache_creation` input tokens.
- `contextMax = /\[1m\]$/i.test(model) ? 1_000_000 : 200_000`.
- `cacheTtlMs`: `ephemeral_1h_input_tokens>0 → 3_600_000`; else
  `ephemeral_5m_input_tokens>0 || cache_read>0 → 300_000`; else `null`.
- `cacheExpiresAt = ttl===null ? null : timestampMs + ttl`.
- `lastActivityAt = timestampMs`.

⚠ In Branch C, usage/model/lastTool refresh but do **not** by themselves
trigger an emit (gate is status/subagent/loop only), so the context bar only
updates when a status change happens to ride along.

**/loop ScheduleWakeup** (`extractLoopState`): newest→oldest, first assistant
`tool_use` named `ScheduleWakeup`. If `input.delaySeconds` is a number and
`timestamp` parses: `{ nextWakeAt: timestampMs + delaySeconds*1000, reason }`;
else undefined. Does not change status — pure `details.loop` metadata (a
future `nextWakeAt` renders as "looping, sleeping"). A `loop.nextWakeAt`
change **is** part of the Branch-C emit gate. Only `tool_use` entries count.

## 4. Subagent detection (`readActiveSubagents(subagentsDir, now)`)

readdir (→ `[]` if missing). For each `agent-*.jsonl`: stat mtime; **skip if
`now - mtime > 2min`**. Read sibling `agent-<id>.meta.json` →
`{agentType?, description?}` (agent still counts if meta missing/unreadable →
`{}`). Sort most-recently-active first → `SubagentInfo[]`. Non-`agent-*` files
ignored. `subagentSignature(list)` = per-agent
`` `${agentType??""} ${description??""}` `` **sorted then joined**
(order-independent change signature). Computed on **every** processFile call.

## 5. Liveness cross-check (`claude-pid.ts`)

`~/.claude/sessions/<pid>.json`, each `{ pid, sessionId }`.
`createClaudePidLookup`:

- `pidForThread(threadId)`: lazily builds a `sessionId→pid` map by reading
  all `*.json` in the sessions dir (skips unreadable/invalid); cached until
  `invalidate()`.
- `isAlive(pid)`: signal-0 check (Rust: `kill(pid, 0)` or `/proc/<pid>`).
- `invalidate()`: called at the top of **every** scan → map rebuilt every 2s.

Demotion: a `running` thread demotes to `idle` when pid is known-and-dead, OR
(Branch A only) journal mtime >2min. No matching pid entry → no pid-based
demotion (mtime only). Tracker interplay: watcher emits `idle`; tracker
`pruneIdle(30s)` removes unpinned idle instances shortly after (the pane
scanner re-pins live panes so real sessions survive). Prune-by-age compares
against `lastActivityAt` (journal turn time — can be minutes stale), so an
idle emit is often pruned almost immediately.

## 6. Event emission — when & payload

Payload (all emits): `{ agent:"claude-code", session, status, ts, threadId,
threadName?, details? }`. No `paneId`/`unseen` — stamped later by
pane-scanner/tracker. `details` = undefined if all absent, else
`{ model, contextUsed, contextMax, cacheExpiresAt?, cacheTtlMs?,
lastActivityAt, lastTool?, subagents?, loop? }` (null cache fields →
undefined).

Emit sites:

1. **Seed finalize** (first scan completes, `seeded` flips): for each stored
   session with `status !== "idle"` && `projectDir` && resolvable session;
   re-checks running-liveness (demoted → skipped). These land while the
   server's `watchersSeeded` is still false → marked **unseen**.
2. **Branch A running→idle** (pid dead / mtime stale): emit `idle`.
3. **Branch A subagentSig changed**: emit `prev.status`.
4. **Branch C**: emit iff `status changed || subagentSig changed ||
   loop.nextWakeAt changed`.

## 7. Persistence

**Nothing survives restart** — sessions map, offsets, pid cache, tracker are
all rebuilt via seed on start. Only `session-order.json` persists (separate
module). Seed reconstructs pre-existing non-idle sessions flagged unseen.

## 8. Edge cases

Handled: empty file (idle, no emit); malformed JSONL line (skipped
individually); missing meta.json (subagent counts as `{}`); multiple sessions
in one dir (each `.jsonl` = distinct threadId → separate tracker instance,
same resolved session); system-like first user message (skipped for
threadName). Not handled (⚠): clock skew / future journal timestamps; the
partial-line and truncation gaps of §2.

## Port decisions (adopted 2026-07-02)

Port §3/§4/§5/§6 **faithfully**. Deliberate fixes during the port:

1. **Offset at last newline boundary** + re-read the incomplete tail next
   tick (fixes partial-line loss); **reset offset to 0 and re-seed the thread
   when a file shrinks** (fixes stuck-on-truncation).
2. **Add usage-delta to the Branch-C emit gate** so token/model updates
   broadcast without needing a status change.
3. **Match project dirs encoded↔encoded** (carry the raw encoded dir through
   and compare against re-encoded known repo paths) instead of the naive
   lossy decode.

Kept as-is (faithful): thinking-only→done, subagent-change re-emit refreshing
terminal-prune timers, the 4-status vocabulary {running, done, question,
idle} with `waiting` remaining a server/bridge-side overlay.
