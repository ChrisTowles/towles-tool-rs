# Migration Backlog

Porting the TypeScript `towles-tool` CLI to Rust, one feature at a time. Source
of truth for the old behavior is the TS CLI at
`~/code/p/towles-tool-repos/towles-tool-slot-1` (paths below are relative to that
repo). Structural patterns come from Yaak (see [ATTRIBUTION.md](../ATTRIBUTION.md)).

Work the items roughly in order; each builds on the last. When deriving code,
cite the yaak or slot-1 source path in the commit.

- [x] **0 — Scaffold (milestone 0).** Cargo workspace, `tt-config` + `tt-exec`
  shared crates, `tt-cli` (`config`, `doctor`), Tauri shell, React client. *This
  repo.*

- [x] **1 — Journal commands.** Filesystem note management with `chrono`-based
  templating (`{yyyy}`, `{monday}` and related date tokens); daily notes,
  meeting notes, list, search. Ported to the `tt-journal` crate (`tokens` +
  `entries` modules) and wired into `tt-cli` as `journal daily-notes|note|meeting|list|search`
  plus a top-level `today` alias.
  Source: `src/commands/journal/` (`fs.ts`, `templates.ts`, `daily-notes.ts`,
  `meeting.ts`, `note.ts`, `list.ts`, `search.ts`, `paths.ts`, `editor.ts`).
  Behavior deviations from the TS CLI:
    - Path templates support only the Luxon tokens the defaults actually use —
      `{yyyy}`, `{MM}`, `{dd}`, `{title}`, and their `{monday:...}` forms
      (e.g. `{monday:yyyy-MM-dd}`). Other Luxon tokens are emitted literally
      rather than formatted (the full Luxon vocabulary is not reimplemented).
    - The editor auto-open (`<editor> <folder> <file>`) is suppressed by a new
      `--no-open` flag, and is also skipped whenever stdout is not a TTY, so
      tests/CI never spawn an editor.
    - Note/meeting still prompt for a missing title interactively (via `inquire`,
      matching the TS `consola.prompt`), but when stdin is not a TTY they fail
      with a clear "title is required" error instead of hanging.
    - The per-command `--debug` flag is dropped in favor of the global
      `-v/--verbose` flag.
    - The TS built-in fallback strings for meetings carry a few trailing spaces
      that the on-disk default templates do not; this quirk is preserved
      verbatim (the fallback is only reached if the template dir is unwritable).

- [ ] **2 — GitHub helpers.** First `gh pr` and `gh branch-clean`, then `gh
  branch` with an interactive issue picker (evaluate `inquire` or `nucleo` for
  the fuzzy UI).
  Source: `src/commands/gh/` (`pr.ts`, `branch-clean.ts`, `branch.ts`).

- [ ] **3 — Install + Claude settings; doctor history/diff.** The `install`
  command and the `claude-settings` writer, plus extending `doctor` with run
  history and diffing.
  Source: `src/commands/install.ts`, `src/commands/claude-settings.ts`,
  `src/commands/doctor/history.ts`, `src/commands/doctor/checks.ts`.

- [ ] **4 — Graph.** JSONL token accounting and treemap rendering. Consider
  simplifying the TS design during the port rather than reproducing it 1:1.
  Source: `src/commands/graph/` (`parser.ts`, `analyzer.ts`, `treemap.ts`,
  `render.ts`, `sessions.ts`, `tools.ts`, `graph-template.html`).

- [ ] **5 — Claude plugin carry-over.** Bring `packages/core` markdown across
  as-is (hooks + skills) and decide how the marketplace distribution works.
  Source: `packages/core/` (`hooks/`, `skills/`, `README.md`).

- [ ] **6 — Tauri app feature direction.** Decide what the desktop app *is*.
  Leading candidate: agentboard-as-desktop.

- [ ] **7 — Agentboard Rust rewrite.** The hardest item: a `ratatui` TUI over
  `tokio-tungstenite` (websockets), `notify` (fs watching), and `tmux`. May be
  superseded by item 6 if the desktop app absorbs this role.
  Source: `src/commands/agentboard.ts`.

- [ ] **8 — Distribution + rename.** Ship it via `cargo-dist` / npm / a self-
  hosted updater (own infrastructure only), and perform the `ttr` → `tt` hard
  cutover once at feature parity.
