---
name: blind-spot-pass
description: Surface the user's unknown unknowns before they start work — what they don't know they don't know about an unfamiliar area, so they can prompt better. Use when the user says "blindspot pass", "blind spot pass", "unknown unknowns", is entering an unfamiliar part of the codebase or a new domain, or asks you to teach them what they don't know they're missing.
user_invocable: true
---

# Blind Spot Pass

When you start work in an unfamiliar area — a new part of the codebase, an
unfamiliar domain, a design discipline you've never practiced — your most
dangerous unknowns are the ones you don't know you have. You don't know what
questions to ask, what "good" looks like, what historical work has been done, or
what potholes to avoid. This skill finds those **unknown unknowns** and teaches
them to you so you can prompt better.

Reference prompts this skill answers (use them almost verbatim):

> "I'm working on adding a new auth provider but I know nothing about the auth
> modules in this codebase. Can you do a blindspot pass to help me figure out my
> relevant unknown unknowns and help me prompt you better."

> "I don't know what color grading is but I need to grade this video. Can you
> teach me to understand my unknown unknowns about color grading, so that I can
> prompt better?"

## Process

1. **Get the starting point.** Before searching, know who's asking and what they
   already know. If it isn't in the prompt, ask briefly: their experience with
   this area, what they've already tried, and what "done" would look like. Their
   blind spots depend on their vantage point.
2. **Investigate the territory.** This is where you earn your keep — you search
   fast and know more than the user about the average topic.
   - **Codebase blind spots:** read the relevant modules, trace the existing
     patterns, find prior art (past PRs, related issues, `CONTEXT.md`), and note
     the conventions a newcomer would violate without knowing.
   - **Domain blind spots:** research the discipline. What are the core concepts,
     the vocabulary, the standard workflow, the well-known failure modes?
3. **Name the unknowns explicitly.** Produce the list of things the user didn't
   know to ask about. For each: what it is, why it matters here, and the specific
   pothole of ignoring it.
4. **Teach, then re-arm the prompt.** Explain each blind spot in plain language,
   then hand back **better prompts** — concrete, reworded asks the user can now
   make because they know what to ask for.

## Output

Deliver an **HTML artifact** — a blind-spot report is exactly the kind of thing
best read as a page, not a wall of chat text. Structure it:

- **What you told me** — the starting point, so the report is scoped to this user.
- **Your blind spots** — one card per unknown unknown: name, why it matters here,
  the pothole.
- **What "good" looks like** — the bar you couldn't see, with concrete examples.
- **Better prompts** — 3-6 sharpened asks the user can now make, ready to copy.

## Rules

- **Teach the unknowns, don't just fix them.** The deliverable is the user's
  improved intuition and a better next prompt — not the work itself. Resist
  jumping straight to implementation.
- **Be concrete and local.** "You didn't consider X" is only useful with the
  specific file, function, term, or failure it applies to. Generic checklists are
  a non-answer.
- **Calibrate to the stated experience.** Don't re-teach what the user already
  knows; hunt for the gap between what they said they know and what the territory
  actually demands.
