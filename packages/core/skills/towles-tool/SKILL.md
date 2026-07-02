---
name: towles-tool
description: Use towles-tool (`tt`) CLI for git helpers, journaling, and developer utilities. Use when asked about "tt commands", "create branch from issue", "daily notes", "meeting notes", or "check dependencies".
user_invocable: true
---

# towles-tool CLI

Personal CLI toolkit. Alias: `tt`

Config: `~/.config/towles-tool/towles-tool.settings.json`

## Git

```bash
tt gh branch        # Create branch from GitHub issue
tt gh pr            # Create pull request
tt gh branch-clean  # Delete merged branches
```

## Journaling

```bash
tt journal daily-notes  # Weekly file, daily sections (alias: tt today)
tt journal meeting      # Meeting notes (alias: tt m)
tt journal note         # General notes (alias: tt n)
```

## Utilities

```bash
tt config   # Show config (alias: cfg)
tt doctor   # Check dependencies
tt graph    # Visualize dependency graph
tt install  # Configure Claude Code settings
```
