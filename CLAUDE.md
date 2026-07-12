# CLAUDE.md

Rust rewrite of `towles-tool`: a Tauri 2 desktop app plus the `ttr` CLI. Modeled on
the [Yaak](https://github.com/mountain-loop/yaak) repo structure (see
[ATTRIBUTION.md](ATTRIBUTION.md)).

## Commands

Rust:

```sh
cargo run -p tt-cli -- <args>       # run the CLI (binary is `ttr`, not `tt`)
cargo run -p tt-cli -- doctor       # e.g. doctor, config, journal, gh, install, claude-sessions
cargo fmt --check                   # formatting (rustfmt, 100-col)
cargo clippy --all -- -D warnings   # lint; warnings are errors
cargo test --all                    # unit + assert_cmd black-box tests
```

Desktop app / frontend:

```sh
npm install                         # installs apps/client (npm workspaces)
npm run dev                         # tauri dev — app + Vite frontend
npm run dev:drive                   # like dev, but the window is automatable (live-drive)
npm run drive -- <verb>             # drive the dev:drive window (status|invoke|shot|click|…)
npm run e2e                         # regression suite vs the real shell (see below)
cd apps/client && npx shadcn@latest add <name>   # vendor a shadcn/ui component
```

**Verifying UI/IPC changes — drive the real app.** Two ways, both hitting the
*actual* Tauri shell (WebKitGTK WebView + real Rust IPC), never a bare browser or
the mock dev server:

- **Live drive** — `npm run dev:drive` opens one automatable window (HMR, you use
  it normally); `node scripts/drive.mjs <verb>` drives *that same* window:
  `status`, `invoke <cmd> [json]` (real IPC), `eval "<js>"`, `shot <name>` (→
  `e2e/screenshots/<name>.png`, which you can `Read`), `click "<css>"`,
  `type "<css>" <text>`, `url <path>`. This is the way to visually/behaviorally
  debug a change and see the result. It's a plain-`fetch` client talking to the
  app's in-process WebDriver server — no WebdriverIO.
- **Regression suite** — `npm run e2e` runs WebdriverIO specs that spawn a fresh
  window, run, and exit (CI pass/fail). Specs in `e2e/specs/*.e2e.ts` are
  **read-only** (never write your real settings file); `npm run e2e:run` skips
  the rebuild.

Both are gated behind the `wdio` cargo feature + `VITE_WDIO` flag, so nothing
ships in normal/release builds. Ports come from `.env.local` (`TT_DEV_PORT`;
webdriver = `+3000`); `dev:drive` and `e2e` share a slot's ports, so don't run
both at once in one slot. Full docs + Linux gotchas: [e2e/README.md](e2e/README.md).

> The binary is **`ttr`** during migration. Do not rename it to `tt` — the
> TypeScript CLI keeps `tt` until the daily-driver commands are ported (full
> CLI parity is **not** a goal), then we hard-cut over (see
> [docs/MIGRATION.md](docs/MIGRATION.md), item 8).

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
    tool. Also the **single resolver for every mutable state path** (settings,
    `tt.db`, agentboard `*.json`): `state_scope()` detects when the process runs
    from a slot checkout (cwd walks up to a dir containing `crates/tt-config`)
    and, when scoped, nests state under `…/towles-tool/slots/<scope>/…` so
    concurrent slots don't clobber one shared file. Unscoped (installed daily
    driver) = the historic defaults, untouched. `TT_STATE_SCOPE` overrides
    (empty = force unscoped); the CLI `--config-dir` flag still wins for the
    settings path. Never build these paths ad-hoc — call the resolver.
  - `tt-exec` — process/command wrappers.
  - `tt-journal` — journal/note filesystem logic and date-token path templating.
  - `tt-git` — GitHub/git helpers: branch-name slugging, PR content, merged-branch
    filtering, issue parsing, picker layout.
  - `tt-graph` — session-JSONL token accounting, treemap/bar-chart building, and
    JSON/CSV/HTML rendering.
  - `tt-store` — the data-hub SQLite store (`~/.local/share/towles-tool/tt.db`):
    events, kanban todos (local, optionally issue-linked), issues, PR status,
    collector freshness. Collectors write events/issues/PRs; todos are
    user-created (and promotable to a `gh` issue). The app UI and MCP server
    read. Timestamps are epoch ms, passed in (`now_ms`) — never read the clock
    in logic.
  - `tt-collect` — collectors that fill tt.db: calendar via `claude -p`
    (strict-JSON prompt + lenient extraction; `CalendarProvider` picks the
    Google/Outlook prompt+MCP) — **off by default** since it burns tokens
    per tick; issues + PRs via `gh`; a watched Slack DM via the Slack Web API
    (escalating banner in the app). Collector keys are `claude:calendar`,
    `issues`, `prs`, `slack:dm` — the frontend matches on them. Email was
    removed in the day-screens pivot.
  - `tt-mcp` — hand-rolled stdio JSON-RPC MCP server (`ttr mcp serve`) exposing
    the store + live agent sessions + `journal_append` to claude sessions.
  - `tt-vt` — libghostty-vt terminal-state engine used by the app's canvas
    terminals (needs zig 0.15.x; see the frontend section).
  - `tt-agentboard` — agentboard watchers/engine: repo list, session tracking,
    needs-you synthesis (consumed by the app shell).
  - `tt-claude-code` — Claude Code transcript/session parsing models.
  - `tt-doctor` — doctor checks logic (CLI + app screen both consume it).
- `crates-cli/tt-cli` — `clap` 4 CLI, binary `ttr`. Commands:
  `config show|validate|schema|reset`, `doctor [--json --track --diff]`,
  `journal daily-notes|note|meeting|list|search` (+ `today` alias),
  `gh pr|branch|branch-clean|assign` (+ `pr` alias), `install [-o]`,
  `claude-sessions [-s --days -f html|json|csv --open/--no-open]`,
  `agentboard repos|sessions` (+ `ag` alias),
  `collect calendar|issues|prs|slack|all`, `mcp serve`.
- `crates-tauri/tt-app` — Tauri 2.11 shell. Identifier `dev.towles.tool`.
  `npm run dev` (root) picks a free dev-server port automatically
  (`scripts/dev-port.mjs`), scanning up from a per-slot base port derived from
  the slot's directory name (`scripts/slot-port.mjs`) instead of a hardcoded
  1420, so multiple worktree slots run the app concurrently without colliding.
  Pin a slot to a fixed port with `TT_DEV_PORT` in a gitignored root
  `.env.local` (dev-port reads it and passes it through to vite). Each window is
  labeled by slot: the title bar reads `Towles Tool — <slot>` and the app
  header shows a colored slot badge (`app_slot` command).
- `apps/client` — React 19 + Vite frontend styled with Tailwind CSS v4 +
  shadcn/ui (`@/*` → `src/*` alias, components vendored into
  `src/components/ui/`, light/dark via the `.dark` class). Yaak-style app
  shell: resizable sidebar + closable tabs (`src/lib/workspace.tsx` context),
  command palette (⌘K), settings dialog, status bar, keyboard shortcuts via
  the validated registry in `src/lib/shortcuts.tsx` (`?` opens the help
  overlay; screen-scoped bindings gate on their tab). Screens live in
  `src/screens/` (registry in
  `src/lib/screens.ts`). Live data flows through `src/lib/data.ts`
  (`useStoreSnapshot` → `store_snapshot` command + `store://snapshot` event)
  and `src/lib/agentboard.ts`; both fall back to mock data in plain-Vite
  browser dev. Older screens still render static mocks from
  `src/lib/mock-data.ts`. The three "Focus" screens are **Cockpit** (default
  day home — next-meeting countdown + PRs + issue queue), **Board** (cross-repo
  kanban over local todos grouped by status, with promote-to-issue), and
  **Agentboard** (repos + per-repo terminals). Terminals are a canvas
  renderer (`components/terminal-view.tsx` + `src/lib/term-protocol.ts`)
  over **libghostty-vt** terminal state in Rust (`crates/tt-vt`, one engine
  thread per terminal): the PTY host (`crates-tauri/tt-app/src/terminal.rs`)
  spawns shells with portable-pty, feeds bytes to the engine, and emits
  `terminal://frame` events (dirty-row style runs + cursor + selection +
  mode hints); input/resize/scroll/selection/copy go back as `term_*`
  commands. Building tt-vt needs **zig 0.15.x** on PATH (dotfiles
  `functions/18-zig.sh`). No cross-restart persistence; closing the app
  kills the shells. Product rules: the
  app is for getting in the zone —
  manage PRs and work issues across repos; calendar is only *time until the
  next meeting*. Agent status is **reported, never re-rendered** (interaction
  happens in the real PTY via the terminal view); the day
  bar (`day-bar.tsx`) and the Agentboard needs-you feed unify agents, PRs, and
  calendar into one attention model. Verify frontend/IPC changes by driving the
  real shell with `npm run e2e` (see the Commands section and
  [e2e/README.md](e2e/README.md)) — not just the mock browser dev server.

## Migration

Features are ported from the TypeScript CLI at
`~/code/p/towles-tool-repos/towles-tool-slot-1` per
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

- **Errors:** `thiserror` in library crates; flatten to exit codes at the CLI
  boundary (in `tt-cli`), not deep in the libs.
- **Tests:** black-box CLI tests with `assert_cmd`; unit tests alongside logic.
- **Formatting:** rustfmt, 100-column width.
- **Frontend styling:** Tailwind + shadcn/ui only — no CSS modules, no
  hand-rolled stylesheets, no CSS-in-JS. Add components with
  `npx shadcn@latest add <name>`, don't hand-write Radix wrappers.
- **No CLI-parity requirement.** The app is the primary product; each feature
  picks its natural surface. App-only features don't need a `ttr` subcommand,
  and terminal-native tools (journal, gh, doctor) don't need app screens. The
  CLI remains the home for terminal workflows and headless entry points
  (`mcp serve`, `collect`, `install`). Either way, the logic lands in a
  Tauri-free `crates/` library with unit tests — the e2e harness is not the
  primary correctness seam.
- **Hard cutover, no back-compat shims** — replace, don't wrap. (No compat
  layers, no dual-name aliases beyond the deliberate `ttr`→`tt` rename.)
- **Dev tooling must not hardcode ports/paths.** Chris runs multiple worktree
  slots of this repo concurrently (see [ATTRIBUTION.md](ATTRIBUTION.md) /
  `tt:parallel-slots`), so a fixed port, lockfile path, or other singleton
  resource makes copies collide. Default to dynamic allocation (e.g.
  `scripts/dev-port.mjs` picks a free port derived from the slot dir name)
  over a hardcoded value like `1420`.
- **No planning/implementation-notes docs committed to the repo** (e.g.
  `docs/<feature>/plan.html`, `implementation-notes.md`), even when a
  planning skill calls for writing one during implementation. Write them to
  the scratchpad directory instead — checked-in plans drift out of sync with
  the code and it's unclear which is authoritative. Git history retains any
  that were committed in the past; no need to preserve them elsewhere before
  removing.
