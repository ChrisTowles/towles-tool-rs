---
name: write-prd
description: Transform a conversation or idea into a structured PRD with user stories, testing seams, and implementation decisions. Use when the user wants to create a PRD, write a spec, or turn the current context into a product requirements doc.
user_invocable: true
---

# Write a PRD

Create a Product Requirements Document from the current conversation or a provided description.

If the user passed a file, path, or description as an argument, read it fully first.

## Process

1. **Gather context first** — ALWAYS ask 3-5 clarifying questions via `AskUserQuestion` BEFORE writing
   any PRD. Never skip this step. Do NOT output PRD sections until you have answers. Ask about:
   - **Users**: target users/personas?
   - **Scope & non-goals**: what's in/out of scope?
   - **Success criteria**: measurable acceptance criteria?
   - **Technical specifics**: APIs, constraints, current state?
   - **Current state**: what exists today?
2. **Explore the codebase** — understand the current state. Use the project's domain glossary
   (`CONTEXT.md`) vocabulary throughout the PRD, and respect existing decision issues in the area
   you're touching.
3. **Sketch the seams** — before drafting, identify the seams at which you'll _test_ the feature.
   Prefer existing seams to new ones. Use the highest seam possible. If new seams are needed, propose
   them at the highest point you can. **Confirm the seams match the user's expectations before
   writing.** These feed the Testing Decisions section.
4. **Draft the PRD** — only after answers + seams, use the template below.
5. **Present for review** — show the draft, get feedback via `AskUserQuestion`.
6. **Output** — ask preference: save to `docs/plans/YYYY-MM-DD-<feature-name>.md` or a GitHub issue.

## PRD Template

```markdown
# [Feature Name]

## Problem Statement

What problem does this solve? Who has it? From the user's perspective.

## Goals

- Goal 1

## Non-Goals

- Explicitly out of scope

## User Stories

A long, numbered list. Each: "As a [user], I want [action], so that [outcome]". Cover all aspects of
the feature — be extensive.

## Acceptance Criteria

- [ ] Criterion 1 (specific and testable)

## Implementation Decisions

The modules built/modified, interfaces changed, architectural decisions, schema changes, API
contracts. Do NOT include specific file paths or code snippets — they go stale fast. (Exception: a
prototype-produced snippet that encodes a decision more precisely than prose — a state machine,
reducer, schema, or type shape — may be inlined, trimmed to the decision-rich parts, noted as
prototype-sourced.)

## Testing Decisions

- What makes a good test here (test external behavior, not implementation details)
- The seams at which the feature will be tested (from the Sketch step)
- Which modules will be tested
- Prior art — similar tests already in the codebase

## Open Questions

- Anything unresolved
```

## Rules

- **Questions first, always** — the first response must be clarifying questions, never a PRD draft.
- **User stories are mandatory** — every feature maps to at least one.
- **Acceptance Criteria are mandatory** — specific, testable. Write them before Implementation Decisions.
- **No file paths or code snippets** in Implementation Decisions (prototype-snippet exception aside).
- Use `CONTEXT.md` vocabulary; respect decision issues in the area.
- Keep it concise — 1-3 pages, not a novel.
