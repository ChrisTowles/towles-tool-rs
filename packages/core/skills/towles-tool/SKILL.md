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

Everything else (PR/issue flow, dashboards, collectors) lives in the desktop
app; `tt collect` is the one headless entry point.
