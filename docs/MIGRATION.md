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

- [x] **2 — GitHub helpers.** `gh pr` / `gh branch-clean` / `gh branch` (interactive
  issue picker), plus a top-level `pr` alias. Pure logic ported to the `tt-git` crate
  (`branch_name`, `pr`, `branch_clean`, `issues`, `picker` modules), fully unit-tested
  without `gh`/`git`/a terminal; the CLI layer shells out via `tt-exec` and prompts with
  `inquire` (chosen over `nucleo` — inquire's `Select` gives fuzzy filtering out of the box).
  Source: `src/commands/gh/` (`pr.ts`, `branch-clean.ts`, `branch.ts`) +
  `src/lib/git/{gh-cli-wrapper,branch-name}.ts` + `src/lib/render.ts`.
  Behavior deviations from the TS CLI:
    - Confirmation prompts require a TTY. `pr` refuses to run without `--yes` when
      stdin is not a terminal (clear error); `branch-clean` treats a non-TTY without
      `--force`/`--dry-run` as a no-op cancel. This keeps CI/tests from hanging.
    - `gh branch` uses an `inquire::Select` fuzzy picker instead of the TS
      `prompts` + `fzf` autocomplete. Column-layout/label rendering is ported
      verbatim (including the 24-bit hex label colors and dimmed-ellipsis truncation).
    - Branch-name/PR-title slugging matches the TS byte-for-byte, including its
      ASCII-only `\w`/`[^0-9a-zA-Z_-]` semantics (non-ASCII letters become `-`),
      verified against the TS test suite and a live luxon/JS cross-check.
    - The per-command `--debug` flag is dropped in favor of the global `-v/--verbose`.

- [x] **3 — Install + Claude settings; doctor history/diff.** `ttr install` plus
  the `claude-settings` writer, and `doctor` extended with `--track`/`--diff` run
  history. The settings read/write is a pure `claude_settings` module (models
  Claude Code's real `~/.claude/settings.json` as an open `serde_json::Map` so
  every unknown key survives). Doctor is restructured into
  `commands/doctor/{mod,history}.rs`: `mod` runs the checks (tools, gh auth,
  required Claude plugins, AgentBoard) into a `DoctorRunResult` whose serde shape
  matches the TS record exactly; `history` holds the pure history/diff logic
  (fully unit-tested).
  Source: `src/commands/install.ts`, `src/commands/claude-settings.ts`,
  `src/commands/doctor.ts`, `src/commands/doctor/history.ts`,
  `src/commands/doctor/checks.ts`, `src/commands/doctor/format.ts`.
  Behavior deviations from the TS CLI:
    - The doctor run-history file is SHARED with the TS CLI at
      `$XDG_CONFIG_HOME/tt/doctor-history.json` (default
      `~/.config/tt/doctor-history.json` — note the path uses `tt`, **not**
      `towles-tool`; this quirk is replicated faithfully). The Rust
      `DoctorRunResult` serializes to the exact TS shape (camelCase `ghAuth`,
      `version: string | null`, optional `warning`) so both tools read each
      other's records. History-path resolution honors `XDG_CONFIG_HOME`.
    - Doctor's output format is selected with a `--json` bool flag, not the TS
      `--format json` string option. `--json` emits the full `DoctorRunResult`
      (matching the TS `formatDoctorJson`), replacing milestone-0's ad-hoc
      `{tools, all_ok}` payload.
    - An extra `cargo` tool check is kept from milestone 0 (git gh node bun claude
      tmux ttyd otherwise match the TS names/patterns, with ttyd optional →
      `ok=true` + "optional, not installed" when missing). `diff` tolerates
      added/removed tools, so the extra check never breaks a comparison.
    - `ttr install` skips the interactive plugin install when stdin is not a TTY,
      printing a dim "skipped (non-interactive)" note instead of prompting — so
      CI/tests never hang and never run a real `claude plugin install` (same
      TTY-guard pattern as the journal/gh commands). The `claude plugin
      list`/`marketplace add` probes still run but degrade gracefully when
      `claude` is absent.
    - The per-command `--debug` flag is dropped in favor of the global
      `-v/--verbose` flag.

- [x] **4 — Graph.** JSONL token accounting and treemap rendering. Ported in two
  slices: **phase 1** the pure logic to the Tauri-free `tt-graph` crate (`types`,
  `parser`, `tools`, `labels`, `analyzer`, `sessions`, `treemap`, `format`,
  `render` — 80 unit tests), **phase 2** the `ttr graph` CLI wiring in
  `tt-cli` (`commands/graph.rs`). Flags: `-s/--session`, `--days` (default 7,
  0 = no limit), `-f/--format html|json|csv` (default html), `--open`/`--no-open`
  (default open). `~/.claude/projects` and `~/.claude/reports` resolve via `$HOME`
  so tests use a fixture project dir.
  Source: `src/commands/graph/` (`index.ts`, `parser.ts`, `analyzer.ts`,
  `treemap.ts`, `render.ts`, `sessions.ts`, `tools.ts`, `labels.ts`, `format.ts`,
  `graph-template.html`); `server.ts` intentionally *not* ported.
  Behavior deviations from the TS CLI:
    - **The local HTTP server is dropped** (approved simplification): no
      `--serve`/`--port` flags and no `server.ts` port. `ttr graph` (html format)
      only writes the report file under `~/.claude/reports/` and opens it in a
      browser — it never starts a server or blocks on Ctrl+C.
    - Auto-open is skipped when stdout is not a TTY (so tests/CI never launch a
      browser), in addition to the explicit `--no-open` flag.
    - Invalid `--format` values are validated in-command with the TS message
      (`Invalid format "x". Use: html, json, csv`) rather than by clap's own
      value-enum error, so the wording matches the TS CLI.
    - The per-command `--debug` flag is dropped in favor of the global
      `-v/--verbose` flag.
    - Report-filename timestamp: the TS luxon `yyyy-MM-dd'T'HH-mmZZZ` renders
      `ZZZ` as a techie offset (e.g. `-0400`); chrono's `%z` produces the
      identical `±HHMM` token, so filenames match byte-for-byte.
    - Carried over from the phase-1 crate port (already committed): `now_ms` is
      passed explicitly into `find_recent_sessions`/`calculate_cutoff_ms` instead
      of reading the clock internally (deterministic tests); session `startTime`
      is emitted locale-free; JSON key order follows the struct field order.

- [~] **5 — Claude plugin carry-over.** `packages/core/` (plugin manifest, hooks,
  skills, README) is copied across **verbatim** from slot-1 — pure markdown/JSON,
  byte-identical (`diff -r` clean), file modes preserved. Content still references
  the `tt` binary and the `tt@towles-tool` plugin id, left as-is (correct for
  existing users and post-cutover). The root `.claude-plugin/marketplace.json`
  lives at the *repo root* in slot-1, outside `packages/core`, so it is not part
  of this scoped carry-over.
  Distribution: **working default is option (a)** — keep shipping from the live
  ChrisTowles/towles-tool repo until the `ttr`→`tt` cutover; this copy is a
  mirror, not the source of truth, until then. Adopted as the only zero-breakage
  option (marketplaces are URL-keyed); see
  [docs/PLUGIN-DISTRIBUTION.md](PLUGIN-DISTRIBUTION.md). Chris can override.
  Source: `packages/core/` (`.claude-plugin/plugin.json`, `hooks/`, `skills/`,
  `README.md`).

- [x] **6 — Tauri app feature direction: agentboard-as-desktop.** The desktop
  app *is* agentboard. This was the leading candidate throughout; adopted
  2026-07-02 under the "continue all slices" directive. Consequence for item 7:
  the rewrite targets the Tauri app (Rust backend + React frontend), not a
  `ratatui` TUI — the terminal TUI is superseded, though the CLI keeps an
  `agentboard`/`ag` command to launch/manage the app-side pieces.

- [ ] **7 — Agentboard rewrite inside the Tauri app.** The hardest item,
  reshaped by item 6: port the agentboard core (repo scanning, git status,
  Claude-session watching, tmux integration, persistence) to a Tauri-free
  `tt-agentboard` crate, expose it through Tauri commands/events in `tt-app`,
  and rebuild the UI in the React client. Phasing per the port plan derived
  from the source inventory.
  Source: slot-1 `packages/agentboard/` (live entry `src/server/main.ts`) +
  `src/commands/agentboard.ts`.

- [ ] **8 — Distribution + rename.** Ship it via `cargo-dist` / npm / a self-
  hosted updater (own infrastructure only), and perform the `ttr` → `tt` hard
  cutover once at feature parity.
