---
name: brainstorm
description: Explore approaches and prototype before committing, to surface unknown knowns — the criteria you only know to define once you see them. Use when the user wants to brainstorm, explore design directions, see several UI variations, mock up a layout with fake data before wiring it up, or says "give me a few directions", "let me react to it", "what's possible here".
user_invocable: true
---

# Brainstorm & Prototype

Some criteria you can only define **when you see them** — these are your *unknown
knowns*. Verbalizing them early, while brainstorming, is cheap. Discovering them
mid-implementation is expensive: small spec changes cause large code changes, and
reverting is painful. So before committing, explore approaches and throw up cheap
artifacts the user can react to.

Reference prompts this skill answers (use them almost verbatim):

> "I want a dashboard for this data but I have no visual taste and don't know
> what's possible. Make me an HTML page with 4 wildly different design directions
> so I can react to them."

> "Before wiring anything up, make a single HTML file mocking the new editor
> toolbar with fake data. I want to react to the layout before you touch the real
> app."

> "Here's my rough problem: users churn after onboarding. Search the codebase and
> brainstorm 10 places we could intervene, from cheapest to most ambitious. I'll
> tell you which ones resonate."

## Pick the mode

- **Wide brainstorm** — "where could we intervene / what could we build?" Search
  the codebase and the web, then return a **ranked list of distinct approaches**
  (cheapest → most ambitious). Cast wider than the user framed it: they set too
  narrow or too wide a scope, and a good brainstorm corrects both. The user
  reacts; you don't pick.
- **Visual / layout prototype** — "what should this look like?" Produce a **single
  self-contained HTML file** with **several wildly different directions** on one
  page (or toggleable), using **fake data**. Do not touch the real app, wire up
  routes, or add state. The point is to react to layout and direction, fast.
- **Logic / state prototype** — "does this data model or flow feel right?" Mock
  the states with fake data so the user can walk through the cases that are hard
  to reason about on paper. Throwaway; no persistence.

If the mode is ambiguous and the user is reachable, ask. Otherwise default from
the surrounding context (a page/component → visual; a data model/flow → logic; an
open "where do we even start" → wide) and state the assumption at the top.

## Rules

- **Diverge, don't converge.** The value is in *different* options, not one option
  polished. "4 wildly different directions" means genuinely different, not four
  shades of the same idea.
- **Fake data, no wiring.** Prototypes mock the surface. No backend routes, no real
  state, no persistence — those are the things the prototype is *checking*, not
  depending on.
- **Self-contained HTML for anything visual.** One file the user can open
  directly. Inline everything.
- **Cheap and throwaway.** No tests, no polish, no abstractions. The output the
  user keeps is the *decision* — capture which direction resonated, then discard
  the rest.
- **The user reacts; you don't decide.** Present, label the trade-offs, and let
  them tell you which resonates.
