---
name: slot-onboarding
description: Onboard a git repo onto tt worktree slots — unique per-slot ports, env templating, and Claude Code worktree hooks. Use when the user asks to "onboard this repo for slots", "set up tt slots", "make worktrees get unique ports", "wire the worktree hooks", or when a repo's slots render an empty `.env` and it needs per-slot ports. Requires the `tt` CLI on PATH.
---

# Onboard a repo onto tt worktree slots

Goal: after this, `tt slot new -b feat/thing` (or `claude --worktree`) gives
every worktree of this repo its own rendered `.env` with unique ports, and
merged slots clean up with `tt slot rm` / `tt slot clean`.

`tt slot init` does the mechanical half (template sidecar, gitignoring
`.env`, wiring the WorktreeCreate/WorktreeRemove hooks into
`.claude/settings.json`, rendering the primary checkout's `.env`). Your job
is the judgment half: figure out what this repo's slots actually need
templated, then run init and verify.

## 1. Discover what the repo needs per-slot

Look for anything two concurrent checkouts would collide on:

- Dev-server ports: vite/next/webpack config, `package.json` scripts,
  `Procfile`, framework defaults (vite 5173, next 3000, rails 3000, …).
- `docker-compose.yml`: host port mappings, container/volume/network names,
  database names.
- An existing `.env.example` / `.env.sample`: hardcoded ports and URLs.
- Setup: the repo's install command (lockfile detection covers plain
  `npm/pnpm/yarn/bun install`; anything more needs `TT_SLOT_SETUP`).

A repo with none of these still onboards fine — init creates an empty
sidecar and slots render an empty `.env`.

## 2. Pick port pools that don't overlap other repos

Claims are unique across a repo's own checkouts by construction, but there
is no machine-wide registry — two repos with overlapping pool ranges can
collide. Check the `${tt:port A-B}` ranges already used by the user's other
onboarded repos (grep their `.env.example` / `.claude/slot-env.template`;
tracked checkouts are listed in the app rail) and
pick a distinct range per variable, sized ~20+ ports, ideally starting at
the app's default port (e.g. vite → `5173-5272`).

## 3. Write the template

Prefer a committed, tokenized `.env.example` (the convention); use the
`.claude/slot-env.template` sidecar only when the repo shouldn't commit tt
tokens. Grammar (one token per value, comments never claim):

```sh
UI_PORT=${tt:port 5173-5272}          # port-pool claim, unique per checkout
DB_NAME=myapp_${tt:slot-name}         # checkout dir basename
URL=http://localhost:${tt:var UI_PORT}  # value rendered on an earlier line
BASE=${tt:base}                       # branch this slot PRs into
TT_SLOT_SETUP=npm install --prefer-offline
```

Make the app actually read these env vars — a templated port nobody reads
changes nothing. Never leave a hardcoded port behind for anything a slot
runs.

## 4. Run init and commit

```sh
tt slot init          # idempotent; renders primary/.env and prints claims
```

Commit what it changed plus your template: `.env.example` (or the sidecar),
`.gitignore`, and `.claude/settings.json` — hooks execute from the
*committed* copy, so they only take effect in new worktrees after the
commit lands.

## 5. Verify

```sh
tt slot ls                       # primary listed with its claimed ports
tt slot new -b chore/slot-smoke  # renders .env, runs setup
tt slot rm chore-slot-smoke      # guarded removal works
```

Confirm the smoke slot's `.env` got *different* ports than primary's, then
remove it.
