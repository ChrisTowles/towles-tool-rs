# towles-tool-app

Bridges Claude Code to the towles-tool desktop app (`tt-app`) — the MCP tools it
exposes and one hook that keeps its PR view fresh.

## MCP server

Registers the app's own `tt mcp serve` (`crates/tt-mcp`), so any session with this
plugin enabled gets these tools without manual `claude mcp add` setup:

`day_brief`, `needs_you`, `prs_status`, `issues_open`, `todo_create`,
`todo_update`, `todo_set_status`, `todo_delete`, `todo_link_issue`,
`todo_clear_done`, `journal_append`, `collect_refresh`, `collect_status`,
`agent_sessions`, `calendar_next`, `calendar_today`, `dm_status`, `snapshot`,
`tasks_open`.

Requires `tt` on `PATH` (see the root [README](../../README.md) / `install`
command).

## Skills

- **`slot-onboarding`** — walks a repo through adopting tt worktree slots:
  discover what the repo needs per-slot (dev ports, docker names, setup),
  pick `${tt:port A-B}` pools that don't overlap other onboarded repos,
  write the tokenized `.env.example` (or `.claude/slot-env.template`
  sidecar), then run the mechanical half with `tt slot init` and verify with
  a smoke slot. Triggers on "onboard this repo for slots" / "set up tt
  slots" / a `tt slot new` "no template" error.

## Hooks

| Hook                            | Event                | Does…                                                                 |
| -------------------------------- | --------------------- | ---------------------------------------------------------------------- |
| `hooks/scripts/gh-pr-nudge.sh`  | `PostToolUse` (Bash)  | After a `gh pr` mutation (merge/create/close/reopen/ready) or a `gh issue` mutation (create/close/reopen), nudges a running `tt-app` instance to refresh the matching data immediately instead of waiting for its normal poll interval (`tt collect nudge prs`/`tt collect nudge issues`). |

The hook is a no-op unless the session looks towles-tool-relevant — either it's
running inside a terminal the app itself spawned (`TT_SESSION_ID`/
`TT_APP_INSTANCE` set), or its working directory is inside a towles-tool-rs
checkout (a `crates/tt-config` ancestor). This plugin is meant to be enabled
globally, so without that guard the hook would still fire — harmlessly, but
uselessly — for `gh` commands run in unrelated projects. It also does nothing
if the towles-tool app isn't actually running for that checkout; the nudge is
picked up on the app's next start otherwise.

## Installation

```bash
claude plugin marketplace add ChrisTowles/towles-tool-rs
claude plugin enable towles-tool-app@towles-tool
```
