---
name: improve-architecture
description: Find deepening opportunities and architectural friction in a codebase, informed by the domain language in CONTEXT.md, then propose improvements. Use when the user wants to improve architecture, find refactoring opportunities, consolidate tightly-coupled modules, or make a codebase more testable and AI-navigable.
user_invocable: true
---

# Improve Codebase Architecture

Surface architectural friction and propose **deepening opportunities** — refactors that turn shallow
modules into deep ones. The aim is testability and AI-navigability. Diagnose first, present options —
never start refactoring immediately.

> "If you have a garbage code base, the AI will produce garbage within that code base."

## Glossary

Use these terms exactly in every suggestion — don't drift into "component," "service," "API," or
"boundary." Full definitions in [LANGUAGE.md](./LANGUAGE.md).

- **Module** — anything with an interface and an implementation (function, class, package, slice).
- **Interface** — everything a caller must know: types, invariants, error modes, ordering, config. Not
  just the type signature.
- **Implementation** — the code inside.
- **Depth** — leverage at the interface: a lot of behaviour behind a small interface. **Deep** = high
  leverage. **Shallow** = interface nearly as complex as the implementation.
- **Seam** — where an interface lives; a place behaviour can be altered without editing in place. (Use
  this, not "boundary.")
- **Adapter** — a concrete thing satisfying an interface at a seam.
- **Leverage** — what callers get from depth. **Locality** — what maintainers get: change, bugs,
  knowledge concentrated in one place.

Key principles:

- **Deletion test:** imagine deleting the module. If complexity vanishes, it was a pass-through. If
  complexity reappears across N callers, it was earning its keep.
- **The interface is the test surface.**
- **One adapter = hypothetical seam. Two adapters = real seam.**

## Process

### 1. Explore

Read the project's domain glossary (`CONTEXT.md`) and any decision issues in the area first. Use the
`Agent` tool with `subagent_type=Explore` to walk the codebase — explore organically and note friction.

Anti-patterns to look for (agent-friendliness + depth):

- **Scattered concepts** — understanding one feature requires bouncing between many small files
- **Shallow modules** — interface nearly as complex as the implementation
- **Pure functions extracted only for testability** — but the real bugs hide in how they're called (no
  **locality**)
- **Tight coupling / leaks across seams** — modules that can't be tested or changed independently
- **Missing boundaries, circular dependencies, god classes, impure functions in business logic**
- **Inconsistent patterns** — mixed error handling, logging, data access
- **Test gaps**, and **large files** (>300 lines is a smell)

Apply the **deletion test** to anything you suspect is shallow. If the architecture is already clean,
say so — do not invent problems.

### 2. Diagnose + present

For each candidate classify **Impact** (comprehension + AI collaboration), **Effort** (small/medium/
large), and **Risk** (what could break). Prioritize high impact + low effort, and note which fixes
unlock further improvements.

Present **5-15 specific candidates**. Two output modes:

- **Quick:** a written list — each with concrete problem, proposed fix (plain English, no code
  snippets), affected areas, effort. Then `AskUserQuestion` with `multiSelect` to choose.
- **Rich (optional):** a self-contained **HTML report** (Tailwind + Mermaid via CDN) written to the OS
  temp dir and opened (`xdg-open`/`open`/`start`), with before/after visualisations per candidate. See
  [HTML-REPORT.md](./HTML-REPORT.md). Nothing lands in the repo.

Use `CONTEXT.md` vocabulary for the domain and [LANGUAGE.md](./LANGUAGE.md) vocabulary for the
architecture. Do NOT propose interfaces yet. Ask: "Which of these would you like to explore?"

### 3. Grilling loop

Once the user picks a candidate, drop into a grilling conversation — walk the design tree (constraints,
dependencies, the shape of the deepened module, what sits behind the seam, what tests survive). See
[DEEPENING.md](./DEEPENING.md) for dependency categories and testing strategy, and
[INTERFACE-DESIGN.md](./INTERFACE-DESIGN.md) for the design-it-twice parallel sub-agent pattern.

Side effects happen inline as decisions crystallize:

- **Naming a deepened module after a concept not in `CONTEXT.md`?** Add the term to `CONTEXT.md` — same
  discipline as `/tt:interview-me` (see [../interview-me/CONTEXT-FORMAT.md](../interview-me/CONTEXT-FORMAT.md)).
- **User rejects a candidate with a load-bearing reason?** Offer to record it as a **GitHub `decision`
  issue** — _"Want me to record this as a decision issue so future architecture reviews don't re-suggest
  it?"_ We do NOT write in-repo ADRs (they drift). Format and the gate in
  [../interview-me/DECISION-ISSUE-FORMAT.md](../interview-me/DECISION-ISSUE-FORMAT.md). Only offer when
  the reason would actually be needed by a future explorer.

### 4. Output

For selected improvements, create **GitHub issues** (preferred — searchable, tied to outcome) via
`/tt:prd-to-issues` style slices, or save a plan to `docs/plans/YYYY-MM-DD-architecture-improvements.md`
if the user prefers. Favor incremental migration over big-bang rewrites.
