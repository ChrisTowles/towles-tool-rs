---
name: prd-to-issues
description: Break a PRD, plan, or spec into independently-grabbable GitHub issues using tracer-bullet vertical slices, classified HITL vs AFK with dependency ordering. Use when the user wants to convert a plan into issues, create implementation tickets, or break work into issues.
user_invocable: true
---

# PRD → Issues

Break a plan into independently-grabbable GitHub issues using **vertical slices (tracer bullets)**.

If the user passed an issue reference, file path, or PRD path as an argument, fetch and read it fully
(body + comments).

## Process

### 1. Gather context

Work from whatever is already in the conversation. Otherwise find the PRD — read the given path or
check `docs/plans/`. If none exists, suggest `/tt:write-prd`.

### 2. Get repo and labels

- `gh repo view --json nameWithOwner --jq '.nameWithOwner'`
- `gh label list --json name --jq '.[].name'` — discover the real label vocabulary.

If you have not explored the codebase yet, do so. Issue titles/descriptions should use the project's
domain glossary (`CONTEXT.md`) vocabulary and respect decision issues in the area.

### 3. Draft vertical slices (tracer bullets)

Each issue is a thin vertical slice that cuts through ALL integration layers end-to-end — NOT a
horizontal slice of one layer.

- Each slice delivers a narrow but COMPLETE path through every layer (schema, API, UI, tests).
- A completed slice is demoable or verifiable on its own.
- Prefer many thin slices over few thick ones; riskiest unknowns early.

**Classify each slice HITL or AFK:**

- **HITL** — requires human interaction (an architectural decision, a design review, manual testing).
- **AFK** — can be implemented and merged without human interaction.

Prefer AFK over HITL where possible.

### 4. Quiz the user

Present the breakdown as a numbered list. For each slice show:

- **Title** — short descriptive name
- **Type** — HITL / AFK
- **Blocked by** — which slices must complete first
- **User stories covered** — which stories this addresses (if the source has them)

Ask: Does the granularity feel right (too coarse / too fine)? Are the dependencies correct? Should any
slices merge or split? Are HITL/AFK marked correctly? Iterate until approved.

### 5. Create the issues

Publish in **dependency order** (blockers first) so you can reference real issue numbers in "Blocked
by". Use `gh issue create` with:

- Title prefix: `feat:`, `fix:`, `refactor:`, `chore:`
- Real repo labels (from step 2). For **AFK** slices, apply the `ready-for-agent` label if it exists
  (map the canonical name to the repo's actual label; ask if unclear).
- The body template below.

Do NOT close or modify any parent issue.

### 6. Report

A table with issue URLs and the dependency graph.

## Issue Body Template

```markdown
## Parent

Reference to the parent issue (omit if the source wasn't an existing issue).

## What to build

A concise description of this vertical slice — the end-to-end behavior, not layer-by-layer
implementation. Avoid file paths and code snippets (they go stale). Exception: a prototype-produced
snippet that encodes a decision precisely (state machine, reducer, schema, type shape) may be inlined,
trimmed, and noted as prototype-sourced.

## Acceptance Criteria

- [ ] Criterion 1
- [ ] Criterion 2

## Blocked by

- #N (or "None — can start immediately")
```

## Rules

- Vertical slices, not horizontal layers.
- Each issue completable in a single session.
- Riskiest slices first.
- Prefer AFK over HITL.
