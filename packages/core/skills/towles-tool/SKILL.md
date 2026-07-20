---
name: towles-tool
description: Use towles-tool (`tt`) CLI for journaling and worktree slots. Use when asked about "tt commands", "daily notes", "meeting notes", or worktree slot management.
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

## Worktree slots

```bash
tt slot new -b feat/thing  # Create a slot (branch-named worktree + rendered .env)
tt slot ls                 # Fleet: main checkout + slots, branch, dirty, ports
tt slot rm <name>          # Guarded removal
tt slot clean              # Remove every merged/gone slot
```

Everything else (PR/issue flow, dashboards, collectors) lives in the desktop
app; `tt collect` is the one headless entry point.
