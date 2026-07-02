# Towles Tool (Rust)

A Rust rewrite of the [`towles-tool`](https://github.com/ChrisTowles/towles-tool)
CLI, paired with a [Tauri 2](https://v2.tauri.app/) desktop shell. The repository
is built from the [Yaak](https://github.com/mountain-loop/yaak) golden template —
a Cargo workspace with Tauri-free shared crates, a `clap` CLI, and a React + Vite
frontend.

During the migration the Rust binary is named **`ttr`**. Once it reaches feature
parity with the TypeScript CLI, it takes over the `tt` name in a hard cutover.

> **Status:** in progress. The scaffold plus config, doctor, journal, GitHub
> helpers, install, and graph are ported. Features land one at a time — see
> [docs/MIGRATION.md](docs/MIGRATION.md).

## Quick start

**Prerequisites**

- Node.js 24+
- Rust (stable toolchain)
- Linux: `webkit2gtk` and the usual Tauri system dependencies
  (see the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/))

**Run the desktop shell**

```sh
npm install
npm run dev      # tauri dev — launches the app with the Vite frontend
```

**Run the CLI**

```sh
cargo run -p tt-cli -- doctor
```

## Commands

The CLI binary is `ttr`. Run any command with `--help` for its flags.

- `config show|validate|schema|reset` — inspect, validate, print the schema for, or reset settings.
- `doctor [--json] [--track] [--diff]` — check dependencies/environment; optionally save a run and diff against the last.
- `journal daily-notes|note|meeting|list|search` — filesystem notes with date-token path templates (`today` is an alias for `daily-notes`).
- `gh pr|branch|branch-clean` — open a PR from the current branch, create a branch from a GitHub issue, or delete merged branches (`pr` is an alias for `gh pr`).
- `install [-o/--observability]` — apply recommended Claude Code settings and ensure required plugins.
- `graph [-s/--session] [--days N] [-f html|json|csv] [--open/--no-open]` — token-usage treemap from session data; HTML report to `~/.claude/reports`, or JSON/CSV to stdout.

## Crates

Cargo workspace with Tauri-free shared crates plus the CLI and Tauri shells:

- `crates/tt-config` — settings (shared on disk with the TypeScript CLI).
- `crates/tt-exec` — process/command wrappers.
- `crates/tt-journal` — journal/note logic and date-token path templating.
- `crates/tt-git` — git/GitHub helpers (branch names, PR content, issue parsing).
- `crates/tt-graph` — session token accounting and treemap/JSON/CSV/HTML rendering.
- `crates-cli/tt-cli` — the `clap` CLI (binary `ttr`).
- `crates-tauri/tt-app` — the Tauri 2 desktop shell; `apps/client` is its React + Vite frontend.

## More

- [ATTRIBUTION.md](ATTRIBUTION.md) — derivation from Yaak and its MIT license
- [docs/MIGRATION.md](docs/MIGRATION.md) — the feature-port backlog
- [CLAUDE.md](CLAUDE.md) — project instructions and architecture

## License

MIT © 2026 Chris Towles. See [LICENSE](LICENSE).
