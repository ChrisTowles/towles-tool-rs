# towles-tool-app

Bridges Claude Code to the towles-tool desktop app (`tt-app`) — the MCP tools it
exposes and one hook that keeps its PR view fresh.

## MCP server

Points at the desktop app's own MCP server (`crates/tt-mcp`, served over
loopback HTTP by `tt-app`), so any session with this plugin enabled gets these
tools without manual `claude mcp add` setup:

- **Board** — `task_list`, `task_status` (reads), `task_create` and `task_delete`
  (writes). `task_delete` removes the task's panes and git worktree along with
  its board row, and refuses — deleting nothing — when the worktree still holds
  uncommitted or unlanded work.
- **Calendar** — `calendar_today`, `calendar_next` (reads) and `calendar_set`
  (writes). These exist for *focus protection* — how long until the next
  meeting, how much uninterrupted time is left — not calendar management.

The broader dashboard-read tools were pruned in the 2026-07 tool-surface review.

**Requires the app to be running.** There is no headless fallback: the server
lives in `tt-app`, so app closed means MCP down. Exactly one instance serves —
whichever binds the port first — so with several worktree tasks open, only one
is reachable, by design.

No token: the endpoint is loopback-only and refuses any request carrying an
`Origin` header or a non-JSON `Content-Type`, which is what keeps a web page you
visit from POSTing to it. That is the *whole* guard on writes — there is no
capability gate any more. See the trust-boundary doc in `crates/tt-mcp`.

Port defaults to `8787`; override with `"mcp": {"port": N}` in the shared
settings file, and update this plugin's `.mcp.json` to match.

## Skills

- **`task-onboarding`** — walks a repo through adopting tt worktree tasks:
  discover what the repo needs per-task (dev ports, docker names, setup),
  pick `${tt:port A-B}` pools that don't overlap other onboarded repos,
  write the tokenized `.env.example` (or `.claude/task-env.template`
  sidecar), then run the mechanical half with `tt task init` and verify with
  a smoke task. Triggers on "onboard this repo for tasks" / "set up tt
  tasks" / a repo whose tasks render an empty `.env` but need per-task
  ports. (Tasks work without any template — onboarding is only for repos
  that need ports/env vars templated per task.)
- **`towles-tool`** — `tt` CLI reference: journaling and worktree-task
  commands. Triggers on "tt commands", "daily notes", "meeting notes", or
  worktree management.

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
