# CLAUDE.md — crates-tauri/tt-app

Tauri 2 desktop shell — see the root [`CLAUDE.md`](../../CLAUDE.md) for what
this crate is (identifier, dev-port picking, slot labeling). This file is
the internal invariants a single read of the code won't surface: it's the
largest crate in the repo (~6,100 lines / 20 modules), and most of what
follows is a cross-cutting rule that spans multiple files.

## Locking and ordering

- **Never hold the agentboard `Engine` lock across a git subprocess.**
  `lib.rs`'s scan task and stat-poll both do git work *outside*
  `engine.lock()` deliberately — on Linux, sync Tauri commands dispatch
  inline on the GTK main thread, so a lock held across a `git` chain would
  freeze every `ab_*` command, not just the caller.
- **Every `StatePayload` leaving the app must pass through
  `stamp_pty_state`** (`agentboard.rs`). The Tauri-free engine can't see
  PTYs, so a new command that builds/returns a `StatePayload` without this
  stamp silently reports stale `live`/`shellKind`/needs-you counts.
- **PTY replacement is generation-checked** (`terminal.rs`), so a stale EOF
  from a killed/replaced session can never close its successor. Treat
  `TermState`'s lock as map-surgery-only — don't hold it across anything
  that can block.
- **`slot_remove` kills a folder's PTYs before touching its worktree on
  disk** (`slots.rs`) — do the same ordering for any new slot-mutating
  command, or you'll orphan a shell pointed at a deleted cwd.
- **Task-status mutations must route through `spawn_gh_status_sync`**
  (`store.rs`) — the single call site for gh close/reopen, added after a
  real drift bug (#246). Don't add a second path that flips status without
  it.
- **`ab_save_windows`/`ab_save_collapsed` deliberately skip re-emitting
  state** (`agentboard.rs`), unlike every other mutator, to avoid
  clobbering rapid client-side edits. Match this if you add another
  purely-client-authoritative setter.

## Singletons and cross-slot state

- **`InstanceLock` is a shared, cross-slot singleton**, not per-checkout
  (`instance_lock.rs`) — it gates Slack Socket Mode, since settings/tokens
  are machine-wide. Without it, every open worktree slot would open a
  duplicate websocket on the same token.
- **Nested shells get their env scrubbed and re-stamped** (`terminal.rs`,
  issue #39): a `tt-app` or `npm run dev` launched *inside* an embedded
  terminal doesn't collide with the outer instance's port/session identity.
  `CLAUDE_CODE_SSE_PORT` is re-stamped for deterministic IDE pairing even
  with several slots open — don't strip this scrubbing step to "simplify"
  terminal spawning.
- **The scheduler's watchers/in-flight guards persist across a
  settings-reload rebuild** (`scheduler.rs`), and a failed `claude:calendar`
  run still counts as "recent" — this avoids re-billing tokens on relaunch.

## IDE bridge

- **The IDE server serves multiple concurrent connections per terminal**
  (`ide.rs`) — a Claude Code ≥2.1 session is a TUI process *and* a session
  daemon, both dialing in.
- **`openDiff` replies are deferred through a channel that auto-rejects on
  drop**, so a torn-down pane can never hang the CLI waiting on a review
  decision.

## Misc

- **OSC 52 clipboard writes are gated on terminal focus** (`terminal.rs`) —
  a background agent pane can't hijack the system clipboard.
- The `WEBKIT_DISABLE_DMABUF_RENDERER` env var (`lib.rs`, Linux-only) works
  around a WebKitGTK/NVIDIA rendering bug (tauri-apps/tauri#9304) — only set
  when NVIDIA is actually driving the screen, and never override an
  explicit user setting.
- **Linux app-id / desktop-entry self-registration** (`linux_desktop.rs`):
  `tauri build`'s packaging step normally writes a `.desktop` file + themed
  icon so GNOME/COSMIC can resolve a running window's Wayland `app_id` to
  the right dock icon — but the daily-driver flow (`npm run run`) runs
  `tauri build --no-bundle` and execs the raw binary, skipping packaging
  entirely. `linux_desktop::ensure_installed` (called from `.setup()`)
  self-registers both into `~/.local/share/{applications,icons}` on every
  startup instead, idempotently. This pairs with `enableGTKAppId: true` in
  `tauri.conf.json`, which is what actually sets the running window's
  app-id — without it the desktop entry has nothing to match against.
