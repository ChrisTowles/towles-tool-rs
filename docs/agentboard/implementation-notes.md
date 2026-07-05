# Agentboard — implementation notes

Working log for the Agentboard lifecycle / cache-health / windows feature.
Plan: [plan.html](./plan.html). Keep this file current while building.

**Rule:** if an edge case forces a deviation from the plan, take the
**conservative** option, log it under **Deviations** with the reasoning, and keep
going.

## Status

| Tier | What | State |
|------|------|-------|
| 0 | Rollup tally | **done** — `agentRollup()` selector; nav badge in app-sidebar; `RollupChip` atop the rail |
| 1 | Session liveness + empty states | **done** — `SessionData.live` stamped in tt-app (`stamp_live`/`stamped_payload`); PTY start/exit/kill nudge the emitter; hollow ring + "not started" + zero-session folders |
| 2 | Folder purpose | not started |
| 3 | Cache health + settings | **done** — details (ctx/cache) were already on `AgentEventDetails`; added `compactRecommendPercent` (tt-config, StatePayload, `ab_set_compact_percent`), `ctxPct/isCold/needsCompact` selectors, CacheBadge rows, ❄ rollup buckets, ⚙ settings popover w/ slider |
| 4 | Start / Stop / Compact / Restart | **done** — PTY-write actions (`claude\r`, Ctrl-C→Ctrl-D, `/compact\r`, restart) + 2.5s optimistic overlay; hover-reveal RowControls (▶ ✦ ■ ⤿ ↻ ✎ ✕); compact gated to at-prompt statuses |
| 5 | Windows / panes / grouping | **done** — WindowsStore (windows.json) + `ab_save_windows` + `StatePayload.windows`; window strip, percent-rect tiled panes in one flat mounted pool, placeholder panes for not-started restores, rail group-color tags + ⊟ ungroup |

## Decisions locked (from the plan review)

User approved all plan recommendations (2026-07-05).

- [x] 1 · **PTY-write** actions: start = `term_write(id,"claude\r")` (auto-`term_start` if stopped);
  stop = `term_write(id,"\x03")` (Ctrl-C, shell survives); compact = `/compact\r` gated on
  `status ∈ {waiting, idle}`; restart = Ctrl-C then a **fresh** `claude\r`. Optimistic overlay ~2.5s.
- [x] 2 · `cold = !cacheExpiresAt || now ≥ cacheExpiresAt` (null cache ⇒ cold). Nudge only when
  `cold AND ctx% ≥ threshold`. No warm nudge. Show 5m as `◔ m:ss`, 1h as `⧗`.
- [x] 3 · `agentboard.compactRecommendPercent` in tt-config, default **30**, **global only**, `#[serde(default)]`.
- [x] 4 · Filled dot = live PTY, hollow ring = not-started. Zero-session folders allowed. Keep the
  default `shell 1` seed but it starts **not-started**.
- [x] 5 · Nav rollup counts **running agents only** + non-zero status mini-dots + `❄ K to compact`.
- [x] 6 · Windows are frontend-owned; persist via single debounced `ab_save_windows` blob, hydrate in
  `ab_get_state`. Rail group colour tags + hover-`⊟` ungroup. Tiling: thirds at 3, 2×2 at 4.

## Plan amendments (final review, before implementation)

- **[Tier 4] stop = `\x03` then `\x04`** — a single Ctrl-C only interrupts the current
  turn and leaves the Claude REPL open; Ctrl-D at the now-empty prompt exits Claude
  while the shell survives. Sequence with ~150ms between writes.
- **[Tier 4] start opens the session as a pane first** — PTYs only spawn when a
  TerminalView mounts (`term_start` is in its effect); there is no headless PTY. So
  `▶ start` = select/open session → mount spawns shell → `term_write("claude\r")`.
- **[Tier 3] settings write path** — expose `compactRecommendPercent` on `StatePayload`
  (like `preferredEditor`) and add `ab_set_compact_percent(pct)` to persist via
  tt-config and re-emit. The plan listed the field but no setter.
- **[Tier 1] liveness merge point confirmed** — stamped in tt-app's debounced emit task,
  which owns both `TermState` and the engine handle; the Tauri-free crate stays clean.

## Prototype-parity pass (post-Tier-5, 2026-07-05)

User flagged gaps vs the Calm Rail mockup; closed in one pass and verified by
driving the mock UI in Chrome (multi-pane tiling confirmed working):

- Empty folders: `✦ start Claude · + shell` (creates the session, then launches).
- Focused-agent **cache bar** under the window strip: ctx meter + %, warm
  countdown / ❄ cold, and when over-threshold the warning pill + `⤿ compact` /
  `↻ start over` buttons (the prototype's cachebar).
- ❄ pill is now **clickable** (runs /compact); short `❄ N%` in the rail,
  long `❄ N% compact` in pane headers.
- Rail group tags show the **window name** next to the color square.
- **Toasts** on every lifecycle action (start/stop/compact/restart).
- **Window rename** via double-click on its strip tab.
- `+ session` in the window strip (new shell in the focused folder).
- Pane headers gained visible `■ stop` and `✕ kill` (kill = PTY + record).
- Fixed rail-row overflow (meta cluster now shrinks/truncates).

## Deviations

- **[Tier 5] panes positioned by percent-rects in one flat pool**, not nested
  per-window containers — React remounting a `TerminalView` re-runs
  `term_start`, which kills and respawns the shell. Terminals mount once and
  are shown/hidden + repositioned; scrollback and the PTY survive window
  switches and regrouping.
- **[Tier 5] restored panes don't auto-spawn PTYs** — a pane hydrated from
  windows.json renders a dashed placeholder (▶ shell / ✦ Claude / ⊟ remove)
  until started, honoring the liveness semantics instead of silently spawning
  shells at app launch.
- **[Tier 5] no window rename in v1** — windows get auto names ("main",
  "window N"); rename is a cheap follow-up if wanted.

- **[Tier 3] no new `AgentUsage` struct** — the planned payload type already
  existed: `AgentEventDetails` carries `contextUsed/contextMax/cacheExpiresAt/
  cacheTtlMs/lastActivityAt` and the claude watcher populates it. Only the
  threshold setting + frontend selectors were new.

- **[Tier 1] `ensure_default` seeds only never-seen folders** (key absent from
  sessions.json), not any empty folder. The old behavior re-seeded `shell 1` on
  every recompute, which would make "close the last session" impossible —
  contradicting the zero-session decision. New folders still get their seed;
  deliberately emptied folders stay empty.

## Verification log

- **Tier 4+5 (2026-07-05):** full `cargo test --workspace` 0 failures (121
  tt-agentboard incl. 3 windows tests) · workspace clippy 0 warnings ·
  `cargo fmt --check` clean · client tsc + vite build clean.
- **Rebase (2026-07-05):** work moved to `feat/agentboard-lifecycle`, rebased
  onto origin/main (+6 commits incl. context-max 1M fix); 118 tests green after.
- **Tier 2+3 (2026-07-05):** `cargo test -p tt-agentboard -p tt-config` 118+7
  passed · workspace clippy 0 warnings · client tsc + vite build clean.
- **Tier 0+1 (2026-07-05):** `cargo test -p tt-agentboard` 111 passed ·
  `cargo clippy -p tt-agentboard -p tt-app --all-targets` 0 warnings ·
  `apps/client npx tsc --noEmit` clean · `vite build` clean.
