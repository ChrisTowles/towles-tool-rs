# Agentboard tmux mode — port spec

Decision (Chris, 2026-07-03): the Tauri app stays (graph + future features),
but **agentboard must be served inside tmux to be useful** — the desktop-only
plan in [AGENTBOARD-PORT.md](AGENTBOARD-PORT.md) ("tmux does not port") is
amended. This spec scopes the tmux mode: a Rust server + **ratatui** sidebar
TUI, reusing the already-ported `tt-agentboard` data engine.

Sources of truth (slot-1 `packages/agentboard/src` + `src/commands/agentboard.ts`):

| TS source | LOC | Ports to |
|---|---|---|
| `mux-tmux/client.ts` | 543 | `tt-agentboard::tmux::client` — tmux subprocess wrapper (via `tt-exec`), tab-delimited `-F` format parsing (pure, fixture-tested) |
| `mux-tmux/provider.ts` | 270 | `tt-agentboard::tmux::provider` — sidebar spawn/stash/restore (`_ab_stash` session, `agentboard-sidebar` pane title), hide via `join-pane -d` into stash, hooks setup/cleanup |
| `runtime/server/index.ts` | 1827 | split: orchestration handlers → `tt-agentboard::sidebar` (pure parts) + `tt-cli` server module; WS command set → HTTP command POSTs; pane↔agent attribution → phase T6 |
| `runtime/server/sidebar-width-sync.ts` | 100 | `tt-agentboard::sidebar_width_sync` — pure, direct port (750ms cooldown drag-vs-rescale disambiguation) |
| `runtime/server/launcher.ts` | 61 | `tt-cli` `ensure_server()` — PID file + port probe; **spawn via `std::env::current_exe()`**, never a hardcoded binary name (fixes the TS `spawn("tt",…)` cutover gotcha) |
| `runtime/themes.ts` | 769 | `tt-agentboard::themes` — Catppuccin palette data, mechanical |
| `tui/` (index.tsx 797 + components ~640) | ~1500 | `tt-cli` ratatui TUI (`ttr agentboard tui`) |
| `src/commands/agentboard.ts` | 578 | `tt-cli` subcommands `setup/uninstall/init/server/tui/restart/run/keys` (existing `repos` kept) |

## Architecture

One long-lived server process (`ttr agentboard server`, default
`127.0.0.1:4201`, PID file `/tmp/agentboard.pid`) owns the engine — tracker,
watchers, git-info, metadata, repos — extracted from `tt-app` into
`tt-agentboard::engine` so both hosts share it (`crates/` stays Tauri-free;
tokio is allowed). State fan-out via a `tokio::sync::watch`-style channel:

- `tt-app` subscribes → re-emits `agentboard://state` Tauri events (unchanged UI).
- The tmux server subscribes → pushes to TUI clients.

**Transport: SSE + POST, no WebSocket.** The server extends the existing
hand-rolled HTTP/1.1 style (no framework dep): `GET /events` streams
`data: <ServerState JSON>\n\n`; TUI client commands are `POST /command`
(`{"command":"switch-session",...}` — the TS `ClientCommand` vocabulary minus
`identify-pane`/`report-width` WS plumbing where obsolete). tmux hooks POST
exactly as in TS (curl, `#{q:...}`-escaped pipe-delimited bodies).

Routes: existing 4 metadata routes + `GET /` health + `GET /events` +
`POST /focus /refresh /resize-sidebars /ensure-sidebar /toggle /switch-index
/quit /shutdown /command`.

Desktop app and tmux server both binding 4201: port-in-use → warn + continue
(existing behavior); the tmux server is the primary owner once in use.

tmux floor: 3.x (`join-pane -f`, `#{q:...}`, `display-popup -E`). `$TMUX`
required for `tui`/`init`/`run`; the server itself must NOT require it (fixes
a TS pain point) — sidebar orchestration just no-ops without tmux sessions.

## Phases

1. **T1 — tmux client + provider** in `tt-agentboard` (new `tmux` module).
   Command construction + output parsing pure and fixture-tested; subprocess
   calls isolated thin. Provider: `spawn_sidebar` (edge-pane split or stash
   restore; no select-pane after spawn — terminal-capability-response leak),
   `hide_sidebar` (resize stash window 200x200 first), `kill/resize_sidebar_pane`,
   `list_sidebar_panes` (+windowWidth), `setup_hooks`/`cleanup_hooks` (the 7
   global hooks), session list/switch/create/kill, `display_popup`.
2. **T2 — themes** (`themes.rs`, pure data) + `sidebar_width_sync.rs` (pure).
3. **T3 — engine extraction + server.** Move `Engine` + scan/git/debounce
   scheduling from `tt-app/src/agentboard.rs` into `tt-agentboard::engine`
   (state watch channel; hosts own their emit). Rewire `tt-app` (behavior
   unchanged). New `ttr agentboard server`: tokio HTTP with all routes,
   sidebar orchestration (`ensure_sidebar_in_window`, `toggle_sidebar`,
   `resize_sidebars` + width-sync, debounces), PID file, SIGINT/SIGTERM
   cleanup (unset hooks, stash-or-keep sidebars per TS `cleanup()`).
4. **T4 — ratatui TUI** (`ttr agentboard tui`): SSE subscriber thread →
   event loop; session list + SessionCard (name, status icons, agent lines,
   model/tool, diff stats, unseen, progress/log) + StatusBar; keys
   `Tab j k ↑ ↓ Enter l 1-9 d x r ? q`; sessionizer via `display-popup -E`
   (port `tui/scripts/sessionizer.sh`); `REFOCUS_WINDOW` refocus after start.
5. **T5 — CLI wiring**: `setup`/`uninstall` (tmux.conf `run-shell 'ttr
   agentboard init'` line, TPM-aware insertion, `# agentboard` marker),
   `init` (set-environment, `@agentboard-key` prefix table binds, digits 1-9,
   the 7 hooks), `restart` (kill `_ab_stash*`, stop via PID file or POST
   /shutdown, ensure up, POST /refresh + /ensure-sidebar per client),
   `run --toggle|--focus`, `keys`. No `ensureBun` equivalent needed.
6. **T6 — pane↔agent attribution** (~600 LOC of server/index.ts):
   `scan_all_tmux_pane_agents` (single `list-panes -a` + ps-tree), per-watcher
   pane resolution (claude/codex/opencode/amp), pane-presence merge into
   state, `focus-agent-pane`/`kill-agent-pane` commands, pane highlight,
   `ports` column finally attributable. Sidebar is useful without this;
   ship T1–T5 first.

## Deviations from TS (decided up front)

- SSE+POST replaces WebSocket (simpler to hand-roll, same one-way state flow).
- Renderer is ratatui/crossterm, not OpenTUI/SolidJS.
- MuxProvider capability-trait layer stays collapsed (per AGENTBOARD-PORT.md);
  tmux is wired directly.
- Launcher spawns `current_exe()`, not `"tt"` from PATH.
- Server runs without `$TMUX` (degrades to engine+metadata only).
- Clock injection (`now_ms`) continues everywhere in pure code.
