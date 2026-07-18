# CLAUDE.md — crates/tt-collect

Collectors that fill `tt-store`'s `tt.db`: calendar (via `claude -p`), issues
+ PRs (via `gh`), and the pure protocol logic for a watched Slack DM (via
Socket Mode). Tauri-free — both `tt collect` (CLI) and the app's scheduler
drive the same [`CollectSummary`] contract.

## The never-panic, never-`Err` contract

Every public `collect_*` function returns a [`CollectSummary`] (`ok`,
`count`, `message`) — never a `Result::Err`, never a panic. A missing
`claude`/`gh` binary, a non-zero exit, or unparseable output all become
`ok: false` with a message, recorded via `Store::record_run` under a stable
key (`claude:calendar`, `issues`, `prs`, `slack:dm` — the frontend matches on
these). Keep new collectors inside this contract rather than propagating
`Result` upward: the app's scheduler awaits collectors in sequence, so one
`Err`/panic would take out every collector behind it in the batch, not just
its own.

## Per-repo isolation

`collect_issues`/`collect_prs` fan `gh` calls for each tracked repo across a
bounded thread pool (`lib.rs` ~line 285) rather than a single call per repo
serially. Each repo's outcome is independent — one repo's failure (a missing
dir, a repo `gh` can't reach) never sinks another repo's rows. Preserve this
when touching the per-repo path; a shared early-return would silently zero
out unrelated repos' data.

## Calendar collector: timeout matters more than it looks

`collect_calendar` is **off by default** (it burns tokens every scheduler
tick) and shells out to `claude -p` with a hard `CLAUDE_TIMEOUT` (180s). This
isn't a nice-to-have: the app's scheduler awaits collectors serially, so a
wedged `claude` (stuck on an auth prompt, a dead MCP server) would otherwise
block every other collector forever, not just the calendar one. If you add
another `claude -p`-backed collector, give it the same kind of hard ceiling.

## Slack: this crate has the protocol, not the socket

`slack_socket.rs` here is pure, unit-tested envelope/backoff/ack logic —
deliberately Tauri-free. The actual WebSocket I/O, reconnect loop, and the
cross-slot singleton lock that keeps N open worktree slots from each opening
a duplicate Socket Mode connection all live in
[`crates-tauri/tt-app/src/slack_socket.rs`](../../crates-tauri/tt-app/CLAUDE.md)
(see that crate's `InstanceLock` section) — that's the file to read for the
connection-lifecycle half of this feature.
