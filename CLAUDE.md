# CLAUDE.md

Rust rewrite of the `towles-tool` CLI plus a Tauri 2 desktop shell. Modeled on
the [Yaak](https://github.com/mountain-loop/yaak) repo structure (see
[ATTRIBUTION.md](ATTRIBUTION.md)).

## Commands

Rust:

```sh
cargo run -p tt-cli -- <args>       # run the CLI (binary is `ttr`, not `tt`)
cargo run -p tt-cli -- doctor       # e.g. doctor, config, journal, gh, install, graph
cargo fmt --check                   # formatting (rustfmt, 100-col)
cargo clippy --all -- -D warnings   # lint; warnings are errors
cargo test --all                    # unit + assert_cmd black-box tests
```

Desktop app / frontend:

```sh
npm install                         # installs apps/client (npm workspaces)
npm run dev                         # tauri dev — app + Vite frontend
cd apps/client && npx shadcn@latest add <name>   # vendor a shadcn/ui component
```

> The binary is **`ttr`** during migration. Do not rename it to `tt` — the
> TypeScript CLI keeps `tt` until the Rust port reaches feature parity, then we
> hard-cut over (see [docs/MIGRATION.md](docs/MIGRATION.md), item 8).

## Architecture

Cargo workspace + npm workspace (`apps/client` only):

- `crates/` — **Tauri-free** shared libraries. This is a hard rule (Yaak's
  shared-crate pattern): nothing here may depend on `tauri`, so both the CLI and
  the app can use these crates.
  - `tt-config` — settings, stored at
    `~/.config/towles-tool/towles-tool.settings.json`. **This file is shared
    with the TypeScript CLI**, so serde types must tolerate unknown fields
    (`#[serde(default)]` / no `deny_unknown_fields`) to avoid breaking the other
    tool.
  - `tt-exec` — process/command wrappers.
  - `tt-journal` — journal/note filesystem logic and date-token path templating.
  - `tt-git` — GitHub/git helpers: branch-name slugging, PR content, merged-branch
    filtering, issue parsing, picker layout.
  - `tt-graph` — session-JSONL token accounting, treemap/bar-chart building, and
    JSON/CSV/HTML rendering.
  - `tt-store` — the data-hub SQLite store (`~/.local/share/towles-tool/tt.db`):
    events, tasks, emails, PR status, collector freshness. Collectors are the
    only writers; the app UI and MCP server read. Timestamps are epoch ms,
    passed in (`now_ms`) — never read the clock in logic.
  - `tt-collect` — collectors that fill tt.db: calendar/email/tasks via
    `claude -p` (strict-JSON prompts + lenient extraction), PRs via `gh`.
    Collector keys are `claude:calendar`, `claude:email`, `claude:tasks`,
    `prs` — the frontend day bar matches on them.
  - `tt-mcp` — hand-rolled stdio JSON-RPC MCP server (`ttr mcp serve`) exposing
    the store + live agent sessions + `journal_append` to claude sessions.
- `crates-cli/tt-cli` — `clap` 4 CLI, binary `ttr`. Commands:
  `config show|validate|schema|reset`, `doctor [--json --track --diff]`,
  `journal daily-notes|note|meeting|list|search` (+ `today` alias),
  `gh pr|branch|branch-clean` (+ `pr` alias), `install [-o]`,
  `graph [-s --days -f html|json|csv --open/--no-open]`,
  `collect calendar|email|prs|all`, `mcp serve`.
- `crates-tauri/tt-app` — Tauri 2.11 shell. Identifier `dev.towles.tool`.
  `npm run dev` (root) picks a free dev-server port automatically
  (`scripts/dev-port.mjs`), scanning up from a per-slot base port derived from
  the slot's directory name (`scripts/slot-port.mjs`) instead of a hardcoded
  1420, so multiple worktree slots run the app concurrently without colliding.
  Each window is labeled by slot: the title bar reads `Towles Tool — <slot>`
  and the app header shows a colored slot badge (`app_slot` command).
- `apps/client` — React 19 + Vite frontend styled with Tailwind CSS v4 +
  shadcn/ui (`@/*` → `src/*` alias, components vendored into
  `src/components/ui/`, light/dark via the `.dark` class). Yaak-style app
  shell: resizable sidebar + closable tabs (`src/lib/workspace.tsx` context),
  command palette (⌘K), settings dialog, status bar, keyboard shortcuts
  (⌘K/⌘,/⌘B/⌘W/⌘J/⌘D). Screens live in `src/screens/` (registry in
  `src/lib/screens.ts`). Live data flows through `src/lib/data.ts`
  (`useStoreSnapshot` → `store_snapshot` command + `store://snapshot` event)
  and `src/lib/agentboard.ts`; both fall back to mock data in plain-Vite
  browser dev. Older screens still render static mocks from
  `src/lib/mock-data.ts`. Product rules: agent status is **reported, never
  re-rendered** (interaction happens in the real PTY via xterm.js); the day
  bar (`day-bar.tsx`) and the Agentboard needs-you feed unify agents, PRs, and
  calendar into one attention model.

## Migration

Features are ported from the TypeScript CLI at
`~/code/p/towles-tool-repos/towles-tool-slot-1` per
[docs/MIGRATION.md](docs/MIGRATION.md). When deriving code, the commit message
should cite the upstream source path (yaak `path/to/file` or slot-1
`src/commands/...`).

## Conventions

- **Errors:** `thiserror` in library crates; flatten to exit codes at the CLI
  boundary (in `tt-cli`), not deep in the libs.
- **Tests:** black-box CLI tests with `assert_cmd`; unit tests alongside logic.
- **Formatting:** rustfmt, 100-column width.
- **Frontend styling:** Tailwind + shadcn/ui only — no CSS modules, no
  hand-rolled stylesheets, no CSS-in-JS. Add components with
  `npx shadcn@latest add <name>`, don't hand-write Radix wrappers.
- **Hard cutover, no back-compat shims** — replace, don't wrap. (No compat
  layers, no dual-name aliases beyond the deliberate `ttr`→`tt` rename.)
