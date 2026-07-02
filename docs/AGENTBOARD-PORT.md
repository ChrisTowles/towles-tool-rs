# Agentboard → Tauri Port Plan

Slice 7 of [MIGRATION.md](MIGRATION.md). Source of truth: slot-1
`packages/agentboard/src` (~10k LOC TS; live entry `src/server/main.ts`) plus
`src/commands/agentboard.ts`. Inventory taken 2026-07-02.

## What the TS agentboard is

A localhost HTTP+WS server on `127.0.0.1:4201` (Bun) plus OpenTUI/SolidJS
terminal panes as clients, choreographed through tmux (sidebar pane, stash
session, tmux hooks POSTing to the server). The *data engine* underneath:

- **AgentTracker** (`runtime/agents/tracker.ts`, 299) — in-memory agent
  instance state machine + pruning, unseen counts, pins.
- **Agent watchers** (`runtime/agents/watchers/`) — claude-code (659; JSONL
  incremental reads with fileSize offsets, status from message roles/tool_use,
  usage tokens, subagents, /loop wakeups, liveness via
  `~/.claude/sessions/<pid>.json`), codex (370; sqlite), amp (320),
  opencode (225; sqlite polling). fs.watch + poll fallbacks.
- **git-info** (`server/git-info.ts`, 295) — branch, worktree detect,
  porcelain status, numstat vs merge-base, ahead/behind; 5s cache, 1.5s poll,
  fs watch on `.git/HEAD`.
- **port-scanner** (`server/port-scanner.ts`, 151) — ps tree BFS + single
  `lsof -iTCP -sTCP:LISTEN` attributed per session, 10s poll.
- **SessionMetadataStore** (83) + agent-facing HTTP API
  (`/set-status /set-progress /log /clear-log`) so external agents/scripts
  report progress.
- **session-order** (100) — custom order persisted to
  `~/.config/towles-tool/agentboard/session-order.json`.
- **shared types** (`runtime/shared.ts`, 185) — SessionData, ServerMessage
  (`state|session-viewed|re-identify|quit`), ClientCommand set.
- **themes** (`runtime/themes.ts`, 769) — Catppuccin palette variants, pure data.
- Config lives under the `agentboard` key of the shared
  `towles-tool.settings.json` (mux/port/theme/sidebarWidth/sidebarPosition/
  keybinding) + top-level `preferredEditor`. **No repoPaths config and no DB**:
  repos are discovered live from tmux session panes.

## What does NOT port

- All tmux sidebar/window orchestration (~900 LOC of `server/index.ts`):
  sidebar spawn/stash/join-pane, tmux hooks, width sync, popup wiring — the
  Tauri window replaces tmux's role entirely.
- The OpenTUI/SolidJS terminal renderer — UI is rebuilt in the existing
  React 19 client.
- The MuxProvider capability-trait layer (`contracts/mux.ts`,
  `mux/registry.ts`, `plugins/loader.ts`) — speculative abstraction; tmux was
  the only provider ever implemented. Collapse it.
- Stale trees: `plugins/tt-agentboard`, `packages/agentboard/apps/*`.

## Phases

1. **`crates/tt-agentboard` core engine (Tauri-free).** ✅ **DONE (2026-07-02).**
   Shared types, AgentTracker + pruning, SessionMetadataStore, session-order,
   git-info, port-scanner. Process spawns via `tt-exec`. Fully unit-tested
   (55 tests); no tmux, no UI, no transport. Modules: `types`, `tracker`,
   `metadata`, `session_order`, `git_info`, `ports`, `text`.
   Deviations / transport cuts (transport & watchers deferred to phase 3):
     - Clock is injected as an explicit `now_ms` param (the `tt-graph` pattern)
       instead of `Date.now()`, so prunes/caches/timestamps are deterministic.
     - Cut from each source, logic kept: `startGitPoll`/`startPortPoll`/`poll.ts`
       (the `setInterval` loops), `syncGitWatchers` (`fs.watch` on `.git/HEAD`),
       and all WS-broadcast callbacks. git-info cache misses compute
       *synchronously* rather than TS's async background refresh + in-flight
       de-dup; `refresh` is exposed for the future poll loop to drive.
     - `tmux`/`ps`/`lsof`/`git` calls are isolated in thin, un-unit-tested
       functions; the ps-tree/lsof attribution and porcelain/numstat/ahead-behind
       parsing are pure and tested on fixture strings.
     - Insertion-order-sensitive structures use `indexmap` (JS `Map`/`Set`
       semantics that `AgentTracker::get_state`'s tie-break relies on).
     - `SessionOrder::sync` orders with Rust's lexicographic `Ord`, not JS
       `localeCompare` (matches for ASCII session names). `truncate` counts
       Unicode scalars, not UTF-16 units (matches for BMP text).
     - Env-derived `SERVER_PORT`/`SERVER_HOST` (transport config) not ported;
       the `DEFAULT_*` + timeout constants are.
2. **AgentWatcher trait + claude-code watcher.** ✅ **DONE (2026-07-02).**
   `watcher` module (`AgentWatcher`/`WatcherContext` traits, constants) +
   `watchers/{claude_code, claude_usage, claude_pid}` + isolated `fs_notify`
   accelerant. Ported per docs/AGENTBOARD-WATCHER-SPEC.md with all three adopted
   fixes; 86 unit tests (55 pre-existing + 31 new). codex/amp/opencode later.
   Deviations / decisions:
     - **Scan is driven externally**: the `AgentWatcher` trait exposes
       `scan(ctx, now_ms)` instead of TS `start`/`stop` + `setInterval`. The
       bridge owns scheduling; no tokio in the crate. `now_ms` is injected
       everywhere the TS read `Date.now()`, so the watcher is deterministic and
       unit-testable without timers. The `notify` fs-watch is an optional,
       isolated accelerant (`fs_notify::DirNotifier`), not part of the scan core;
       the TS per-dir `fs.watch`/`setupWatchers`/lazy-watch wiring is not ported.
     - **Adopted fix #1** (offset at last-newline boundary + re-read tail;
       shrink → reset offset 0 & re-seed), **#2** (usage delta added to the
       Branch-C emit gate), **#3** (encoded↔encoded project-dir match — the raw
       encoded dir is carried through to `resolve_session`; the lossy
       `decodeProjectDir` is dropped). Kept faithful: §3 status table (incl.
       thinking-only→done), 4-status vocabulary {running,done,question,idle}
       (`waiting`/`error`/`interrupted` remain server/other-watcher concerns),
       subagent 2-min window + order-independent signature, pid-liveness
       demotions, seed/emit sites, subagent-change re-emit.
     - Default pid liveness uses Linux `/proc/<pid>` (deployment target) rather
       than `kill(pid, 0)`; `is_alive` is an injectable `fn(i32)->bool` so tests
       and other platforms substitute their own probe. Timestamps parse via
       chrono RFC3339 (`Date.parse` equivalent for the ISO-8601 journal stamps).
       The `scanning` re-entrancy guard is unneeded (synchronous scan) and dropped.
       `thread_name` capped at 80 Unicode scalars (not UTF-16 units).
     - New deps: `notify` (fs accelerant, isolated) and `chrono` (timestamp parse).
3. **Tauri bridge + minimal React UI.** ✅ **Rust half DONE (2026-07-02)** (React
   UI handled separately). Ported the composition/broadcast half of
   `server/index.ts` per docs/AGENTBOARD-BRIDGE-SPEC.md. Pure snapshot assembly
   (`bridge::assemble_state` + `synthesize_waiting`/`merge_agents_waiting`) lives
   in `tt-agentboard` (unit-tested); `crates-tauri/tt-app/src/agentboard.rs` owns
   the `Engine` (tracker + metadata + session-order + git cache + claude-code
   watcher; ports left unused per BRIDGE-SPEC) behind a `Mutex` and the tokio
   tasks (tauri runtime): watcher scan every 2s (+ eager on the `notify`
   accelerant), git refresh every 1.5s, and a 200ms-debounced emitter. Each
   rebuild pins by pid-liveness → runs the §4 prune schedule → assembles the
   trimmed snapshot → emits the `agentboard://state` Tauri event with
   `{sessions, theme, preferredEditor, ts}`.
   Tauri commands (registered in `lib.rs`): `ab_get_state`, `ab_mark_seen`
   (fast-path: patch `unseen` on the cached snapshot + re-emit, no rebuild),
   `ab_dismiss_agent`, `ab_reorder_session`, `ab_set_theme` (persists to shared
   settings `agentboard.theme` via tt-config), `ab_add_repo`, `ab_remove_repo`,
   `ab_refresh`, and the four metadata mutations `ab_set_status`/`ab_set_progress`
   /`ab_log`/`ab_clear_log` (§5 validation: non-empty session/message, tone
   whitelist→undefined, caps enforced by the store). The old `greet` command was
   removed (hard cutover). `apps/client` untouched (frontend agent owns it).
   Deviations: `waiting` synthesis + prune pinning driven by pid-liveness, not
   pane presence (§6); dropped payload/`SessionData` fields (`sidebarWidth`,
   `createdAt`, `panes`, `windows`, `uptime`, `isWorktree`, `ports`,
   `eventTimestamps`) carry default/zero values in the snapshot; the git poll
   holds the engine lock across git subprocesses (brief, acceptable for the poll);
   dropped tmux routing commands and `session-viewed`/`re-identify`/`resize`
   messages. New tt-app deps: `tt-agentboard`, `tt-config`, `tokio`
   (time/sync/rt/macros), `dirs`.
4. **Repo source config.** ✅ **DONE (2026-07-02).** `repos` module in
   `tt-agentboard`: repo-path list persisted to its own
   `~/.config/towles-tool/agentboard/repos.json` (`{"repoPaths":[...]}`) — NOT the
   shared settings file (the TS CLI's zod round-trip could strip unknown keys;
   sits beside `session-order.json`). Path-parameterized load/save + add/remove
   helpers; session name = dir basename, disambiguated by the parent-dir basename
   on collision; `resolve_session_name` matches Claude's encoded project dir
   encoded↔encoded (adopted fix #3). Wired to `ab_add_repo`/`ab_remove_repo`
   (remove drops the name → next `computeState`/`pruneSessions` cleans metadata).
5. **Parity extras.** ⏳ **Partially DONE (2026-07-02).**
   - ✅ **Localhost metadata HTTP ingest.** `metadata_http` module in
     `tt-agentboard` (pure request-head parse + §5 validation → `MetadataMutation`
     + status/body, unit-tested on raw strings); `tt-app` binds a hand-rolled
     HTTP/1.1 responder (tokio `TcpListener`, no HTTP-framework dep) on
     `TT_AGENTBOARD_HOST:TT_AGENTBOARD_PORT` (default `127.0.0.1:4201`) with
     POST `/set-status` `/set-progress` `/log` `/clear-log` (204 ok; 400 on
     invalid JSON / empty session / non-string text / empty message; tone
     whitelist→none; caps via the store) and `GET /` health route list.
     Port-in-use → warn + run without the listener (never crashes if the TS
     server is up). Mutations feed the same engine + debounced emit.
   - ✅ **`ttr agentboard` CLI (alias `ag`).** `repos` (list), `repos add <path>`
     (canonicalizes, requires an existing dir, warns if not a git repo but still
     adds), `repos remove <name-or-path>`. Operates on the same `repos.json`; the
     engine now re-reads `repos.json` on every scan/rebuild (was cached at
     startup) so CLI edits are picked up live without an app restart.
   - ⏳ **Still pending** (next, decision after the e2e demo): the codex / amp /
     opencode watchers and open-in-editor.
