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

1. **`crates/tt-agentboard` core engine (Tauri-free).** Shared types,
   AgentTracker + pruning, SessionMetadataStore, session-order, git-info,
   port-scanner. Process spawns via std/tokio. Fully unit-tested; no tmux,
   no UI.
2. **AgentWatcher trait + claude-code watcher.** Trait feeding AgentEvents
   into the tracker; claude-code implementation first (`notify` + poll
   fallback, incremental JSONL, `~/.claude` parsing). codex/amp/opencode
   later (rusqlite where needed).
3. **Tauri bridge + minimal React UI.** WS `state` broadcast becomes a Tauri
   event carrying the SessionData snapshot; ClientCommands become Tauri
   commands. React renders StatusBar + SessionCard list; themes.ts carried
   over as data. **Demo milestone: live repo list with git stats and
   claude-code agent status updating in the Tauri window.**
4. **Repo source config.** Explicit `repoPaths` (new config, replacing tmux
   session discovery as the desktop repo source).
5. **Parity extras.** Localhost HTTP metadata ingest (external agents keep
   POSTing status/progress/log), remaining watchers, open-in-editor,
   CLI `agentboard` command to launch/manage the app.
