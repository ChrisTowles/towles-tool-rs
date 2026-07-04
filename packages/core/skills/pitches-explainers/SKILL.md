---
name: pitches-explainers
description: Package the work — prototype, spec, implementation notes — into a single pitch/explainer doc that gets buy-in and approvals, leading with the demo. Use when the user wants to explain a change to reviewers or stakeholders, get sign-off, "package this for Slack", "write a pitch", "make an explainer", or share what shipped.
user_invocable: true
---

# Pitches & Explainers

Shipping isn't done until someone approves it. A pitch/explainer artifact gets you
there two ways: it **accelerates understanding** for reviewers who start with the
same unknowns you did, and it **accelerates approval** for experts who want to see
you already accounted for the failure points *they* would have anticipated. You've
done the work of discovering the unknowns — the pitch shows you did.

Reference prompt this skill answers (use it almost verbatim):

> "Package the prototype, the spec, and the implementation notes into a single doc
> I can drop in Slack to get buy-in. Lead with the demo GIF."

## Process

1. **Gather the artifacts.** Pull together whatever exists — the prototype, the
   spec/plan, the implementation notes, the diff, screenshots or a demo GIF. The
   pitch is a synthesis, not new work.
2. **Lead with the demo.** Open with the most visceral proof it works — a GIF, a
   screenshot, a before/after. Reviewers decide whether to engage in the first
   few seconds; give them the payoff first.
3. **Then answer the reviewer's unknowns, in their order:**
   - **What & why** — the problem and the shape of the solution, for someone with
     zero context.
   - **How it works** — enough to trust it, not a code tour.
   - **Decisions & trade-offs** — pulled from the implementation notes: what you
     chose, what you rejected, why. This is what wins expert approval.
   - **Failure points handled** — the potholes you anticipated and covered.
   - **What's left / open questions** — honest edges, so reviewers don't have to
     find them to feel safe.
4. **Produce a single self-contained artifact** — an **HTML doc** (or Markdown if
   it's headed straight into Slack/PR). One thing to drop in a channel. Inline the
   assets so it travels intact.

## Rules

- **Demo first, always.** Lead with the proof; bury the mechanics.
- **Write for the reader's unknowns, not your process.** Order it the way a
  reviewer needs to learn it, not the way you built it.
- **Show the trade-offs you weighed.** Experts approve faster when they see you
  already considered the objection they were about to raise.
- **Single, self-contained, shareable.** One artifact, assets embedded, ready to
  paste.
