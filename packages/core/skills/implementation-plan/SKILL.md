---
name: implementation-plan
description: Write an implementation plan that leads with the decisions the user is most likely to change — data models, type interfaces, UX flows — and buries mechanical work at the bottom. Use when the user is ready to implement and wants a plan to review, says "write an implementation plan", "plan this out", or "show me the plan before you build".
user_invocable: true
---

# Implementation Plan

A plan's job isn't to prove you understand the mechanics — it's to **surface the
decisions the user still needs to make** while they're cheap to change. So the
plan leads with what's most likely to be tweaked and buries what the user already
trusts you to handle. The user reads the top, reacts, and the expensive rework
never happens.

Reference prompt this skill answers (use it almost verbatim):

> "Write an implementation plan in HTML, but lead with the decisions I'm most
> likely to tweak: data model changes, new type interfaces, and anything
> user-facing. Bury the mechanical refactoring at the bottom, I trust you on that
> part."

## Process

1. **Explore first.** Read the code you'd be changing so the plan is real, not
   aspirational. Find the seams, the existing patterns, and the constraints.
2. **Sort by likelihood-of-change, not by execution order.** The plan is ordered
   for *review*, not for *doing*. Put the reversible-in-your-head-but-expensive-in-
   code decisions at the top.
3. **Write the artifact** — an **HTML file** (a plan is easier to react to as a
   page). Structure top to bottom:
   - **Decisions you'll want to tweak** (lead here):
     - **Data model changes** — new/changed entities, fields, relationships.
     - **New type interfaces** — the signatures and shapes other code will bind to.
     - **Anything user-facing** — UX flows, states, copy, behavior.
     For each: state the decision, the alternative(s), and the trade-off — so the
     user can redirect with one comment.
   - **Mechanical work** (bury here): refactors, moves, wiring, test scaffolding —
     the parts the user trusts you on. Keep it short.
4. **Invite redirection.** End by pointing at the top-of-plan decisions and asking
   which to change before any code is written.

## Rules

- **Lead with the tweakable.** If a data-model or interface choice is on page two,
  the plan has failed at its one job.
- **Make each decision reversible on paper.** State the option *and* its
  alternatives, so "change it" is a sentence, not a redesign.
- **Bury, don't omit, the mechanical.** The user delegated it, not disowned it —
  list it briefly so nothing is a surprise, but don't make them wade through it.
- **Plan against the real code.** No plan built on assumptions about how the code
  works; go read it.
