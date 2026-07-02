---
name: handoff
description: Compact the current conversation into a handoff document for another agent to pick up. Use when the user wants to hand off work, summarise the session for a fresh agent, or mentions "handoff".
user_invocable: true
---

# Handoff

Write a handoff document summarising the current conversation so a fresh agent can continue the work.
Save it to the temporary directory of the user's OS — **not** the current workspace. Resolve the temp
dir from `$TMPDIR`, falling back to `/tmp` (or `%TEMP%` on Windows), and tell the user the absolute
path.

Include a **"Suggested skills"** section that names the `tt:` skills the next agent should invoke
(e.g. `/tt:diagnose` to continue a debugging loop, `/tt:tdd` to keep building test-first,
`/tt:prd-to-issues` to slice the remaining work).

Do not duplicate content already captured in other artifacts (PRDs, plans, decision issues, GitHub
issues, commits, diffs). Reference them by path or URL instead.

Redact any sensitive information — API keys, passwords, or personally identifiable information.

If the user passed arguments, treat them as a description of what the next session will focus on and
tailor the doc accordingly.
