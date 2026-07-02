---
name: tdd
description: Implement a feature or fix using strict red-green-refactor TDD with vertical tracer-bullet slices. Use when asked to "write tests first", "use TDD", "red-green-refactor", "test-driven", or to build features/fix bugs test-first.
user_invocable: true
---

# Test-Driven Development

Implement using strict TDD. Red → Green → Refactor.

**Your first code output is always a test. Never implementation first.**

If the user passed a description or file as an argument, use it to write the first test.

## Philosophy

**Core principle:** tests verify behavior through public interfaces, not implementation details. Code
can change entirely; tests shouldn't.

**Good tests** are integration-style: they exercise real code paths through public APIs and read like a
specification ("user can checkout with valid cart"). They survive refactors. See [tests.md](./tests.md).

**Bad tests** are coupled to implementation — they mock internal collaborators, test private methods, or
verify through external means. The warning sign: the test breaks when you refactor but behavior hasn't
changed. See [mocking.md](./mocking.md) for what to mock (system boundaries only — and note `vi.mock` is
banned in this repo; use constructor dependency injection).

## Anti-pattern: horizontal slices

**DO NOT write all tests first, then all implementation.** Tests written in bulk test _imagined_
behavior — the _shape_ of things rather than user-facing behavior — and go insensitive to real changes.

```
WRONG (horizontal):  RED: test1..test5   then  GREEN: impl1..impl5
RIGHT (vertical):    RED→GREEN: test1→impl1, then test2→impl2, ...
```

Each test responds to what you learned from the previous cycle.

## Process

### 1. Planning

When exploring, use the project's domain glossary (`CONTEXT.md`) so test names and interface vocabulary
match the project's language, and respect decision issues in the area you're touching.

Before writing code:

- [ ] Confirm what interface changes are needed
- [ ] Confirm which behaviors to test (prioritize — you can't test everything)
- [ ] Identify opportunities for [deep modules](./deep-modules.md) (small interface, deep implementation)
- [ ] Design interfaces for [testability](./interface-design.md)
- [ ] List the behaviors to test (not implementation steps)

Only ask clarifying questions when you truly cannot write any meaningful test. For refactoring, always
write characterization tests for existing behavior BEFORE changing code — non-negotiable.

### 2. Red phase — confirm failure

- Write ONE test describing expected behavior. This is the ONLY code in this phase.
- Run it. Confirm it fails. Say: "Running the test to confirm it fails (Red phase)".
- Outline the plan (do NOT write implementation yet): "Next: Green — minimum code to pass; then
  Refactor — clean up, run all tests, commit." Test progression: happy path → edge cases → error
  handling → integration.
- STOP. Implementation comes in the next interaction.

A test that passes immediately is wrong — the test must fail before implementation.

### 3. Green phase — minimum code to pass

- Write the minimum code to pass. Run it. Confirm it passes. Say: "Running the test to confirm it passes
  (Green phase)".

### 4. Refactor phase

- Clean up with no behavior change. Look for [refactor candidates](./refactoring.md): extract
  duplication, deepen modules, move complexity behind simple interfaces.
- Run all tests — confirm nothing broke. **Never refactor while Red.**
- Commit. Each commit = one red-green-refactor cycle.

### 5. Repeat

Order: happy path → edge cases → error handling → integration points. One test at a time; only enough
code to pass the current test; don't anticipate future tests.

### 6. Final check

Run the full suite, `bun typecheck`, `bun run lint`. Commit.

## Checklist per cycle

```
[ ] Test describes behavior, not implementation
[ ] Test uses public interface only
[ ] Test would survive an internal refactor
[ ] Code is minimal for this test
[ ] No speculative features added
```
