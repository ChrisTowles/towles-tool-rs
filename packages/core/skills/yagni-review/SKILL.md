---
name: yagni-review
description: Code review focused exclusively on over-engineering. Finds what to delete — reinvented standard library, unneeded dependencies, speculative abstractions, dead flexibility. One line per finding with location, what to cut, and what replaces it. Use when the user says "yagni review", "review for over-engineering", "what can we delete", "is this over-engineered", "simplify review", or invokes /yagni-review. Complements correctness-focused review — this pass only hunts complexity.
user_invocable: true
---

<!--
Concept adapted from the "ponytail-review" skill by Dietrich Gebert (MIT):
https://github.com/DietrichGebert/ponytail
-->

# YAGNI Review

Review the diff (or the files the user points at) for unnecessary complexity.
One line per finding: location, what to cut, what replaces it. The diff's best
outcome is getting shorter.

If no target is given, review the current branch's diff against the default
branch (`git diff main...HEAD`); fall back to staged/unstaged changes if the
branch is clean.

## Format

`<file>:L<line>: <tag> <what>. <replacement>.`

Tags:

- `delete:` — dead code, unused flexibility, speculative feature. Replacement: nothing.
- `stdlib:` — hand-rolled thing the standard library (or bun built-in) ships. Name the function.
- `native:` — dependency or code doing what the platform already does. Name the feature.
- `yagni:` — abstraction with one implementation, config nobody sets, layer with one caller, option no caller passes.
- `shrink:` — same logic, fewer lines. Show the shorter form.

## Examples

❌ "This EmailValidator class might be more complex than necessary, have you
considered whether all these validation rules are needed at this stage?"

✅ `src/email.ts:L12-38: stdlib: 27-line validator class. "@" in email, 1 line — real validation is the confirmation mail.`

✅ `src/dates.ts:L4: native: moment.js imported for one format call. Intl.DateTimeFormat, 0 deps.`

✅ `src/repo.ts:L88: yagni: AbstractRepository with one implementation. Inline it until a second one exists.`

✅ `src/fetch.ts:L52-71: delete: retry wrapper around an idempotent local call. Nothing replaces it.`

✅ `src/map.ts:L30-44: shrink: manual loop builds record. Object.fromEntries(keys.map((k, i) => [k, values[i]])), 1 line.`

## Scoring

End with the only metric that matters: `net: -<N> lines possible.`

If there is nothing to cut, say `Lean already. Ship.` and stop.

## Boundaries

- Complexity only — correctness bugs, security holes, and performance go to a
  normal correctness-focused review pass, not this one.
- Tests are not bloat. A smoke test or assert-based self-check is the minimum,
  never a finding. Only flag tests that test mocks instead of behavior.
- Dependency injection at a real system boundary (this repo's testing
  convention) is not a `yagni:` finding — DI with exactly one production
  implementation and a test fake is the intended pattern here.
- Does not apply the fixes, only lists them. The user picks what to cut.
