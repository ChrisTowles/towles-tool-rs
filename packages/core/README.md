# tt-core

Skills for **discovering your unknowns** — Claude Code helpers built around the
map-vs-territory workflow from
[trq212, "The map is not the territory"](https://x.com/trq212/status/2073100352921215386).

The map is what you give Claude (prompts, skills, context). The territory is where
the work actually happens (the codebase, its real constraints). The gap between
them is your **unknowns**, and reducing them is the skill of agentic coding. Each
skill is a cheap way to find out what you didn't know — before it gets expensive
to fix.

## Skills

Skills are named by phase (`before-` / `during-` / `after-`) so their place in
the workflow is obvious at a glance.

### Before implementation

| Skill                  | Finds…            | Description                                                                      |
| ---------------------- | ----------------- | -------------------------------------------------------------------------------- |
| `tt:before-blindspot`  | unknown unknowns  | Surface what you don't know you don't know in an unfamiliar area, and teach it.  |
| `tt:before-brainstorm` | unknown knowns    | Explore approaches / prototype with fake data so you can react before wiring.    |
| `tt:before-interview`  | known unknowns    | Interview you one question at a time, architecture-changing questions first.     |
| `tt:before-references` | —                 | Convey intent with a reference (ideally source code); reimplement its semantics. |
| `tt:before-plan`       | tweakable choices | Plan that leads with what you'll change (data models, types, UX), buries chores. |

### During implementation

| Skill             | Description                                                                          |
| ----------------- | ----------------------------------------------------------------------------------- |
| `tt:during-notes` | Log decisions and deviations as unknowns surface — conservative option, keep going. |

### After implementation

| Skill                 | Description                                                          |
| --------------------- | ------------------------------------------------------------------- |
| `tt:after-pitch`      | Package the work into one buy-in doc, demo first.                   |
| `tt:after-comprehend` | Report on the change + a quiz you must pass before merging.         |

### Utilities

| Skill               | Description                                                                       |
| ------------------- | --------------------------------------------------------------------------------- |
| `tt:towles-tool`    | `tt` CLI reference: git/gh helpers, journaling, dependency checks.                |
| `tt:parallel-slots` | Fan out parallel Claude Code agents across slot clones of any repo, via `gh` CLI. |

## Installation

```bash
claude plugin marketplace add ChrisTowles/towles-tool
claude plugin enable tt@towles-tool
```
