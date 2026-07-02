---
name: task
description: Create, list, and manage Claude Code tasks in the current session. Use when asked to "create tasks", "track work", "list tasks", "mark task done", or to break work into trackable steps.
user_invocable: true
---

# Task

Manage tasks for the current Claude Code session. Break work into trackable steps.

If the user passed arguments, treat them as described under "With arguments" below.

## Behavior

### No arguments — interactive mode

Ask what the user is working on, then break it into 3-7 concrete tasks. Create all via `TaskCreate`.
Print a summary table.

### With arguments — direct creation

Parse the arguments as task descriptions. A single sentence → one task. Comma-separated or bulleted →
multiple tasks.

### Subcommands

| Input             | Action                                | Example                             |
| ----------------- | ------------------------------------- | ----------------------------------- |
| `list`            | TaskList — show all tasks with status | `/tt:task list`                     |
| `done <id>`       | TaskUpdate — mark task completed      | `/tt:task done 1`                   |
| `start <id>`      | TaskUpdate — mark task in_progress    | `/tt:task start 2`                  |
| `cancel <id>`     | TaskUpdate — mark task cancelled      | `/tt:task cancel 3`                 |
| _(anything else)_ | Create task(s) from the text          | `/tt:task fix login bug, add tests` |

## Rules

- Task descriptions should be concrete and actionable — "add X" not "think about X"
- When creating multiple tasks, order them by dependency (do first → do last)
- After creating tasks, print a numbered summary so the user can reference them
- When marking done, confirm which task was completed
- Keep task names short (under 80 chars) — details go in the description field
- Subcommand routing is strict: `list` alone triggers TaskList. `done`, `start`, `cancel` must be
  followed by a numeric ID. Any other pattern — including `done <non-numeric text>` — is treated as task
  creation text
