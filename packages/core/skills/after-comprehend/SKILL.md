---
name: after-comprehend
description: After a long working session, generate a report on the change plus a quiz the user must pass before merging — so they actually understand what happened, not just what the diff shows. Use when the user says "quiz me", "test my understanding", wants to verify they understand a change before merge, or after a big session where a lot happened.
user_invocable: true
---

# Comprehension Check

After a long session, you may have accomplished more than the user realizes.
Reading the diff gives only a shallow understanding — much of the real behavior
lives in *existing* code paths the change now flows through. A quiz forces genuine
understanding. The rule: **merge only after passing.**

Reference prompt this skill answers (use it almost verbatim):

> "I want to make sure I understand everything that's happened in this change. Give
> me an HTML report on the changes for me to read and understand — with context,
> intuition, what was done, etc. — and a quiz at the bottom that I must pass."

## Process

1. **Reconstruct what happened.** Review the diff *and* the code paths it touches —
   including the existing behavior the change now depends on or alters. The
   understanding gap is usually in the code that *didn't* change but now matters.
2. **Build an HTML report** (a report + quiz is far better as a page). Two parts:
   - **The report** — teach the change:
     - **Context** — why this was done, the problem it solves.
     - **What was done** — the actual changes, grouped by concern, not file-by-file.
     - **Intuition** — the mental model: how it fits existing code, what to watch
       for, non-obvious interactions and consequences.
   - **The quiz** (at the bottom) — questions the user must answer correctly:
     - Prioritize questions about **behavior and consequences**, not trivia. "What
       happens when X now?" "Which existing path does this change affect?" "What
       would break if you removed Y?"
     - Include the answers (collapsed/hidden) so the user can self-check.
3. **Gate the merge.** Make it explicit: the user merges only after answering every
   question correctly. If they miss one, that's a real unknown — explain it, then
   re-quiz on that area.

## Rules

- **Quiz on consequences, not the diff.** Anyone can read what lines changed; the
  test is understanding what the change *does* through the existing code.
- **Target the blind spots.** Aim questions at the interactions the user is most
  likely to have missed by only reading the diff.
- **Pass-to-merge is the whole point.** A quiz that's trivial to pass teaches
  nothing — it should be genuinely possible to fail.
- **Teach on a miss.** A wrong answer is a discovered unknown; close it before merge.
