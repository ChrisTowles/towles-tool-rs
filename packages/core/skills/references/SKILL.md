---
name: references
description: Use a concrete reference — ideally source code — to convey what you want when describing it in words would be slow or imprecise. Use when the user points at a library, crate, module, component, file, or website they like and asks to reimplement or match its behavior, semantics, or design, even across languages.
user_invocable: true
---

# References

Sometimes you can't describe what you want in detail — you lack the vocabulary, or
it would just take too long. A **reference** collapses that gap. Diagrams, docs,
and screenshots help, but the **best reference is source code**: it carries the
exact semantics, structure, and edge-case handling that prose leaves ambiguous.

Reference prompt this skill answers (use it almost verbatim):

> "This Rust crate in vendor/rate-limiter implements the exact backoff behavior I
> want. Read it and reimplement the same semantics in our TypeScript API client."

## Process

1. **Read the reference fully.** Whatever the user pointed at — a crate, a module,
   a component, a file, or a URL — read the actual source, not just its surface. If
   it's a website or component the user likes, read the underlying markup and code
   that produces it, not only the rendered screenshot: that's where the real detail
   lives (structure, states, how it's actually built).
2. **Extract the semantics that matter.** Name the specific behavior to carry over
   — the algorithm, the edge-case handling, the interface shape, the visual
   structure. Confirm with the user which parts are load-bearing and which are
   incidental to the reference's original context.
3. **Reimplement idiomatically in the target.** Match the *semantics*, not the
   syntax. Translate into the conventions, idioms, and style of the destination —
   language and framework included. A faithful port reads like native
   destination-code, not a transliteration of the source.
4. **Show the mapping.** Point out where you diverged from the reference and why
   (language differences, project conventions, things that didn't apply), so the
   user can check the translation.

## Rules

- **Source over screenshot.** When a reference exists as code, read the code. A
  picture tells you *what*; the source tells you *how* and *why*.
- **Semantics over syntax.** The goal is the same behavior/feel, expressed
  natively in the target — not a line-by-line copy.
- **Cross-language is fine.** The reference being in another language is normal;
  that's what makes it a reference and not a copy-paste.
- **Cite what you borrowed.** Note the reference path/URL in the commit or code
  comment so the lineage is traceable.
