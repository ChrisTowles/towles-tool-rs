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
  disk — but only once the removal guards have passed** (via
  `ops::remove_slot`'s `before_removal` hook, `slots.rs`). Both halves are
  load-bearing for any new slot-mutating command: kill before deleting or
  you'll orphan a shell pointed at a deleted cwd; kill only past the guards
  or a *refused* removal costs a live session the guard existed to protect.
- **Task-status mutations must route through `spawn_gh_status_sync`**
  (`store.rs`) — the single call site for gh close/reopen, added after a
  real drift bug (#246). Don't add a second path that flips status without
  it.
- **`ab_save_windows`/`ab_save_collapsed` deliberately skip re-emitting
  state** (`agentboard.rs`), unlike every other mutator, to avoid
  clobbering rapid client-side edits. Match this if you add another
  purely-client-authoritative setter.

## Singletons and cross-slot state

- **`tauri.conf.json` has no `enableGTKAppId`, deliberately — do not re-add
  it.** It used to be `true`, which made `tao` register a real
  D-Bus-activatable GTK `Application` per resolved identifier. That doesn't
  just risk two *processes* colliding: **any** activation of that D-Bus name
  — a dock/taskbar icon click, `gio launch`, systemd, literally
  `gdbus call --dest <id> --object-path /<id-with-slashes> --method
  org.freedesktop.Application.Activate '{}'` — re-enters Tauri's internal
  `setup()` (`tauri::app::make_run_event_loop_callback`'s `Ready` arm calls
  it unconditionally, with no re-entrancy guard) and panics rebuilding the
  config's `"main"` webview a second time (`a webview with label 'main'
  already exists`, tauri-2.11.5's `app.rs:1425`). Reproduced live: with
  `enableGTKAppId` on, a single `gdbus` `Activate` call crashed an
  already-running instance with **zero second process involved** — so a
  per-slot/per-checkout identifier can reduce the collision surface but
  can't close it; only dropping `enableGTKAppId` does, since without an
  app-id `tao` never asks GLib to register a bus name at all. The identifier
  is still patched per-slot at runtime (`lib.rs`'s `app_identifier`, applied
  to `context` right after `generate_context!()`) so `linux_desktop::
  ensure_installed`'s self-installed `.desktop` entry/icon get their own
  filename per slot instead of every slot's binary overwriting the same one.
- **`InstanceLock` is a generic, PID-tagged file lock** (`instance_lock.rs`),
  reused for two unrelated purposes under different lock names — don't
  assume every holder is cross-slot or every holder is per-checkout; it
  depends which name was passed to `try_acquire`:
  - `"slack-socket"` (`slack_socket.rs`) is a **shared, cross-slot**
    singleton: Slack credentials live in the shared config dir, so every
    open slot's process reads the same token, and without this guard N
    open slots would each open a duplicate Socket Mode websocket on it.
  - `"app-<identifier>"` (`lib.rs`'s `run`, acquired before `.run()`) is
    **per-checkout**: with no GTK/D-Bus single-instance registration
    anymore (see above), nothing else stops the *same* checkout being
    launched twice at once, duplicating windows/PTYs/scheduler polling. A
    second launch just prints "already running" and exits instead of
    proceeding — this is a resource-duplication guard now, not the crash
    fix (dropping `enableGTKAppId` is).
- **Nested shells get their env scrubbed and re-stamped** (`terminal.rs`,
  issue #39): a `tt-app` or `npm run dev` launched *inside* an embedded
  terminal doesn't collide with the outer instance's port/session identity.
  `CLAUDE_CODE_SSE_PORT` is re-stamped for deterministic IDE pairing even
  with several slots open — don't strip this scrubbing step to "simplify"
  terminal spawning.
- **The scheduler's watchers/in-flight guards persist across a
  settings-reload rebuild** (`scheduler.rs`), and a failed `claude:calendar`
  run still counts as "recent" — this avoids re-billing tokens on relaunch.
- **An external process can force an eager `prs` or `issues` collect via the
  nudge dir** (`tt_config::nudge_dir_path()`, watched in `scheduler.rs` via
  `tt_agentboard::fs_notify::DirNotifier`, same accelerant pattern as the
  agentboard journal watch in `lib.rs`). `tt collect nudge prs`/`tt collect
  nudge issues` (a plain filesystem touch, no store I/O) is the write side —
  the `towles-tool-app` Claude Code plugin's `gh pr`/`gh issue` mutation hook
  is the only current caller. It's a directory *separate* from `data_dir()`
  itself deliberately, so the watch isn't spammed by tt.db's own WAL/SHM
  churn. The `DirNotifier` callback only signals "something in the dir
  changed" — it can't tell which file — so `changed_nudge_batches` diffs each
  target's file mtime against what it last saw to resolve that into specific
  batches, which reuse `spawn_batch`/`guards.{prs,issues}` so a nudge can't
  stack a duplicate run alongside that collector's own tick. The watcher
  construction is `.ok()`-swallowed like every other `DirNotifier` use — a
  failed watch (e.g. inotify limits) just falls back to the normal poll
  cadence, never breaks startup.

## IDE bridge

- **The IDE server serves multiple concurrent connections per terminal**
  (`ide.rs`) — a Claude Code ≥2.1 session is a TUI process *and* a session
  daemon, both dialing in.
- **`openDiff` replies are deferred through a channel that auto-rejects on
  drop**, so a torn-down pane can never hang the CLI waiting on a review
  decision.

## Telemetry (required for user-gesture commands)

- **Every `#[tauri::command]` triggered by an explicit user gesture must emit
  its own `tracing` event** — a mutation, a confirm, a delete, or an action
  that signals a process. This is the backend half of the root CLAUDE.md's
  "every user action emits its OTel event" mandate (see the `tt-otel` bullet
  there), and it is not optional: without it the command is invisible in the
  on-disk event log, and "feature unused" can't be told from "feature
  uninstrumented" (the gap #363 fixed across ~all `ab_*`/`store_*`/slot/
  slack/settings/cockpit/ide commands). The rules:
  - **Name the event for *what changed*, `noun.verb`** (`task.created`,
    `repo.identity_set`, `session.closed`, `slot.created`) — never reuse
    `ui.action`. The frontend click already emitted a `ui.action`; a backend
    event with the same name double-counts the gesture. The two are
    complementary: `ui.action` records the intent, the command event records
    the outcome (and catches invocations that never came from a click).
  - **Record the outcome, not just that it ran** — a `changed`/`count` field,
    a `from`/`to` pair, or a `started`/`already_running`/`blocked` discriminant
    where the command can no-op or be refused (see `store_collect_now`,
    `slot_remove`). Log after the mutation succeeds; a longer-running command
    that can end three ways uses a span with an `outcome` field
    (`slot_remove`, `slot_stop_port`).
  - **Never log content or continuous input** — no note/message/prompt text,
    no per-keystroke/mouse/scroll/resize/PTY-write events. That's why
    `slack_dm_send` logs `slack.dm_sent` with *no* text, and the `term_*`
    input commands emit nothing (the PTY *spawn* is recorded in `term_start`
    via `tt_exec::record_detached_spawn`, the *kill* in `term_kill`).
  - **Don't instrument pure reads/pollers** (`*_get`/`*_snapshot`/`ab_get_*`/
    `app_resource_usage`) or agent-driven status reporters
    (`ab_set_status`/`ab_set_progress`/`ab_log` come from the PTY agent, not
    the user) — over-logging buries the signal. A command that already shells
    out through `tt_exec` (every `gh`/`git`) is covered by that `process.spawn`
    span, but still add a semantic event when the *user gesture* itself is
    what you want to be able to query for.

## Misc

- **`TT_NO_FOCUS_STEAL` skips OS focus-steal on launch** (`lib.rs`'s `run`,
  right after the identifier patch): when set, it flips every window
  config's `focus` to `false` before `context` reaches the builder.
  `scripts/dev-drive.mjs` and `scripts/e2e.mjs` set it, since both are
  test/verification launches, never the user actually sitting down to use
  the app. Deliberately a runtime env var, not `#[cfg(feature = "wdio")]` —
  that feature means "wdio plugins are compiled in," a different concern
  that only happens to correlate with these two scripts today.
- **OSC 52 clipboard writes are gated on terminal focus** (`terminal.rs`) —
  a background agent pane can't hijack the system clipboard.
- The `WEBKIT_DISABLE_DMABUF_RENDERER` env var (`lib.rs`, Linux-only) works
  around a WebKitGTK/NVIDIA rendering bug (tauri-apps/tauri#9304) — only set
  when NVIDIA is actually driving the screen, and never override an
  explicit user setting.
- **Linux app-id / desktop-entry self-registration** (`linux_desktop.rs`):
  `tauri build`'s packaging step normally writes a `.desktop` file + themed
  icon so GNOME/COSMIC can show the right entry/icon in the launcher/search
  — but the daily-driver flow (`npm start`) runs `tauri build --no-bundle`
  and execs the raw binary, skipping packaging entirely.
  `linux_desktop::ensure_installed` (called from `.setup()`) self-registers
  both into `~/.local/share/{applications,icons}` on every startup instead,
  idempotently, one `.desktop`/icon pair per slot (keyed by the per-slot
  identifier). `StartupWMClass` is the constant binary name (`tt-app`), not
  the per-slot identifier — `enableGTKAppId` is off (see "Singletons and
  cross-slot state" above for why), so the running window's real WM_CLASS is
  GTK's default, not our identifier; matching on the identifier here would
  never resolve. The running window's dock/taskbar icon is best-effort as a
  result — the launcher/search entry's icon is still exact.
