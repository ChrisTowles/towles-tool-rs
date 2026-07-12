---
description: Brainstorm 15 graded app improvements — "review" publishes an artifact of options; "build" ships the top 5 as parallel-slot PRs
argument-hint: review|build [optional theme, e.g. "terminal polish"]
---

Check out main and pull. Look in .claude/workflows/ first — if there's already a
saved workflow for this kind of fan-out, reuse it instead of authoring a new one.

Arguments: $ARGUMENTS

The first word of the arguments is the **mode** — `review` or `build`. Anything
after it is an optional theme/focus area for the brainstorm. If no mode is
given, default to `review` (the safe, look-before-you-build path).

## Shared first steps (both modes)

Run /tt:01-blindspot on this task first. Then:

1. Brainstorm 15 concrete improvements to this app. Ground them in the actual
   codebase (read the screens, crates, and docs/MIGRATION.md first — no generic
   ideas). Mix of: user-facing features, paper cuts in the three Focus screens,
   terminal/agentboard polish, and code-health items. Respect the product rules
   in CLAUDE.md and memory (no agent-TUI recreation, no repo auto-discovery,
   calendar = next-meeting only, hard cutover / no back-compat).

2. Grade each idea 1–10 on: value to my daily flow, effort (favor ≤ 1 slot-day),
   risk of merge conflicts with the other picks, and fit with the
   get-in-the-zone product direction.

3. Rank them and identify the top 5, favoring ideas that touch *different*
   areas so five parallel PRs won't conflict with each other.

## Mode: review

Load the artifact-design skill, then publish an Artifact (favicon 💡, keep the
same file path across reruns so it redeploys to one URL) presenting the
options for my review before anything is built:

- The full 15-idea table with grades per axis and total score.
- A highlighted "recommended top 5" section: for each, a short pitch, the
  files/crates it touches, effort estimate, and how I'd verify it.
- A short "cut but interesting" note for ideas 6–15 so nothing is lost.

Do NOT implement anything in review mode. End by telling me to run
`/improve-app build` (optionally with the same theme) when I've picked —
and note that I can name substitutions if I disagree with the top 5.

## Mode: build

Implement the top 5 (or the 5 I named when invoking build) in parallel using
the slot clones (tt:parallel-slots skill) with subagents — one idea per slot,
each on its own branch off main. Do not stop to ask me anything; my only
involvement is reviewing the finished PRs.

Each agent must: follow CLAUDE.md conventions, land logic in a Tauri-free
crates/ library with unit tests where applicable, run `cargo fmt --check`,
`cargo clippy --all -- -D warnings`, and `cargo test --all` (plus the
drive/e2e check if it touches UI), then open a PR with `gh`. Commit messages
cite upstream sources where relevant. Subagents don't see this conversation —
put every constraint above into each agent's prompt verbatim.

Report back with the 5 PR links, a one-line summary of each, and how to
verify each one.
