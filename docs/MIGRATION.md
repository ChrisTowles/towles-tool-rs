# Migration Backlog

Porting the TypeScript `towles-tool` CLI to Rust, one feature at a time. Source
of truth for the old behavior is the TS CLI at
`~/code/p/towles-tool-repos/towles-tool-slot-1` (paths below are relative to that
repo). Structural patterns come from Yaak (see [ATTRIBUTION.md](../ATTRIBUTION.md)).

Work the items roughly in order; each builds on the last. When deriving code,
cite the yaak or slot-1 source path in the commit.

**2026-07-11 — CLI parity dropped as a requirement.** The app is the primary
product; remaining TS features are ported selectively (only if still wanted)
and land on their natural surface — app screen or CLI command, no obligation
to ship both. The `ttr` → `tt` cutover gate (item 8) is that the daily-driver
commands work, plus Chris's explicit go-ahead — not full parity.

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

- [x] **3 — Install + Claude settings; doctor history/diff.** `tt install` plus
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
    - `tt install` skips the interactive plugin install when stdin is not a TTY,
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
  **2026-07-10 rename:** `ttr graph` → `ttr claude-sessions` (command module
  `commands/claude_sessions.rs`; the `tt-graph` crate name is unchanged — it's
  an internal implementation detail, not user-facing). The app screen (`Graph`
  → `Claude Sessions`) gained a "Recent sessions" list (title, project, date,
  tokens, sorted by recency) alongside the existing project/model bar charts,
  via a new `claude_sessions_list` Tauri command — the underlying discovery
  pass already scanned every repo under `~/.claude/projects`, so this just
  surfaces per-session `SessionResult` data the app wasn't showing yet.
  **2026-07-17 app-only cutover:** the `tt claude-sessions` CLI command is
  removed entirely (all formats — hard cutover, no alias) and the crate is
  renamed `tt-graph` → `tt-claude-sessions`; the CLI-only `format` module
  plus other dead code were deleted. The treemap explorer itself (deep
  project→date→session→turn→tool d3 report, `graph-template.html`) was then
  retired the same day in favor of an answer-first **Insights** tab (ranked
  waste findings — token outliers, re-read loops, cache churn, marathons —
  riding the cached ledger scan) plus a per-session turn/tool breakdown
  dialog opened from the Sessions table; the Overview charts picked up the
  report's CVD-validated categorical palette.

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

- [x] **7 — Agentboard rewrite inside the Tauri app.** Done across the 5 phases
  of the agentboard port: the Tauri-free
  `tt-agentboard` crate (types, tracker, metadata, session-order, git-info,
  ports, all four watchers — claude-code/amp/codex/opencode, bridge assembly,
  repos config, metadata-HTTP validation), the `tt-app` bridge (engine + tokio
  scan/git tasks + `agentboard://state` event + `ab_*` commands + localhost
  metadata ingest), the React UI, and the `tt agentboard`/`ag` repos CLI. The
  end-to-end demo verified live repos with git stats and a live Claude session
  updating in the Tauri window. Open questions / deferred: the pane-based
  "waiting" synthesis + prune pinning are driven by pid-liveness (only
  claude-code has a process-liveness signal; amp/codex/opencode are
  status/DB-derived and thus never pinned — a very long-idle-but-live
  codex/opencode "running" session could be pruned by the 3-min stuck rule); the
  `ports` column is unused (no tmux to attribute ps-tree ports); codex reads
  `session_index.jsonl` (JSONL), not sqlite, in the current slot-1 version.
  Source: slot-1 `packages/agentboard/` (live entry `src/server/main.ts`) +
  `src/commands/agentboard.ts`.

- [x] **8 — Distribution + rename.** Prepared 2026-07-02: distribution is
  local-first (`cargo install --path crates-cli/tt-cli`; tauri bundle for the
  app; registries/updaters deferred until the tool has other users — own
  infrastructure only, never yaak's). The `ttr` → `tt` flip was scripted
  in [docs/CUTOVER.md](CUTOVER.md) and **executed 2026-07-13**: the binary,
  every hint string, and the docs now say `tt` (hard cutover, no `ttr` alias);
  the operator steps (unlink the TS `tt`, `cargo install`, archive the TS repo)
  live in CUTOVER.md. The tmux agentboard workflow it once retired is
  **already removed** (2026-07-04, dogfooding the Tauri app full-time) — the
  `agentboard` command is now just the `repos` watch-list.

- [~] **9 — Data hub + day screens (new feature, not a TS port).** Built
  2026-07-04 from the product-direction session (Agentboard = attention inbox ×
  Daily Cockpit): `tt-store` (SQLite at `~/.local/share/towles-tool/tt.db`),
  `tt-collect` (calendar/email/tasks via `claude -p`, PRs via `gh`;
  `tt collect calendar|email|prs|all`), `tt-mcp` (`tt mcp serve`, stdio
  JSON-RPC MCP server: calendar/tasks/email/prs/agent-sessions/journal_append/
  collect_status tools), tt-app store commands + `store://snapshot` event +
  collector scheduler, and the client's day bar, needs-you Agentboard rework
  (split terminals), and Email + Calendar screen. Product rules: agent TUIs are
  never re-rendered (status is read-only; interaction = real PTY), collectors
  are the only tt.db writers, `assistant` settings block gates the token-costing
  claude collectors.

  **Day-screens pivot (2026-07-04, same day).** The product refocused on
  *getting in the zone*: PRs + cross-repo issues + a personal kanban. Changes:
  **email removed** everywhere (collector, `emails` table, MCP
  `email_needs_reply`, the Email + Calendar screen); **calendar reduced to the
  next-meeting countdown** (today only). New **`issues` collector**
  (`gh issue list --assignee @me`); collector keys are now `claude:calendar`,
  `issues`, `prs`; CLI is `tt collect calendar|issues|prs|all`. Collectors are
  now **config-driven** via `settings.collectors` (per-collector enable +
  cadence, replacing the `assistant` block), with a calendar `provider` field
  (`google`|`outlook`) selecting a built-in prompt+MCP for home/work. `tasks`
  became a **local kanban** (status/position + optional issue link;
  `store_set_task_status`, `store_promote_task_to_issue` → `gh issue create`).
  Frontend screens: **Cockpit** (default day home), **Board** (kanban),
  Agentboard unchanged. MCP swapped `email_needs_reply` for `issues_open`.
