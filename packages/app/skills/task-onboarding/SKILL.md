---
name: task-onboarding
description: Onboard a git repo onto tt worktree tasks — unique per-task ports, env templating, and Claude Code worktree hooks. Use when the user asks to "onboard this repo for tasks", "set up tt tasks", "make worktrees get unique ports", "wire the worktree hooks", or when a repo's tasks render an empty `.env` and it needs per-task ports. Requires the `tt` CLI on PATH.
---

# Onboard a repo onto tt worktree tasks

Goal: after this, `tt task new "Thing" --repo <name|dir> -b feat/thing` (or `claude --worktree`) gives
every worktree of this repo its own rendered `.env` with unique ports, and
merged tasks clean up with `tt task rm` / `tt task clean`.

`tt task init` does the mechanical half (template sidecar, gitignoring
`.env`, wiring the WorktreeCreate/WorktreeRemove hooks into
`.claude/settings.json`, rendering the primary checkout's `.env`). Your job
is the judgment half: figure out what this repo's tasks actually need
templated, then run init and verify.

## 1. Discover what the repo needs per-task

Look for anything two concurrent checkouts would collide on:

- Dev-server ports: vite/next/webpack config, `package.json` scripts,
  `Procfile`, framework defaults (vite 5173, next 3000, rails 3000, …).
- `docker-compose.yml`: host port mappings, container/volume/network names,
  database names.
- An existing `.env.example` / `.env.sample`: hardcoded ports and URLs.
- Setup: the repo's install command (lockfile detection covers plain
  `npm/pnpm/yarn/bun install`; anything more needs `TT_TASK_SETUP`).
- Teardown: anything setup starts that removal can't already find on its
  own — a docker compose stack not named after the task, an external
  process, a scratch DB — needs `TT_TASK_TEARDOWN`. No fallback detection
  here (unlike setup); unset means nothing runs.

A repo with none of these still onboards fine — init creates an empty
sidecar and tasks render an empty `.env`.

## 2. Pick port pools that don't overlap other repos

Claims are unique across a repo's own checkouts by construction, but there
is no machine-wide registry — two repos with overlapping pool ranges can
collide. Check the `${tt:port A-B}` ranges already used by the user's other
onboarded repos (grep their `.env.example` / `.claude/task-env.template`;
tracked checkouts are listed in the app rail) and
pick a distinct range per variable, sized ~20+ ports, ideally starting at
the app's default port (e.g. vite → `5173-5272`).

## 3. Write the template

Prefer a committed, tokenized `.env.example` (the convention); use the
`.claude/task-env.template` sidecar only when the repo shouldn't commit tt
tokens. Grammar (one token per value, comments never claim):

```sh
UI_PORT=${tt:port 5173-5272}          # port-pool claim, unique per checkout
DB_NAME=myapp_${tt:task-name}         # checkout dir basename
URL=http://localhost:${tt:var UI_PORT}  # value rendered on an earlier line
BASE=${tt:base}                       # branch this task PRs into
TT_TASK_SETUP=npm install --prefer-offline
#TT_TASK_TEARDOWN=docker compose down --volumes
```

Make the app actually read these env vars — a templated port nobody reads
changes nothing. Never leave a hardcoded port behind for anything a task
runs.

## 4. Run init and commit

```sh
tt task init          # idempotent; renders primary/.env and prints claims
```

Commit what it changed plus your template: `.env.example` (or the sidecar),
`.gitignore`, and `.claude/settings.json` — hooks execute from the
*committed* copy, so they only take effect in new worktrees after the
commit lands.

## 5. Verify

```sh
tt task ls                       # primary listed with its claimed ports
tt task new "Task smoke" --repo . -b chore/task-smoke  # renders .env, runs setup
tt task rm chore-task-smoke      # guarded removal works
```

Confirm the smoke task's `.env` got *different* ports than primary's, then
remove it.
