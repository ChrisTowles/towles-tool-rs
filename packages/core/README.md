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

One command per technique, grouped by phase: before implementation,
plan/during, after. Each is a generic prompt you invoke; pass an optional
target as an argument.

### Before implementation

| Command             | Finds…           | Description                                                                       |
| -------------------- | ---------------- | --------------------------------------------------------------------------------- |
| `/tt:blindspot`   | unknown unknowns | Surface what you don't know you don't know in an unfamiliar area, and teach it.   |
| `/tt:brainstorm`  | unknown knowns   | Explore approaches / prototype with fake data so you can react before wiring.     |
| `/tt:interview`   | known unknowns   | Interview you one question at a time, architecture-changing questions first.      |
| `/tt:references`  | —                | Convey intent with a reference (ideally source code); reimplement its semantics.  |

### Plan & during implementation

| Command    | Finds…            | Description                                                                                                                                      |
| ----------- | ----------------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| `/tt:plan` | tweakable choices | Plan that leads with what you'll change (data models, types, UX), buries chores; then keeps implementation-notes with deviations while building. |

### After implementation

| Command           | Description                                                  |
| ------------------ | ------------------------------------------------------------ |
| `/tt:pitch`      | Package the work into one buy-in doc, demo first.            |
| `/tt:comprehend` | Report on the change + a quiz you must pass before merging.  |
| `/tt:memories`   | Review your memory files and recommend which to commit into CLAUDE.md. |
| `/tt:handoff`    | Write a short restart prompt for a fresh agent, copied to your clipboard. |

## Installation

```bash
claude plugin marketplace add ChrisTowles/towles-tool-rs
claude plugin enable tt@towles-tool
```
