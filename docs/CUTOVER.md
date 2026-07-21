# Slice 8 — Distribution & the `ttr` → `tt` cutover

The final migration item ([MIGRATION.md](MIGRATION.md) item 8). **Executed
2026-07-13** (repo side, step 2 below): the binary, hint strings, and docs all
say `tt` now — hard cutover, no `ttr` alias. The remaining steps are operator
actions in the live environment (unlink the TS `tt`, `cargo install`, smoke,
archive).

## Current state

- This repo's CLI builds as **`tt`**; all TS daily-driver commands are ported
  (config, doctor+history, journal, gh/pr, install, claude-sessions, agentboard repos).
- The TS CLI still owns the installed `tt` on any machine where step 1/3 below
  hasn't run (bun-linked from the live towles-tool checkout).
- The desktop app is the agentboard now — the tmux sidebar was **removed**
  (2026-07-04); `tt agentboard` is just the `repos` watch-list the app reads.
- The Claude plugin ships from the live repo (option (a),
  [PLUGIN-DISTRIBUTION.md](PLUGIN-DISTRIBUTION.md)); this repo's
  `packages/core` is a mirror until cutover.

## Distribution (own infrastructure only — never yaak's)

Local-first (it's a personal tool):

1. **Now / dev**: `cargo install --path crates-cli/tt-cli` installs `tt` to
   `~/.cargo/bin`. The app runs via `npm run dev` or `npm run build` (tauri
   bundle → .deb/.AppImage in `target/release/bundle/`).
2. **At cutover**: same commands, plus the rename below. No registry publish
   needed for a single-user tool; revisit cargo-dist / an npm wrapper /
   self-hosted updater only if the tool grows other users. (Explicitly out of
   scope: update.yaak.app, Flathub under yaak's listing, the @yaakapp npm
   scope — never point at yaak infrastructure.)

## Cutover preconditions (checklist)

- [ ] All task-N clones' in-flight branches merged or parked (the TS repo goes
      read-only for feature work).
- [ ] Agentboard daily-driven from the desktop app for long enough to trust it
      (the tmux sidebar has been removed — confirm the desktop app fully covers
      the workflow before the rename).
- [ ] `tt doctor --json`, `tt journal list`, `tt gh branch-clean --dry-run`,
      `tt claude-sessions -f csv` still parity-match their TS counterparts on real
      data (re-run the checks; they were byte-identical at port time).
- [ ] Plugin distribution decision executed (move marketplace here or keep
      shipping from the live repo indefinitely — see PLUGIN-DISTRIBUTION.md).

## The flip (one sitting, ~15 minutes)

1. `bun unlink` (or remove the bun-linked `tt` shim) in the live TS checkout —
   verify `which tt` no longer resolves to the TS CLI.
2. ~~Rename the binary here~~ — **done 2026-07-13**: `crates-cli/tt-cli/Cargo.toml`
   `[[bin]] name = "ttr"` → `"tt"`, plus every hint string, test, frontend
   label, and doc reference. Hard cutover — no `ttr` alias left behind.
3. `cargo install --path crates-cli/tt-cli` → `which tt` → the Rust binary.
4. Smoke: `tt doctor`, `tt journal list`, `tt today --no-open`, `tt ag repos`.
5. Archive the TS repo (GitHub archive on ChrisTowles/towles-tool) once the
   plugin distribution no longer depends on it, and rename this repo/directory
   (`towles-tool-rs` working name → final) — directory rename happens outside
   a live session.

**Rollback**: `cargo uninstall tt` (or delete `~/.cargo/bin/tt`) + re-link the
TS CLI. Both tools read the same config files, so no data migration in either
direction.

## Why the flip is gated on a human

`tt` is muscle memory and scripted in places (shell habits; the Claude plugin's
skills reference `tt` commands). The rename swaps the implementation under all
of that at once.
Everything is ported and parity-checked, but only the daily user can judge
when the desktop agentboard has earned the switch.
