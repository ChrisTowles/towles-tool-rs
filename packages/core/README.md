# tt-core

Skills for **discovering your unknowns** — Claude Code helpers built around the
map-vs-territory workflow from
[trq212, "The map is not the territory"](https://x.com/trq212/status/2073100352921215386).

The map is what you give Claude (prompts, skills, context). The territory is where
the work actually happens (the codebase, its real constraints). The gap between
them is your **unknowns**, and reducing them is the skill of agentic coding. Each
skill is a cheap way to find out what you didn't know — before it gets expensive
to fix.

## Commands

One command per technique, numbered by phase so the menu sorts in workflow
order: `0x` before implementation, `1x` plan/during, `2x` after. Each is a
generic prompt you invoke; pass an optional target as an argument.

### Before implementation (0x)

| Command              | Finds…           | Description                                                                       |
| -------------------- | ---------------- | --------------------------------------------------------------------------------- |
| `/tt:01-blindspot`   | unknown unknowns | Surface what you don't know you don't know in an unfamiliar area, and teach it.   |
| `/tt:02-brainstorm`  | unknown knowns   | Explore approaches / prototype with fake data so you can react before wiring.     |
| `/tt:03-interview`   | known unknowns   | Interview you one question at a time, architecture-changing questions first.      |
| `/tt:04-references`  | —                | Convey intent with a reference (ideally source code); reimplement its semantics.  |

### Plan & during implementation (1x)

| Command       | Finds…            | Description                                                                                                                                      |
| ------------- | ----------------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| `/tt:10-plan` | tweakable choices | Plan that leads with what you'll change (data models, types, UX), buries chores; then keeps implementation-notes with deviations while building. |

### After implementation (2x)

| Command             | Description                                                  |
| ------------------- | ------------------------------------------------------------ |
| `/tt:20-pitch`      | Package the work into one buy-in doc, demo first.            |
| `/tt:21-comprehend` | Report on the change + a quiz you must pass before merging.  |
| `/tt:22-memories`   | Review your memory files and recommend which to commit into CLAUDE.md. |

## Skills

| Skill            | Description                                                        |
| ---------------- | -------------------------------------------------------------------- |
| `tt:towles-tool` | `tt` CLI reference: git/gh helpers, journaling, dependency checks. |

## Installation

```bash
claude plugin marketplace add ChrisTowles/towles-tool-rs
claude plugin enable tt@towles-tool
```
