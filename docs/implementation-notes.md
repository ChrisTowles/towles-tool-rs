# Implementation notes — Claude Code info-gathering layer (`tt-cc`)

Companion to [CC-INFO-GATHERING-PLAN.html](CC-INFO-GATHERING-PLAN.html). Running log kept
while building. If an edge case forces a deviation from the plan: pick the conservative
option, record it under **Deviations**, keep going.

## Status

**Complete (2026-07-05).** `tt-claude-code` extracted; both `tt-graph` and
`tt-agentboard` migrated onto it; the two duplicate transcript parsers are
deleted. Workspace: 389 tests pass / 0 fail, `clippy --all -D warnings` clean.

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

- **`tt-claude-code` deps are serde/serde_json only** (plan said serde/serde_json/chrono). chrono turned out unnecessary: timestamp parsing (`parse_timestamp_ms`) is agent-semantics and stayed in tt-agentboard. Conservative: keep the shared crate as small as possible.
- **`JournalEntry` renamed to `TranscriptEntry`** across tt-graph (not aliased) per the hard-cutover / no-dual-name rule.
- **`parse_journal_lines` (agentboard) deleted, not aliased** — call sites use `tt_claude_code::parse_transcript` directly. Behaviorally identical (`!l.is_empty()` vs `!l.trim().is_empty()` filter is a no-op for JSONL: a whitespace-only line fails `from_str` anyway).
- **B1 held**: agent status/thread/loop *semantics* stayed in tt-agentboard, rebuilt on the shared `Content::tool_uses()`/`first_text()` accessors. No semantics moved down.

## Verification log

- **tt-graph gate (output byte-identical):** `ttr graph -f json` compared before/after — 124/124 unchanged sessions produce identical tokens+cost; the only diffs are live transcripts that physically grew mid-migration (corpus drift, not regression).
- **tt-agentboard gate:** 111 crate tests pass unchanged (fixture-driven watcher scans).
- **Workspace:** 389 tests pass / 0 fail; `cargo clippy --all --all-targets -- -D warnings` clean; `cargo fmt --all --check` clean; Tauri app (`tt-app`) compiles.
- **Dedup confirmed:** no `struct RawEntry` / `JournalEntry` / `fn parse_jsonl` / `parse_journal_lines` / `RawUsage` remains in the Claude Code paths (codex/amp keep their own distinct tool schemas).
