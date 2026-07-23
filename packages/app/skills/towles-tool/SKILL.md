---
name: towles-tool
description: Use towles-tool (`tt`) CLI for journaling and worktrees. Use when asked about "tt commands", "daily notes", "meeting notes", or worktree management.
user_invocable: true
---

# towles-tool CLI

Personal CLI toolkit. Binary: `tt`

Config: `~/.config/towles-tool/towles-tool.settings.json`

## Journaling

```bash
tt journal daily-notes  # Weekly file, daily sections (alias: tt today)
tt journal meeting      # Meeting notes
tt journal note         # General notes
tt journal jot "text"   # Append a timestamped bullet to today's note
tt journal list         # Recent entries
tt journal search TEXT  # Search entries
```

## Worktree tasks

```bash
tt task new "Do the thing" --repo myrepo -b feat/thing  # board task + branch-named worktree + rendered .env
tt task ls                 # Fleet: main checkout + tasks, branch, dirty, ports
tt task rm <name>          # Guarded removal
tt task clean              # Remove every merged/gone task
```

`rm`/`clean` run a task's declared `TT_TASK_TEARDOWN` command (from its
rendered `.env`) against the worktree right before removing it — for
whatever a task's `TT_TASK_SETUP` started that the built-in docker
compose/container sweep can't find on its own (e.g. a compose stack not
named after the task). Unset by default; declare it per-repo in
`.env.example`.
