# Implementation notes — tt-app data hub + screens

> **Superseded in part by the day-screens pivot (2026-07-04, same day).**
> Email was removed entirely (collector, `emails` table, MCP
> `email_needs_reply`, the Email + Calendar screen). Calendar is now
> next-meeting-only. `tasks` became a local kanban (`status`/`position` +
> optional issue link; no `done` bool). New `issues` collector; collector keys
> are `claude:calendar`, `issues`, `prs`. Collectors are config-driven
> (`settings.collectors`, calendar `provider` = google|outlook). New screens
> **Cockpit** (day home) + **Board** (kanban); Agentboard unchanged. MCP tool
> `email_needs_reply` → `issues_open`. Store commands `store_set_task_done`/
> `store_archive_email` → `store_set_task_status`/`store_promote_task_to_issue`.
> The notes below describe the pre-pivot build; read them with that in mind.

Working notes for the 2026-07-04 build-out (plan:
https://claude.ai/code/artifact/ — "tt-app build plan"). Direction: Agentboard
(attention inbox) + Email/Calendar screen + day bar in the Tauri app, backed by
a SQLite store (`tt-store`), collectors (`ttr collect`), and an MCP server
(`ttr mcp serve`) so claude sessions can query the same data.

Ground rules carried in from the session:

- Agent status is **read-only** in the UI — never re-render an agent's TUI;
  interaction = real PTY (xterm.js passthrough).
- Collectors are the only writers to tt.db; UI + MCP only read
  (exceptions: task add/done, email archive, journal append — deliberate).
- Timestamps epoch ms, injected `now_ms` (no clock reads in library logic).
- `crates/` stays Tauri-free.

## Progress log

- (start) Plan published; scaffolding beginning.
- P1 launched: `store` agent (tt-store crate + tt-journal::append_to_daily) and
  `frontend` agent (data.ts contract, day bar, email-calendar screen,
  agentboard rework with split terminals, ⌘J quick log) running in parallel
  against the D5 contract.
- Frontend landed (build verified green): new `lib/data.ts`, `day-bar.tsx`,
  `screens/email-calendar.tsx`, `quick-log.tsx`; agentboard reworked to
  needs-you feed + resizable split terminals (max 4 panes, ⌘D). **Contract for
  the Rust side:** commands `store_snapshot`, `store_add_task{text,dueTs?}`,
  `store_set_task_done{id,done}`, `store_archive_email{id}`,
  `journal_log{text}`; event `store://snapshot` (StoreSnapshot payload);
  collect_runs collector keys must be `claude:calendar`, `claude:email`,
  `claude:tasks`, `prs` (day-bar freshness matches `startsWith("claude")`).

- Store landed (verified: tt-store 22 tests, tt-journal 10 tests green):
  `crates/tt-store` full API per plan D1/D5; `tt_journal::entries::append_to_daily`.
- Coordinator added directly: `tt_exec::run_with_stdin` (piped-stdin runner) and
  `tt-config` `assistant` block ({ enabled, claudeRefreshMinutes,
  prRefreshSeconds }) — both tested green.
- P2 launched in parallel: `collect` (crates/tt-collect + `ttr collect`),
  `mcp` (crates/tt-mcp stdio JSON-RPC server, 8 tools), `tauri` (tt-app
  store_*/journal_log commands + store://snapshot event). File ownership kept
  disjoint per agent.

- tt-app command layer landed (agent gate green: clippy, 9 tests, build):
  store_snapshot/store_add_task/store_set_task_done/store_archive_email/
  journal_log + `store://snapshot` emitted on setup and after every write.
  Store-open failure degrades gracefully (commands Err, journal_log still
  works). Scheduler hook point marked in lib.rs setup.

- tt-mcp landed (verified 17 tests): Dispatcher::handle_at + serve(), 8 tools,
  newline-delimited JSON-RPC. CLI must init env_logger (library doesn't).
- Coordinator P3 (partial): tt-store now opens with WAL + 5s busy_timeout
  (several processes share tt.db); tt-app `scheduler.rs` spawns the collector
  loop — PRs every prRefreshSeconds (min 30s), claude batch every
  claudeRefreshMinutes gated by assistant.enabled, each batch on its own
  connection in spawn_blocking, snapshot re-emitted after every batch.

- `ttr collect` landed (agent gate green: 13 unit + 2 black-box tests) and
  `ttr mcp serve` wired by the coordinator with 3 black-box handshake tests
  (real stdio initialize → tools/list → tools/call against the built binary).
- **Final gate (all green):** `cargo fmt --check`, `cargo clippy --all
  --all-targets -D warnings`, `cargo test --all` (31 suites, 0 failures),
  `npm run build`.
- **Live verification:** `ttr collect all` ran real `claude -p` (25 emails
  triaged, 7 tasks extracted, calendar empty on the holiday) and recorded
  freshness rows; `gh` path verified against this repo (returns PR #5);
  `npm run dev` boots the app — vite + tt-app compile, window process stable,
  scheduler's token guard correctly skipped the just-run claude batch.
- Live-run fix: stale tracked-repo dirs (repos.json still lists
  `towles-tool-rs` and `towles-tool-slot-1`, which no longer exist) made the
  PR collector fail with a misleading spawn error — tt-collect now skips
  missing dirs and reports them in the run message.

## Drive-test findings (2026-07-04, real window + browser interaction pass)

Verified working: day bar with real extracted tasks; Email + Calendar renders
the real triaged inbox ("via claude -p · Nm ago" freshness, tags, summaries,
extracted tasks with due chips); Agentboard needs-you feed ranks failing PR >
review-requested > upcoming meeting with colored urgency bars; task
toggle/email archive apply optimistically; ⌘K palette → "Journal: log a line"
→ ⌘J dialog → toast, all clean; browser mode falls back to mock with a
"browser" badge in the status bar.

Fixed during the pass:
- `lib/agentboard.ts` and `terminal-view.tsx` called Tauri `listen()`
  unguarded → 4+ uncaught exceptions per load in bare-browser dev. Both now
  early-return outside Tauri (terminal pane prints a "requires the desktop
  app" note). data.ts was already guarded.

Known follow-ups (not fixed, by design or deferred):
- The old Pull requests screen still renders static mock PRs while the store
  holds the real set — two sources of truth; should be wired to tt.db or
  folded into the agentboard feed.
- Agentboard "Quiet" list shows stale tracked repos (missing dirs) as ghost
  sessions and misses untracked-but-active ones; needs a missing-dir flag in
  the engine/UI and a "track this repo" affordance.
- Day-bar top task doesn't reflect a screen's optimistic overlay until the
  next live snapshot (browser-only inconsistency; round-trips fine in Tauri).
- Empty-day schedule column is mostly whitespace; could show the week ahead.

## Environment notes for testing

- `~/.config/tt`-side agentboard repos.json has two stale entries; re-point
  with `ttr agentboard repos add <dir>` so the PR collector has real repos.
- First app launch may show a stale page if another slot previously served
  localhost on the same port (WebKitGTK cache) — reload the window once.
- Metadata ingest port 4201 was busy (another slot's server); the app degrades
  gracefully and logs it.
- Register the MCP server with: `claude mcp add tt -- ttr mcp serve`.

## Deviations

- Scheduler token guard (not in plan): the claude batch is skipped when the
  newest successful `claude:*` run is younger than half the refresh period —
  otherwise every app relaunch would immediately re-bill `claude -p` for data
  we already have. Conservative: stale-but-recent data over surprise spend.
- Collector logic lives in a shared `crates/tt-collect` instead of inside
  tt-cli (plan said tt-cli): both the CLI and the app's Phase-3 scheduler need
  it, and app-shelling-out-to-ttr would break in dev when the binary isn't on
  PATH. Conservative: same behavior, better home.
- The app's collector scheduler moved from the `tauri` agent to Phase 3
  (coordinator) so the three P2 agents stay compile-independent.
- Frontend: ⌘J dialog opens via a window CustomEvent ("quicklog:open") instead
  of workspace-context state — avoids threading dialog state; conservative,
  UI-local.
- Frontend: task/email writes render as optimistic local overlays that reset
  when the next live snapshot arrives (snapshot stays the single source of
  truth).
- Frontend agent had to `npm install` in apps/client (node_modules was missing
  the declared @xterm deps in this worktree — pre-existing, unrelated to the
  feature).

---

# Agentboard → Folder Rail (2026-07-04, `feat/agentboard-folder-rail`)

Plan of record: `~/.claude/plans/federated-plotting-moonbeam.md`.

Model in one line: **folder (repo) → 1..N sessions; a session is an xterm PTY;
"agent" is a badge on a session where Claude is detected running.** Agent↔session
link = `TT_SESSION_ID` env injected at PTY spawn, read back from the agent's
`/proc/<pid>/environ` (Chris's idea — replaces fragile pid/cwd matching).

This inverts the data model: today's `SessionData` *is* a repo (one per repo);
it becomes `FolderData` that *contains* `Vec<SessionData>` (each = one PTY).

## Progress log (Folder Rail)

- Setup: branch, appended these notes, HTML plan artifact rendered.
- Rust model (task 2, green): `RepoData`/`FolderData`/new `SessionData` in
  `types.rs` (old one → `TmuxSessionData`); `GitInfo.origin_url`; new
  `sessions.rs` (`SessionStore`, `sessions.json`); `bridge.rs` assembles folders
  → repos grouped by origin URL; `engine.rs` seeds default shells + prunes;
  `tt-mcp` `agent_sessions` flattens the tree. 109 tt-agentboard tests pass.
- Agent↔session linkage (task 3, green): `terminal.rs` sets `TT_SESSION_ID`
  env; new `procenv.rs` reads it back from `/proc/<pid>/environ`; engine joins
  the CLI snapshot's pid to the tracked event via thread_id(==sessionId) and
  attributes the agent to its session.
- Session commands (task 4, green): engine `add/rename/close_session`; Tauri
  `ab_add_session`/`ab_rename_session`/`ab_close_session` registered; CLI
  `agentboard sessions add|rename|remove|list`. 12 CLI black-box tests pass.
- Frontend (tasks 5+6, green): `agentboard.ts` new `RepoData`/`FolderData`/
  `SessionData` + `isAgent`/`sessionNeeds`/`sessionStatusText`/`isSoloRepo`
  helpers + browser-dev mock; `day-bar.tsx` uses `repos[].needs`;
  `agentboard.tsx` rewritten as the Folder Rail (repo→folder→session tree,
  solo-repo collapse, compact PR/calendar strip, tabbed right pane, ⌘D new /
  ⌘W close / dbl-click rename, hidden-mount PTYs); `screens.ts` keywords.
- **Verified (task 7):** `cargo fmt --check` + `cargo clippy --all` clean;
  `cargo test --all` fully green; frontend `tsc -b && vite build` clean; live
  browser render (mock) confirms the 3-level rail — towles-tool-rs (slot-0 +
  slot-1 folders) grouped by origin, toolbox solo-collapsed, ✦/❯ glyphs, status
  dots, needs ⚑ bubbling, attention strip, selection + tabbed right pane.

## Deviations (Folder Rail)

_Conservative option on edge cases; log what/why/impact and keep going._

- Kept the existing `implementation-notes.md` (day-hub build log) and appended
  this section instead of overwriting — the plan said "create" but a tracked
  file already existed. Conservative: preserve history.
- **Plan revised before coding (Chris):** structure is 3-level **Repo → Folder
  → Session**, not flat folders. A repo can be checked out N times (worktrees /
  slot clones); group folders by `git remote get-url origin` (no remote →
  standalone). Solo-repo folders collapse to one header. `GitInfo` gains
  `origin_url`; new `RepoData` wraps `Vec<FolderData>`. Old one-per-repo
  `SessionData` + tmux `ServerMessage`/`ClientCommand` are tt-agentboard-internal
  only → add new types, leave tmux code alone.
- **`running` field dropped from the wire** (plan listed it on SessionData).
  PTY liveness is owned by tt-app's `terminal.rs`, and threading a live-term-id
  set from tt-app through the Tauri-free engine would break the crate boundary.
  Conservative: the frontend derives "running" from its own mounted-PTY set.
- **Agent→session attribution done in the engine, not on `AgentEvent`.** Adding
  a field to `AgentEvent` would touch 12 construction sites; instead the engine
  joins the live CLI snapshot's pid to the tracked event by `thread_id`
  (== CLI `sessionId`) and reads `/proc`. No `AgentEvent` change.
- **Non-Claude agents (amp/codex/opencode) attach to a folder's default
  session**, not pid-linked — only the Claude CLI snapshot exposes pids to read
  `TT_SESSION_ID` from. Acceptable for v1; multi-session folders running those
  agents won't pinpoint the exact PTY.
- **`SessionOrder` no longer applied in assembly** (was a tmux reorder feature).
  The field/commands remain but don't affect the Folder Rail ordering (repos +
  folders sort by name). Conservative: leave the machinery, drop the effect.
- **`mark_seen_patch` now full-recomputes** instead of patching the cached
  snapshot in place (unseen moved from folder to session granularity).

## Open follow-ups (Folder Rail)

- macOS: `read_process_env(pid)` needs a non-`/proc` path (`ps eww`/libproc).
  Linux-only for this pass.


## Post-review tweaks ("this doesn't look different", Chris)

Chris tested the running app and it read as a flat repo list. Root cause: all his
real repos were single checkouts (solo-collapse hides the Repo→Folder nesting),
and agent sessions showed as a bare "shell 1" so the agent's identity was
invisible. Two changes, both requested:

- **A — agent rows lead with the agent's task name.** New `sessionLabel(s)` in
  `lib/agentboard.ts`: when `agentState.threadName` is set (the Claude watcher
  already populates it — `claude_code.rs:554`, `bridge.rs:build_folder` preserves
  it through into `agent_state`), the row/tab leads with that thread name and
  tags the underlying `shell N` as a dim mono secondary; otherwise it's just the
  shell name. No Rust change needed — the name already flowed on the wire, the UI
  just wasn't surfacing it. `MOCK_STATE` updated so browser dev shows the new
  shape (agent sessions are `shell 1` + a `threadName`).
- **B — demonstrate the nesting with real data.** Added this checkout
  (`towles-tool-rs-slot-1`) to `repos.json` alongside the already-watched
  `slot-0`; both share origin `github.com/ChrisTowles/towles-tool-rs.git`, so the
  app now groups them under one `towles-tool-rs` repo header with two folder
  sub-headers (`…-slot-0`, `…-slot-1`) — the multi-checkout path the solo repos
  never exercised. Live-picked-up (no restart); reversible via
  `ttr agentboard repos remove`.

## Solo repos visually distinct from folders (Chris)

Solo-collapsed repo headers and folder (checkout) sub-headers both rendered as
`chevron + name + branch`, so they blurred together. Added a structural level
icon (gray = structure, per the folder-rail-ui skill §1): repo headers (solo +
multi) lead with a `FolderGit2` glyph + semibold full-contrast name; folder
sub-headers lead with a dimmer `Folder` glyph + muted name (on top of the
existing `pl-6` indent). Now a solo repo reads unmistakably as a *repo*, and
nested checkouts read as its children. Documented the convention in the
`folder-rail-ui` skill (mental model updated to the 3-level Repo→Folder→Session).
