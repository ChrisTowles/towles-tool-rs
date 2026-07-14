# Towles Tool (Rust)

A Rust rewrite of [`towles-tool`](https://github.com/ChrisTowles/towles-tool):
a [Tauri 2](https://v2.tauri.app/) desktop app paired with the `tt` CLI. The repository
is built from the [Yaak](https://github.com/mountain-loop/yaak) golden template —
a Cargo workspace with Tauri-free shared crates, a `clap` CLI, and a React + Vite
frontend. It also ships the `tt` Claude Code plugin (see below).

The Rust binary is **`tt`** — the `ttr` → `tt` cutover from the TypeScript CLI
happened 2026-07-13 (hard cutover, no `ttr` alias left behind; see
[docs/CUTOVER.md](docs/CUTOVER.md)).

> **Status:** in progress. The scaffold plus config, doctor, journal, GitHub
> helpers, install, claude-sessions, the data-hub store/collectors, the MCP
> server, worktree slots (`tt slot`), and the Agentboard app screens (with
> live in-app terminals) are ported. Features land one at a time — see
> [docs/MIGRATION.md](docs/MIGRATION.md).

## Quick start

**Prerequisites**

- Node.js 24+
- Rust (stable toolchain)
- [zig](https://ziglang.org/) 0.15.x on `PATH` — the `tt-vt` terminal engine
  (used by the app's in-canvas terminals) builds against libghostty-vt
- Linux: `webkit2gtk` and the usual Tauri system dependencies
  (see the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/))

**Run the desktop shell**

```sh
npm install
npm run dev      # tauri dev — launches the app with the Vite frontend
```

The app is a day-focus shell: **Cockpit** (next-meeting countdown + PRs + issue
queue), **Board** (cross-repo kanban over local todos), and **Agentboard**
(watched repos with live per-repo terminals rendered from a real PTY through
the `tt-vt` engine). Each worktree slot picks its own dev-server port
automatically, so multiple slots run concurrently.

**Run the CLI**

```sh
cargo run -p tt-cli -- doctor
```

## Worktree slots

This repo is developed as **primary + slots**: a primary checkout that always
has the default branch, plus branch-named worktrees under `slots/`, one per
parallel line of work, each with its own rendered `.env` (port claims,
inherited secrets) so concurrent slots never collide. Manage them with
`tt slot` (`new`, `ls`, `rm`, `env`) — never raw `git worktree`. The
Agentboard rail shows the whole fleet and can create a slot from its `+`
button. Full convention and rules: [CLAUDE.md](CLAUDE.md).

## Claude Code plugin

The repo doubles as a Claude Code plugin marketplace. The `tt` plugin (in
[`packages/core`](packages/core/README.md)) packages the map-vs-territory
workflow commands, numbered so they sort in workflow order — `0x` before
implementation (`/tt:01-blindspot`, `/tt:02-brainstorm`, `/tt:03-interview`,
`/tt:04-references`), `1x` plan/during (`/tt:10-plan`), `2x` after
(`/tt:20-pitch`, `/tt:21-comprehend`) — plus the `towles-tool` and
`parallel-slots` skills.

Install it in Claude Code:

```sh
claude plugin marketplace add ChrisTowles/towles-tool-rs
claude plugin enable tt@towles-tool
```

Already installed? Pull the latest version with
`claude plugin marketplace update towles-tool`.

## Commands

The CLI binary is `tt`. Run any command with `--help` for its flags.

- `config show|validate|schema|reset` — inspect, validate, print the schema for, or reset settings.
- `doctor [--json] [--track] [--diff]` — check dependencies/environment; optionally save a run and diff against the last.
- `journal daily-notes|note|meeting|list|search` — filesystem notes with date-token path templates (`today` is an alias for `daily-notes`).
- `gh branch|branch-clean|pr|pr-list|assign|sync|co` — create a branch from a GitHub issue, delete merged branches, open a PR from the current branch, list your open PRs with CI status, assign an issue to a sibling slot, rebase the checkout onto `origin/main`, or check out a PR's branch by number (`pr`/`prs` are top-level aliases for `gh pr`/`gh pr-list`).
- `install [-o/--observability]` — apply recommended Claude Code settings and ensure required plugins.
- `claude-sessions [-s/--session] [--days N] [-f html|json|csv] [--open/--no-open]` — Claude Code session summary across every repo; HTML treemap report to `~/.claude/reports`, or JSON/CSV to stdout.
- `agentboard repos|sessions` — manage the watched-repo list and per-folder PTY sessions the app and collectors read (`ag` is an alias).
- `collect calendar|issues|prs|slack|all|status` — fill the local store: today's calendar via `claude -p`, assigned issues and open/review-requested PRs via `gh`, and a watched Slack DM; `status` reports each collector's health.
- `mcp serve` — stdio MCP server exposing the store, live agent sessions, and `journal_append` (register with `claude mcp add tt -- tt mcp serve`).
- `slot new|ls|rm|env|clean` — manage worktree slots (see [Worktree slots](#worktree-slots) above).

## Crates

Cargo workspace with Tauri-free shared crates plus the CLI and Tauri shells:

- `crates/tt-config` — settings (shared on disk with the TypeScript CLI).
- `crates/tt-exec` — process/command wrappers.
- `crates/tt-journal` — journal/note logic and date-token path templating.
- `crates/tt-git` — git/GitHub helpers (branch names, PR content, issue parsing).
- `crates/tt-graph` — session token accounting and treemap/JSON/CSV/HTML rendering.
- `crates/tt-doctor` — dependency/environment checks (CLI `doctor` and the app screen both consume it).
- `crates/tt-slots` — the worktree-slot convention: `${tt:...}` env-template renderer with port-pool claims, slot naming/layout, removal guards, and the shared `ops` orchestration behind `tt slot` and the app.
- `crates/tt-claude-code` — shared Claude Code transcript parsing (session JSONL, titles, token usage, model table).
- `crates/tt-store` — the data-hub SQLite store (events, kanban todos, issues, PR status, collector freshness).
- `crates/tt-collect` — collectors that fill the store: calendar via `claude -p`, issues/PRs via `gh`, a watched Slack DM via the Slack Web API.
- `crates/tt-agentboard` — watched-repo and agent-session tracking behind the Agentboard screen.
- `crates/tt-vt` — libghostty-vt terminal-state engine driving the app's canvas terminals (needs zig 0.15.x).
- `crates/tt-mcp` — stdio JSON-RPC MCP server over the store and live sessions.
- `crates-cli/tt-cli` — the `clap` CLI (binary `tt`).
- `crates-tauri/tt-app` — the Tauri 2 desktop shell; `apps/client` is its React + Vite frontend.

## More

- [packages/core/README.md](packages/core/README.md) — the `tt` Claude Code plugin in detail
- [ATTRIBUTION.md](ATTRIBUTION.md) — derivation from Yaak and its MIT license
- [docs/MIGRATION.md](docs/MIGRATION.md) — the feature-port backlog
- [docs/CODING-STANDARDS.md](docs/CODING-STANDARDS.md) — Rust/TypeScript coding standards
- [e2e/README.md](e2e/README.md) — driving the real app shell (live-drive + regression suite)
- [CLAUDE.md](CLAUDE.md) — project instructions, architecture, and the worktree-slot workflow

## License

MIT © 2026 Chris Towles. See [LICENSE](LICENSE).
