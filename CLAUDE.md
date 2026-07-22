# CLAUDE.md

Rust rewrite of `towles-tool`: a Tauri 2 desktop app plus the `tt` CLI. Modeled on
the [Yaak](https://github.com/mountain-loop/yaak) repo structure (see
[ATTRIBUTION.md](ATTRIBUTION.md)).

## Commands

Rust:

```sh
cargo run -p tt-cli -- <args>       # run the CLI (binary `tt`)
cargo run -p tt-cli -- task ls      # e.g. task, journal, collect, mcp
cargo fmt --check                   # formatting (rustfmt, 100-col)
cargo clippy --all -- -D warnings   # lint; warnings are errors
cargo test --all                    # unit + assert_cmd black-box tests
```

Desktop app / frontend:

```sh
npm install                         # installs apps/client (npm workspaces)
npm run dev                         # tauri dev — app + Vite frontend (debug build; noticeably laggy)
npm start                           # release build (`tauri build --no-bundle`) + run the binary — for daily driving
npm run dev:drive                   # like dev, but the window is automatable (live-drive)
npm run drive -- <verb>             # drive the dev:drive window (status|invoke|shot|click|…)
npm run e2e                         # regression suite vs the real shell (see below)
cd apps/client && npm run lint      # oxlint (types/react/unicorn/oxc rules; warnings are non-blocking)
cd apps/client && npm run format    # oxfmt, in place (100-col, matches rustfmt's width)
cd apps/client && npx shadcn@latest add <name>   # vendor a shadcn/ui component
```

**Verifying UI/IPC changes — drive the real app.** Two ways, both hitting the
*actual* Tauri shell (WebKitGTK WebView + real Rust IPC), never a bare browser or
the mock dev server:

- **Live drive** — `npm run dev:drive` opens one automatable window (HMR, you use
  it normally); `node scripts/drive.mjs <verb>` drives *that same* window:
  `status`, `invoke <cmd> [json]` (real IPC), `eval "<js>"`, `shot <name>` (→
  `e2e/screenshots/<name>.png`, which you can `Read`), `click "<css>"`,
  `type "<css>" <text>`, `url <path>`, `console [--clear]`. This is the way to
  visually/behaviorally debug a change and see the result. **A screenshot that
  looks right is not proof the render was clean** — React reports invalid
  markup as a runtime console error that nothing else here can see (no linter,
  no component tests), so every verb prints a `⚠ N console error(s)` summary
  and `console` dumps the detail. It's a plain-`fetch` client talking to the
  app's in-process WebDriver server — no WebdriverIO.
- **Regression suite** — `npm run e2e` runs WebdriverIO specs that spawn a fresh
  window, run, and exit (CI pass/fail). Specs in `e2e/specs/*.e2e.ts` are
  **read-only** (never write your real settings file); `npm run e2e:run` skips
  the rebuild.

Both are gated behind the `wdio` cargo feature + `VITE_WDIO` flag, so nothing
ships in normal/release builds. Ports come from the env files (`TT_DEV_PORT` in `.env.local`, or `.env` rendered by `tt task`;
webdriver = the `TT_E2E_WEBDRIVER_PORT` claim, falling back to `+3000`); `dev:drive` and `e2e` share a task's ports, so don't run
both at once in one task. Full docs + Linux gotchas: [e2e/README.md](e2e/README.md).

**After finishing a task that touches the app, leave it running for Chris to
check.** Once the change builds/lints/tests clean, launch `npm start`
(release build, the daily-driving binary) as a background task — Bash with
`run_in_background: true`, not a foregrounded blocking call — as the last
step before ending the turn. This is a courtesy handoff so the real running
app is already on screen for Chris to click through and validate, rather than
him having to remember to launch it himself. It doesn't replace driving/
screenshotting the app yourself first for UI/IPC changes (previous section) —
do both when the change touches the app. Skip it for changes with nothing in
the app to look at (CLI-only, docs-only, crate-internal refactors with no
`tt-app`/`apps/client` surface).

> The binary is **`tt`**. The `ttr` → `tt` cutover from the TypeScript CLI
> happened 2026-07-13 — hard cutover, no `ttr` alias left behind (see
> [docs/CUTOVER.md](docs/CUTOVER.md)).

## Worktree tasks — you are probably working in one

Tasks are branch-named git worktrees nested **inside** the checkout at
`<checkout>/.claude/worktrees/<name>/` — Claude Code's native worktree
location — one per parallel line of work (a `.tt-task` marker file sits at
each task's root). Any plain git checkout is task-capable with no
restructuring: point `tt task new` at it with `--repo`. Tasks are ephemeral:
created for a branch, removed when the branch merges. Manage them with
`tt task` — never raw `git worktree` or new clones. (`git clean -fdx` at the
checkout root is safe — git skips nested repositories without a second `-f`.)

```sh
tt task init                              # onboard a repo: template, .gitignore, worktree hooks, primary .env
tt task new "<title>" --repo <name|dir> [-b feat/thing] [--base <ref>] [--status doing] [--notes ...]
                                          # board task + .claude/worktrees/<branch-slug> in one shot
                                          # (branch defaults to a slug of the title)
tt task ls [--json]                       # fleet: main checkout + tasks, branch, dirty, ports
tt task env <name>                        # (re)render .env — idempotent, keeps claims
tt task env primary                       # same, for the main checkout
tt task rm <name> [--force]               # guarded removal + docker cleanup
tt task clean [--dry-run]                 # rm every merged/gone task + sweep stale state
```

Claude Code's own worktree surfaces (`claude --worktree`, background
sessions, the desktop app's parallel sessions) route through the same
machinery when the repo's `.claude/settings.json` wires the hooks:

```json
"hooks": {
  "WorktreeCreate": [{ "hooks": [{ "type": "command", "command": "tt task hook-create" }] }],
  "WorktreeRemove":  [{ "hooks": [{ "type": "command", "command": "tt task hook-remove" }] }]
}
```

`hook-create` reads the hook JSON on stdin and prints the task path (its one
line of stdout — the hook contract); the requested worktree name IS the
branch, verbatim (`claude -w feat/thing` → branch `feat/thing`, folder
`feat-thing`), never Claude Code's `worktree-<name>` scheme. `hook-remove` runs the same
guarded removal as `tt task rm`. Hooks execute from the *session checkout's
committed copy* of `.claude/`, so hook config edits only take effect in new
worktrees once committed. The blog repo (`~/code/p/blog`) is wired this way
and is the reference example.

The Agentboard rail shows the whole fleet automatically (worktrees of any
tracked checkout are discovered per poll), and the `+` button on the repo
header opens the same creation flow as a modal: goal → branch → base, then
Claude starts on the goal in the new task's terminal.

Rules when working in a task:

- **The main checkout is load-bearing.** Every task's git state lives in its
  `.git` — never delete, move, or re-clone it. Tasks never work on the
  default branch directly (git itself blocks a second checkout of it while
  the main checkout holds it).
- **One branch per task, named after it.**
  `tt task new "Thing" --repo <r> -b feat/thing`
  creates `.claude/worktrees/feat-thing` (the folder is the slugged branch —
  one-way; the branch is always read from git, never parsed back from the
  folder) (`--base` when not branching off the
  default). A task whose PR merged is done — `tt task rm` it (or
  `tt task clean`, which finds every merged/gone task); commits reachable
  from no branch or remote block removal by design.
- **Ports come from the rendered `.env`** — `.env.example` is the template
  (`${tt:port A-B}` pool claims, `${tt:task-name}`, `${tt:var NAME}`; a repo
  without tokens uses the `.claude/task-env.template` sidecar, and a repo
  with neither renders an empty `.env` — no template is required to create
  tasks), and a manual `.env.local` pin overrides it; shell env overrides
  both. Never hardcode a port anywhere. The main checkout claims its ports
  the same way.
- **No setup scripts.** `tt task new` runs the `TT_TASK_SETUP` command
  declared in `.env.example` (spawned directly, no shell — `npm install`
  here), falling back to lockfile detection in repos that don't declare one —
  and, in the CLI, runs it synchronously: `tt task new` is a foreground tool,
  so blocking on the install there is correct. **The app's `+` flow does
  not** — `task_create` (`crates-tauri/tt-app/src/task.rs`) only does the
  fetch/worktree-add/`.env`-render half and returns; `task_run_setup` fires
  separately, after the pane already opened, off `agentboard.tsx`'s
  `createTask`. The pane must never wait on the install again — it's what
  turned a 2–3s Linux task into a 1–2 minute macOS one (npm's per-file cost
  under APFS + Gatekeeper scanning is far higher than on Linux for the same
  `node_modules`), and the fix is to keep the two off the same critical path,
  not to make the install itself faster.
- **Never touch sibling task directories** — other agents work there
  concurrently. Instance state (tt.db, sessions/windows) is scoped per
  checkout via `tt_config::state_scope()`; shared stores (settings, tracked
  repos) are one machine-wide copy.
- Task logic lives in `crates/tt-tasks` (template grammar, removal guards,
  pure decisions) with shared orchestration in `tt_tasks::ops`; the CLI and
  the app's `task_create` command are thin shells over it. Change behavior
  there, not in the shells.
- **Migration state:** this repo's own checkouts still use the retired
  sibling layout (`~/code/p/towles-tool-repos/towles-tool-rs-primary` +
  `tasks/`). Running from an old-layout task still anchors correctly (the
  `.git` file's worktree pointer resolves to the main checkout), but new
  tasks land in `<checkout>/.claude/worktrees/`; old tasks drain as their
  branches merge.
- **Removing a task checkout goes through `tt_agentboard::task_removal`** —
  don't hand-roll the sequence. It untracks the dir from the shared
  `repos.json` and **closes** the bound board row, in that order, after the
  worktree leaves disk; `FinishedTask`/`RemovedTask` carry `dir`/`checkout`
  for exactly this. Closing (2026-07-22) replaced deleting: the row survives
  with a `TaskOutcome` (`done`/`abandoned`) and its `worktree_dir` cleared —
  the app's delete dialog asks which, headless callers (`tt task rm`, MCP)
  infer it via `TaskItem::inferred_outcome` (merged linked PR ⇒ done). Closed
  rows age into the archive (`archived_at`, `Store::archive_closed_tasks`,
  swept from `tt-collect`'s `sync_task_links` and the Board's "Archive done"
  button); `Store::delete_task` remains only behind the Board's explicit
  "Delete permanently", which refuses while a worktree is bound. Skip the
  untrack and a removed task's stale path lingers in the tracked-repos list
  forever, with the scheduler's `prs`/`issues` collectors retrying `gh`/`git`
  against a directory with no `.git` on every tick; skip the row-close and
  the board keeps a card claiming a worktree that no longer exists. `tt task
  rm`/`clean`/`hook-remove` and the app's `task_delete` are all shells over
  it.

## Architecture

Cargo workspace + npm workspace (`apps/client` only):

- `crates/` — **Tauri-free** shared libraries. This is a hard rule (Yaak's
  shared-crate pattern): nothing here may depend on `tauri`, so logic stays
  fast to compile and unit-testable without the app shell (and both the CLI
  and the app can consume it).
  - `tt-config` — settings, stored at
    `~/.config/towles-tool/towles-tool.settings.json`. **This file is shared
    with the TypeScript CLI**, so serde types must tolerate unknown fields
    (`#[serde(default)]` / no `deny_unknown_fields`) to avoid breaking the other
    tool. Also the **single resolver for every mutable state path**, split in
    two: **shared stores** (settings, agentboard `repos.json` — facts about
    the user/machine) are one machine-wide copy from every checkout, while
    **instance state** (`tt.db`, agentboard sessions/windows/collapse — one
    running checkout's world) nests under `…/towles-tool/tasks/<scope>/…` when
    `state_scope()` detects the process runs from a checkout of this repo (cwd
    walks up to a dir containing `crates/tt-config`; `.claude/worktrees/<name>`
    checkouts get repo-qualified scopes). A branch's schema experiments therefore never
    touch the daily driver's `tt.db`, but tracking a repo shows up everywhere.
    An explicitly set `TT_STATE_SCOPE` isolates *everything*, shared stores
    included (tests must never write real settings); empty = force unscoped.
    The CLI `--config-dir` flag still wins for the settings path. Never build
    these paths ad-hoc — call the resolver.
  - `tt-exec` — process/command wrappers.
  - `tt-journal` — journal/note filesystem logic and date-token path templating.
  - `tt-git` — GitHub/git helpers: branch-name slugging, PR content, merged-branch
    filtering, issue parsing, picker layout.
  - `tt-claude-sessions` — backs the app's Claude Sessions screen:
    session-JSONL token accounting, the single-parse ledger scan/search path,
    ranked waste insights (`insights`), and the per-session turn/tool
    drill-down (`breakdown`).
  - `tt-tasks` — the worktree-task convention (see the Worktree tasks section):
    the `${tt:...}` env-template renderer with port-pool claims, dotenv-lite
    parse/merge, task naming/layout, removal guards, and the shared
    orchestration in `ops` that both `tt task` and the app's `task_create`
    call. **`landed` is the one answer to "has this task's work reached the
    base branch"** — `tt task ls`/`rm`/`clean` and the Agentboard rail all go
    through `ops::work_state`, never their own git checks, because no single
    git signal covers all three landing shapes (a squash merge is invisible to
    both `--merged` and `git cherry`, which is what used to make merged tasks
    look like they still held work). It keeps *uncommitted changes* and
    *commits that never reached the base* as separate counts: only the first
    dies with the worktree, and only content-based evidence
    (`LandedVia::is_content_proof`) may justify `clean`'s `git branch -D` — a
    `[gone]` upstream looks identical whether the branch merged or was deleted
    unmerged. Read the module docs before touching the detection.
  - `tt-store` — the data-hub SQLite store (`~/.local/share/towles-tool/tt.db`):
    events, board tasks (#339: the unit of work — 0..N issue links + 0..N PR
    links in `task_issues`/`task_prs`, plus an optional worktree-task
    binding), issues, PR status, collector freshness. Collectors write
    events/issues/PRs and refresh link states; tasks are user-created
    (issues attachable/promotable via `gh`). The app UI and MCP server read.
    Timestamps are epoch ms, passed in (`now_ms`) — never read the clock in
    logic. **Calendar events are the exception**: their `starts_at`/`ends_at`
    are RFC 3339 text keeping the offset the calendar reported, with a
    STORED generated `starts_at_utc` as the sort/range key — never sort or
    range on the authored column, whose lexical order is not chronological
    across offsets.
  - `tt-collect` — collectors that fill tt.db: calendar via `claude -p`
    (strict-JSON prompt + lenient extraction; one run per enabled
    `CalendarSource`, each with its own user-editable prompt and its own store
    lane) — **off by default** since it burns tokens
    per tick; issues + PRs via `gh`; a watched Slack DM via the Slack Web API
    (escalating banner in the app). Collector keys are `claude:calendar`,
    `issues`, `prs`, `slack:dm` — the frontend matches on them. Email was
    removed in the day-screens pivot. See
    [`crates/tt-collect/CLAUDE.md`](crates/tt-collect/CLAUDE.md) for the
    never-panic contract, per-repo isolation, and where the Slack
    protocol/socket split lives.
  - `tt-mcp` — hand-rolled JSON-RPC MCP server, **transport-free** (the same
    split as `tt-ide`): `Dispatcher::handle_at` takes a request string and an
    injected `now_ms` and returns a response string, so the whole tool surface
    is unit-testable with no server to stand up. The transport is
    `crates-tauri/tt-app/src/mcp_http.rs` — read that module's doc before
    touching either half. Tools: `task_list`, `task_status`, `task_create`
    (a #339 board task in a tracked repo's swimlane, same store path as the
    app's `store_add_task`), `task_delete`, plus the calendar family
    `calendar_today`, `calendar_next` and the push-model write `calendar_set`.
    `task_delete` is the one tool that cannot work from the dispatcher alone —
    it kills the task's panes and removes its worktree (the row itself is
    *closed* with an optional `outcome` arg, not deleted — see the
    task-removal bullet in Worktree tasks), neither visible from a Tauri-free
    crate — so the transport injects a `TaskHost` (`tt-app`'s
    `task::delete_task_blocking`) and a dispatcher without one refuses rather
    than touching the row on its own. The broader
    dashboard-read tools (`day_brief`, `needs_you`, `snapshot`,
    PR/issue/DM/collector reads) were pruned in the 2026-07 tool-surface
    review and have not returned.

    **Security posture changed on 2026-07-20 — don't reason from the old
    shape.** There is no bearer token and no `mcp.mutationsEnabled` gate; both
    are gone, not merely defaulted. What guards writes is entirely the
    transport's request admission: **any request carrying an `Origin` header is
    refused** (browsers always send one, real MCP clients never do — the
    DNS-rebinding mitigation) and **`Content-Type: application/json` is
    required** (not a CORS-simple type, so a page can't dodge a preflight).
    Loopback binding alone does *not* keep web pages out, which is why those
    checks exist and why they're pure functions with direct tests. A
    consequence worth knowing before debugging: **the app's own webview cannot
    call the endpoint** — its `fetch` carries an `Origin` — so the MCP screen's
    tool tester issues its request from Rust (`mcp_test_call`). Both crates'
    module docs carry the full threat model.

    Served **one per machine, bind-or-skip**: whichever app instance takes the
    port serves every session, the rest serve none, and the OS bind is the
    mutex. App closed = MCP down; there is no headless fallback (the stdio
    server and `tt mcp serve` were deleted). The port is a **fixed default**
    (`mcp.port`, 8787) rather than a `${tt:port}` claim — the one legitimate
    exception to the no-hardcoded-ports rule, because a machine-wide singleton
    has nothing to collide with, and a stable port is what lets the
    `towles-tool-app` plugin ship a static checked-in `.mcp.json`.
  - `tt-telemetry` — telemetry: the `tracing` subscriber/writer and the reader
    behind the app's Telemetry screen, one crate so both halves can never
    disagree about the on-disk schema. `tt_telemetry::init` installs the global `tracing`
    subscriber for both binaries (it replaced `env_logger` — a hard cutover,
    no second logger), fanning out to stderr (filtered by `-v`/`RUST_LOG`) and
    to an **event log on disk**: one JSON object per line at
    `<data_dir>/telemetry/events-<date>.jsonl`, rotated daily, 14 days kept.
    The disk sink records at `debug` regardless of `RUST_LOG` — a quiet
    terminal must not mean a useless log — and every record carries OTel
    resource attributes including `tt.task`, so a line is attributable to the
    checkout that produced it. `TT_TELEMETRY=0` disables the disk sink.
    **Every subprocess is logged**, in one of two shapes depending on its
    lifecycle. Run-to-completion spawns (`gh`, `git`, `claude` — everything
    going through `tt-exec`'s three run paths) open a `process.spawn` span
    carrying `process.executable.name`, `process.command_args`,
    `process.working_directory`, `duration_ms`, `exit_code`, and `outcome`
    (`ok`/`non_zero_exit`/`timed_out`/`spawn_failed`). Spawns that outlive the
    call and have no exit code to wait for — the PTY behind every terminal,
    `rust-analyzer`, a detached editor — can't use that shape, so they call
    `tt_exec::record_detached_spawn(cmd, args, kind)` instead and emit a single
    event. **A new spawn site must use one or the other**, or it is invisible
    in the log; a bare `Command::new` is the one way to break the "what did
    this launch?" guarantee. Add instrumentation with `tracing` spans, not
    `log::` calls; existing `log::` sites still flow in via the subscriber's
    `tracing-log` bridge.
    **Every user-initiated action must be logged too, not just subprocesses**
    — see the README's "Core goal" section: this app's whole purpose is
    helping Chris manage his own focus/attention instead of it becoming a
    product, and that's only possible if the local event log is a complete,
    honest record of what happened and when. A new Tauri command triggered by
    an explicit user gesture (a click, a confirm, a delete, a shortcut that
    mutates state) needs a `tracing` span or event recording at least the
    action and its outcome — the same way `process.spawn` covers subprocesses.
    Frontend actions (click, shortcut, palette command, form submit) emit a
    `ui.action` event carrying a stable action id, the screen, and an
    optional word of `detail`; since the webview can't reach `tracing`, they
    cross IPC through one shared seam — `uiAction(action, screen, detail?)` in
    `apps/client/src/lib/ui-action.ts` → the `ui_action` command in
    `tt-app/src/lib.rs` — never per-feature ad-hoc plumbing. A backend
    command's own span should record what changed and be named for that
    (`repo.identity_set`), not `ui.action` — the click already emitted one, and
    reusing the name double-counts the action. Discrete intents
    only, never content or
    continuous input: no per-keystroke or mouse-move events, no PTY input, no
    note text (the log is plaintext, and per-record flushing assumes
    human-rate volume). OS-level signals with no other record — window focus/blur
    (`WindowEvent::Focused` in `lib.rs`), a native notification actually
    firing (`agentboard::notify_needs_you`) — get the same treatment, since
    they're exactly the kind of thing that's impossible to reconstruct after
    the fact otherwise (a real incident: `task_delete`'s ~1-minute worktree
    removal appeared to "steal focus" on completion, and there was no way to
    tell from the log alone whether the window itself ever regained OS focus,
    an unrelated needs-you notification fired at the same moment, or neither
    — all three now emit `window.focus_changed` / `notify_needs_you: fired`
    /`skipped` records precisely so the next occurrence is a `jq` query, not
    another live repro session). The **Telemetry** screen (`apps/client/src/
    screens/telemetry.tsx`, `crates-tauri/tt-app/src/telemetry.rs`) reads
    these files back for browsing/searching — day picker, level/kind/target
    filters, substring search, a per-record drill-down. It reads fresh off
    disk on every request rather than caching (the log is small and bounded
    by spawns/discrete actions, never per-keystroke input) and refreshes on a
    manual button and when the screen regains focus, not live-tailed.
  - `tt-ide` — Claude Code IDE-protocol core: the MCP/JSON-RPC dispatcher and
    lockfile schema the app uses to pose as an "IDE" a Claude Code CLI session
    connects to. Transport-free by design (sockets, auth, clocks live in
    `tt-app`); the lockfile's *filename* is the port (Claude Code parses it
    from the path, there's no port field in the JSON).
  - `tt-vt` — libghostty-vt terminal-state engine used by the app's canvas
    terminals. Needs zig 0.15.x on PATH to build; see
    [`crates/tt-vt/CLAUDE.md`](crates/tt-vt/CLAUDE.md) for the Debug-mode
    parser perf trap and other gotchas.
  - `tt-agentboard` — agentboard watchers/engine: repo list, session tracking,
    needs-you synthesis (consumed by the app shell). Also **the one home of the
    task-removal sequence** (`task_removal`): guards → host teardown → worktree
    off disk → untrack from `repos.json` → board row closed last (with a
    `TaskOutcome` — see the task-removal bullet in Worktree tasks). It lives here
    because it needs `tt-tasks`, `repos.json` and `tt-store` at once — `tt-tasks`
    can't host it (this crate already depends on it, so the edge would be a
    cycle) and `tt-app` can't (the CLI has no Tauri and would have to restate
    the order, which is exactly how the two copies drifted). Host-specific work
    — killing PTYs, closing rail folders, reaching a store held behind a mutex —
    enters through the `RemovalHooks`/`BoardRows` traits. Change the order
    there, not in a shell.
  - `tt-claude-code` — Claude Code transcript/session parsing models.
  - `tt-doctor` — doctor checks logic (app screen consumes it; the CLI command
    was removed in the 2026-07-19 trim).
  - `tt-update` — checks GitHub Releases for a newer version than the running
    app. Uses `native-tls` (not rustls/webpki-roots) for the same
    Zscaler-proxy reason called out below.
- `crates-cli/tt-cli` — `clap` 4 CLI, binary `tt`. Deliberately small after the
  2026-07-19 trim (usage review showed everything else was dead or app-owned):
  `journal daily-notes|note|meeting|jot|open|list|search` (+ `today` alias),
  `task init|new|ls|rm|env|clean` (worktrees — see the Worktree tasks
  section), and the headless entry point
  `collect calendar|issues|prs|slack|all|nudge|status` (slated to move into
  the app per the CLI redesign). The MCP server is not a CLI surface — it
  runs inside the app over loopback HTTP. The removed groups (`gh`, `config`,
  `doctor`, `install`, `agentboard`) live in git history; don't reintroduce
  CLI surfaces for app-owned features.
- `crates-tauri/tt-app` — Tauri 2.11 shell. Identifier `dev.towles.tool`.
  `npm run dev` (root) resolves the per-task dev-server port from the
  checkout's rendered `.env` (`scripts/dev-port.mjs` / `task-port.mjs`,
  running `tt task env` automatically when the checkout has no claim yet) —
  the `${tt:port}` claims in `.env.example` are the single source of truth,
  never a hardcoded 1420 or a derived/hashed port; anything already
  listening on the claimed port (almost always this task's own orphaned
  session) is killed first rather than scanned past. Pin a task to a fixed
  port with `TT_DEV_PORT` in a gitignored root `.env.local` (dev-port reads
  it and passes it through to vite). Each window is
  labeled by task: the title bar reads `Towles Tool — <task>` and the app
  header shows a colored task badge (`app_task` command). See
  [`crates-tauri/tt-app/CLAUDE.md`](crates-tauri/tt-app/CLAUDE.md) for the
  crate's internal locking/ordering/singleton invariants — it's the largest
  crate in the repo and the easiest one to introduce a subtle bug in.
- `apps/client` — React 19 + Vite frontend styled with Tailwind CSS v4 +
  shadcn/ui (`@/*` → `src/*` alias, components vendored into
  `src/components/ui/`, light/dark via the `.dark` class). Yaak-style app
  shell: resizable sidebar (the only nav UI — no visible tab strip; screens
  stay mounted in the background across switches), command palette (⌘K),
  settings dialog, status bar, keyboard shortcuts (`?` opens the help overlay).
  Screens live in `src/screens/`; the three "Focus" screens are **Cockpit**
  (default day home — next-meeting countdown + PRs + issue queue), **Board**
  (cross-repo kanban of tasks — #339's unit of work: issue/PR link chips,
  task branch, attach/detach + promote-to-issue; done rolls up from GitHub),
  and **Agentboard** (repos + per-repo terminals; its `+` flow creates a
  task whose worktree is an attribute of the task). The `+` form's
  **Dynamic** option launches the task's Claude session in plan mode with the
  goal wrapped by `dynamicFlowPrompt` (`apps/client/src/lib/agentboard.ts`):
  once the user approves the plan in the PTY, the session implements, runs
  `/code-review low --fix` and `/simplify`, rebases onto the base branch,
  opens the PR, and merges it — the merged PR then auto-attaches to the task
  and rolls it to `done` via the existing collect-side
  `auto_attach_worktree_prs`/`rollup_task_statuses` path (no new backend state).
  Terminals are a canvas renderer over **libghostty-vt** terminal state in
  Rust (`crates/tt-vt`); the PTY host
  (`crates-tauri/tt-app/src/terminal.rs`) spawns shells with portable-pty and
  streams frames over `terminal://frame`. No cross-restart persistence;
  closing the app kills the shells. Product rules: the app is for getting in
  the zone — manage PRs and work issues across repos; calendar is only *time
  until the next meeting*. Agent status is **reported, never re-rendered**
  (interaction happens in the real PTY via the terminal view); the day bar
  (`day-bar.tsx`) and the Agentboard needs-you feed unify agents, PRs, and
  calendar into one attention model. See
  [`apps/client/CLAUDE.md`](apps/client/CLAUDE.md) for frontend-internal
  conventions (screen registration, the shortcuts registry, invoke-wrapper
  semantics, the terminal wire protocol). Verify frontend/IPC changes by
  driving the real shell with `npm run e2e` (see the Commands section and
  [e2e/README.md](e2e/README.md)) — not just the mock browser dev server.

## Claude Code plugin marketplace

The repo root doubles as a Claude Code plugin marketplace
(`.claude-plugin/marketplace.json`); each plugin lives in its own
`packages/<name>/` with a `.claude-plugin/plugin.json` manifest, following
the standard plugin layout (`commands/`, `skills/`, `hooks/`, `.mcp.json` —
see [docs](https://docs.claude.com/en/docs/claude-code/plugins)). Two
plugins ship today:

- `tt` (`packages/core`) — the map-vs-territory workflow commands/skills
  (`/tt:01-blindspot` … `/tt:22-memories`).
- `towles-tool-app` (`packages/app`) — bridges Claude Code to the desktop
  app itself: registers the app's MCP server with a static checked-in
  `.mcp.json` (`{"type":"http","url":"http://127.0.0.1:8787/mcp"}` — board
  tasks `task_list`/`task_status`/`task_create`/`task_delete` plus the calendar family
  `calendar_today`/`calendar_next`/`calendar_set`; the app must be running),
  ships the `task-onboarding` skill
  (guides onboarding any repo onto worktrees — port discovery, template
  authoring, `tt task init`), and a `PostToolUse` hook
  (`hooks/scripts/gh-pr-nudge.sh`) that nudges a running app instance to
  refresh its PR or issue data immediately after a `gh pr`/`gh issue`
  mutation via `tt collect nudge prs`/`tt collect nudge issues`, rather than
  waiting for the app's normal poll interval — see the "nudge" mechanism note in
  [`crates-tauri/tt-app/CLAUDE.md`](crates-tauri/tt-app/CLAUDE.md). Meant to
  be enabled globally (its MCP tools are useful from any project), so its
  hook fails open/no-ops outside a towles-tool-relevant session — don't
  drop that guard when touching it.

A new hook/skill/MCP entry belongs in one of these plugin packages, not
loose in `.claude/` — `.claude/hooks/` is reserved for hooks scoped to
*this repo's own* Claude Code sessions (e.g. `guard-task-pkill.sh`), not
things meant to ship to other checkouts.

Any commit touching a plugin package is auto-checked by the
`.githooks/pre-commit` hook (`core.hooksPath .githooks`): it bumps that
plugin's version and runs `claude plugin validate .` against the
marketplace + both manifests before the commit lands.

## Migration

Features are ported from the TypeScript CLI at
`~/code/p/towles-tool-cli-repos/towles-tool-primary` per
[docs/MIGRATION.md](docs/MIGRATION.md). Porting is selective: a TS feature is
ported only if still wanted, and it lands on its natural surface (app screen
or CLI command — see the no-CLI-parity convention below). When deriving code,
the commit message should cite the upstream source path (yaak `path/to/file`
or slot-1 `src/commands/...`).

## Conventions

See [docs/CODING-STANDARDS.md](docs/CODING-STANDARDS.md) for the full
Rust/TypeScript coding standards (errors-as-values, parse-don't-validate,
branded/newtype domain types, deep modules, testing through real seams,
etc.). The points below are repo-specific specializations of that doc.

- **Rust conventions** (errors, tests, formatting, TTY guards, shared-file
  serde, etc.): see [`.claude/rules/rust.md`](.claude/rules/rust.md) — it
  auto-loads for any `.rs` file under `crates/`, `crates-cli/`, or
  `crates-tauri/`, so don't restate it here.
- **TypeScript errors are values**, the same as Rust's `Result` — via
  [better-result](https://better-result.dev). Expected failures belong in the
  return type, not in a `throw`, a rejected promise, or a `null` sentinel that
  conflates "absent" with "broken". `apps/client/src/lib/tauri.ts` is the model:
  one `invoke` returning `Result<T, IpcError>` that never throws, with tagged
  errors in `src/lib/errors.ts` (`TaggedError`, matched via `SomeError.is(e)`).
  See [`apps/client/CLAUDE.md`](apps/client/CLAUDE.md) for the call-site
  patterns. Reserve `throw` for unrecoverable defects (the shortcuts registry's
  module-eval validation) and for foreign interfaces that require it (monaco's
  `IFileSystemProvider`, vscode-jsonrpc) — translate at those boundaries.
  The `scripts/*.mjs` follow the same rule and are typechecked via
  `scripts/tsconfig.json` (`checkJs`), but keep `process.exit(N)` at the
  top-level CLI boundary — a non-zero exit code is the correct terminal
  behavior there, and `Result` is for the seams beneath it.
- **Frontend styling:** Tailwind + shadcn/ui only — no CSS modules, no
  hand-rolled stylesheets, no CSS-in-JS. Add components with
  `npx shadcn@latest add <name>`, don't hand-write Radix wrappers. The one
  carve-out is **animation**, where there are two idioms and the choice is not
  a preference: `tw-animate-css` classes (`data-open:animate-in …`, as the
  vendored `components/ui/*` use) for anything that animates while mounted,
  and the `motion` library for enter/exit of *dynamic lists* — a row removed
  from a backend snapshot unmounts before CSS can run, and only `motion`'s
  `AnimatePresence` can hold it on screen or `layout`-animate the rows that
  survive it. `apps/client/src/lib/rail-motion.ts` is the canonical config.
- **Every user action in the app must emit its OTel event** — event shape
  and exclusions in the `tt-telemetry` bullet in Architecture.
- **No CLI-parity requirement.** The app is the primary product; each feature
  picks its natural surface. App-only features don't need a `tt` subcommand,
  and terminal-native tools (journal, gh, doctor) don't need app screens. The
  CLI remains the home for terminal workflows and headless entry points
  (`collect`). Either way, the logic lands in a
  Tauri-free `crates/` library with unit tests — the e2e harness is not the
  primary correctness seam.
- **Hard cutover, no back-compat shims** — replace, don't wrap. (No compat
  layers, no dual-name aliases — the `ttr`→`tt` rename left no `ttr` behind.)
- **Dev tooling must not hardcode ports/paths.** Chris runs multiple worktree
  tasks of this repo concurrently (see the Worktree tasks section above), so
  a fixed port, lockfile path, or other singleton resource makes copies
  collide. Ports belong in `.env.example` as `${tt:port A-B}` claims rendered
  per checkout by `tt task env` (what `scripts/dev-port.mjs` resolves) —
  never a hardcoded value like `1420`, and never a second derivation scheme
  outside the claim system.
- **No planning/implementation-notes docs committed to the repo** (e.g.
  `docs/<feature>/plan.html`, `implementation-notes.md`), even when a
  planning skill calls for writing one during implementation. Write them to
  the scratchpad directory instead — checked-in plans drift out of sync with
  the code and it's unclear which is authoritative. Git history retains any
  that were committed in the past; no need to preserve them elsewhere before
  removing.
- **TLS clients must trust the machine's trust store, not a bundled root
  list.** Chris develops behind a Zscaler-style TLS-inspecting proxy, which
  installs its own root CA into the OS trust store; `rustls` + `webpki-roots`
  (or any other bundled Mozilla root list) never sees that CA and fails to
  connect. Any new outbound HTTP/WebSocket client (`ureq`, `reqwest`,
  `tokio-tungstenite`, etc.) must be configured to verify against the OS store
  — `native-tls` (used by the Slack integration: `crates/tt-collect/src/
  slack.rs`'s `agent()`, `crates-tauri/tt-app/src/slack_socket.rs`) or an
  OS-native-roots rustls variant (e.g. `rustls-native-certs` /
  `rustls-tls-native-roots`) — never the crate's bundled-webpki-roots default.
