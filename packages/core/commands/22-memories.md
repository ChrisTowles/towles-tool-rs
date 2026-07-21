---
description: Review your Claude Code memory files and recommend which are durable enough to commit into this repo's CLAUDE.md
argument-hint: [optional scope]
---

Review your own memory files for this project (the auto-memory system: read
`MEMORY.md` in your memory directory, then each file it links to). If this repo is
checked out in multiple parallel worktrees, also check sibling project
memory directories under `~/.claude/projects/` whose path
matches this repo's name with a different task suffix — facts learned in one task
are invisible in another, since memory is keyed by exact filesystem path, not by
repo.

For each memory, judge whether it's a durable, repo-wide fact (an architecture
decision, a standing feedback rule, a product-direction pivot) that should hold
regardless of which task or session someone is in, versus something ephemeral or
session-specific that belongs only in memory. Skip anything already reflected in
CLAUDE.md.

For each durable one that's missing, propose a short addition (a bullet or a
couple of lines, not a narrative) and where it belongs in CLAUDE.md. List your
recommendations and ask before editing — don't commit changes without confirming
which ones I actually want.

$ARGUMENTS
