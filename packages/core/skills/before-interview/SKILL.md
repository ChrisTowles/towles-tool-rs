---
name: before-interview
description: Interview the user one question at a time to resolve ambiguities and known unknowns before implementing, prioritizing questions whose answers would change the architecture. Use when the user says "interview me", "ask me questions", "grill me", wants to harden a plan, or after brainstorming when unknowns remain.
user_invocable: true
---

# Interviews

After brainstorming you still have unknowns — the *known unknowns* you're aware of
but haven't resolved, and the ambiguities lurking in the plan. An interview drags
them into the open **before** code gets written, when changing your mind is cheap.

Reference prompt this skill answers (use it almost verbatim):

> "Interview me one question at a time about anything ambiguous, prioritize
> questions where my answer would change the architecture."

## Process

1. **Load context first.** Read any referenced file, plan, or spec, and explore
   the relevant code so your questions are grounded — never ask what you could
   find yourself. Context is what makes the questions good.
2. **One question at a time.** Ask a single question, wait for the answer, then
   ask the next. This is deliberately different from a batch survey — each answer
   shapes the following question.
3. **Prioritize by blast radius.** Lead with the questions whose answers would
   **change the architecture**: data models, type interfaces, core flows,
   boundaries between concepts. Bikeshed details come last, if at all.
4. **Follow the thread.** When an answer opens a new ambiguity, chase it. When an
   answer is vague, push for the specific number, concrete example, or exact
   definition. Don't accept "the usual."
5. **Wrap up.** When the load-bearing unknowns are resolved, restate:
   - **Decided** — locked-in decisions
   - **Out of scope** — explicit exclusions
   - **Open questions** — anything still unresolved (should be near zero)

## Rules

- **One at a time — really.** The value is the branching conversation. Don't dump
  a numbered list.
- **Architecture-first ordering.** If two questions compete, ask the one that could
  invalidate more downstream work.
- **Never assume — confirm.** If you're guessing at intent, that's a question, not
  an assumption to bury in the plan.
- **Ask about requirements, not tools.** Probe constraints and behavior; don't
  smuggle in a tech recommendation as a question.
