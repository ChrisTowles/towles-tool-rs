---
name: implementation-notes
description: Keep an implementation-notes file during a build, logging decisions and deviations from the plan as unknown unknowns surface mid-implementation — take the conservative option and keep going. Use when the user asks to "keep implementation notes", "log deviations", "track decisions as you build", or when starting a sizable implementation from a plan.
user_invocable: true
---

# Implementation Notes

No matter how much you plan, unknown unknowns lurk in the code — an edge case, a
constraint, an assumption in the plan that turns out to be false. When you hit
one mid-build, you shouldn't stop and re-plan the whole thing; you should make the
**conservative** call, **log** it, and **keep going**. The notes file is how those
deviations become learning for the next attempt instead of silent drift.

Reference prompt this skill answers (use it almost verbatim):

> "Keep an implementation-notes.md file. If you hit an edge case that forces you to
> deviate from the plan, pick the conservative option, log it under 'Deviations',
> and keep going."

## Process

1. **Create the notes file at the start.** `implementation-notes.md` (or `.html`)
   at the repo root or next to the work. It's a running log, not a deliverable —
   it can be deleted or folded into the PR description later.
2. **Log as you go, don't batch.** The moment a decision or deviation happens, it
   goes in the file. Batching at the end loses the reasoning that made it a
   deviation.
3. **On hitting an unknown:** pick the **conservative option** — the one that's
   easiest to reverse and least likely to surprise — record it under
   **Deviations**, and continue. Don't halt the whole build to litigate one edge
   case unless it invalidates the plan wholesale (then stop and flag it).
4. **Structure the file:**
   - **Decisions** — non-obvious choices made while building, with the why.
   - **Deviations** — where reality forced a departure from the plan: what the plan
     assumed, what was actually true, the conservative option taken, and what to
     revisit.
   - **Open questions / revisit** — things worth a second look before merge.
5. **Feed it forward.** These notes are the raw material for the next attempt, the
   PR pitch, and the quiz. They make the plan better *next* time — the map learns
   from the territory.

## Rules

- **Conservative and keep going** is the default reaction to an unknown — not stop,
  not the clever option. Momentum plus a log beats a stall.
- **Log the reasoning, not just the change.** "Did X" is useless later; "plan
  assumed Y, code actually does Z, so did X (reversible)" is gold.
- **Timestamp/anchor deviations to the code** (file, function) so they're findable.
- **It's throwaway.** The durable output is what gets folded into the commit,
  PR, or a decision issue — the file itself can go.
