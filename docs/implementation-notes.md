# Implementation notes — Claude Code info-gathering layer (`tt-cc`)

Companion to [CC-INFO-GATHERING-PLAN.html](CC-INFO-GATHERING-PLAN.html). Running log kept
while building. If an edge case forces a deviation from the plan: pick the conservative
option, record it under **Deviations**, keep going.

## Status

Decisions locked (2026-07-05). Ready to build once the PR #19 sequencing gate is settled.

## Decisions locked

- **0 — do it now?** **0b — extract now.** Rationale (Chris): this is the foundation we build on for years; the multi-year horizon is the forcing function that justifies the single quarantine point and accepts the one-time migration risk.
- **A — crate boundary:** **A1 — schema + parse + projections only.** `tt-claude-code` deps = serde / serde_json / chrono. CLI wrapper, procenv, fs_notify, tail engine all stay in `tt-agentboard`.
- **B — canonical type / logic depth:** **B1 — schema + typed accessors.** One `TranscriptEntry`; content stays untyped `Value`, but `tt-claude-code` ships accessor helpers (`text_blocks()`, `tool_uses()`). Agentboard's status/thread/loop **semantics stay in agentboard**, rebuilt on those accessors — one content parse, "what counts as waiting" stays with the UI layer.
- **Crate name:** **`tt-claude-code`** (explicitly NOT `tt-cc`).
- **C — liveness semantics:** **C1 (default)** — liveness = `claude agents --json` ∪ `/proc` PID; JSONL never decides "running," only enriches. Grace-window (C2) not taken.
- **D — standalone surface:** **Deferred (default)** — MCP + Tauri app consume the new API now; `ttr cc` CLI is a later add, kept off the foundation to avoid scope creep.

## Open sequencing gate

PR #19 (title + dedup in tt-graph) is the canonical source of the code that moves into `tt-claude-code`. Extraction should build on merged #19 (or branch off the #19 branch and rebase if review changes it). **Not yet resolved:** merge #19 first, or start scaffolding now off the #19 branch.

## Deviations

_(none yet)_

## Verification log

_(per-step gate results: tt-graph totals unchanged, agentboard snapshots unchanged, clippy/fmt/tests green)_
