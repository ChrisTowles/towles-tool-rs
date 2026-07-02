---
name: parallel-slots
description: Use when the user wants to dispatch parallel Claude Code agents across slot clones of a repo, asks to "fan out", "run N in parallel", "use the slots", or wants to coordinate multiple isolated working copies of the same repo. Explains the slot directory layout, when to fan out vs. work in a single slot, and the `gh`-driven workflow that ties slots together.
user_invocable: true
---

# Parallel slots

The slot pattern lets you run independent Claude Code sessions on the same repo without stepping on each other. Mirrors Boris Cherny's "5 terminal tabs, each a separate git checkout" workflow.

## Layout

```
~/code/<scope>/<repo>-repos/
  <repo>-slot-1/    # interchangeable slot — interactive or agent work
  <repo>-slot-2/
  <repo>-slot-3/
  <repo>-slot-4/
  <repo>-slot-5/
```

Each slot is a full clone of the same GitHub remote, not a worktree. They check out branches independently. Use the `gh` CLI for all GitHub-side operations (issue → branch, PR create, PR merge, status). If the repo ships a tmux sidebar (e.g. AgentBoard in towles-tool), it watches every slot and surfaces completion via the stop-hook sweep.

## When to fan out

Fan out (use multiple slots) when:

- Three or more independent tasks would benefit from running simultaneously (e.g. one PR, one bug, one refactor).
- A task is risky and you want a clean, throwaway slot that won't pollute another slot's working tree.
- You're iterating on the agent harness itself and want to leave your current slot stable.

Stay in a single slot when:

- The work is sequential or all the changes need to land in the same commit.
- You're reading/exploring; spinning up more slots just adds overhead.

## Dispatch flow

1. Pick a free slot (any slot whose sidebar pane is idle).
2. `cd` into it and confirm the working tree is clean.
3. Branch off from a GitHub issue: `gh issue develop <issue-number> --checkout` — creates a remote branch tied to the issue and switches the slot to it. If there's no issue, name the branch and use `gh pr checkout <pr>` later if you need to hop onto a colleague's PR.
4. Hand the task to Claude in that slot via the repo's sidebar TUI.
5. Watch the sidebar pane for completion. The stop-hook prints results back to it.

## Coordination rules

- Never run two agents on the _same_ branch in two slots — push/pull races destroy work.
- Branch names should be unique per slot for the duration of the run.
- If a slot's working tree is dirty when you arrive, treat it as in-progress work — investigate before resetting.
- Pre-commit hooks (format + lint + typecheck) run in every slot, so `--no-verify` is forbidden.

## Verifying a slot's output

Before merging from a slot, run that repo's verify command (`/verify` in towles-tool) inside it. Don't trust the slot's own self-report; the agent that wrote the change is not the right reviewer.

## Shipping from a slot

Open the PR with `gh pr create` (use `--fill` to seed title/body from commits, or pass `--title`/`--body` explicitly). Merge with `gh pr merge --rebase --admin` — the standard merge style.

## Cleanup

After a slot's branch is merged: confirm with `gh pr status` that the slot's PR is merged, then prune the local branch. Use `compound-engineering:ce-clean-gone-branches` to bulk-prune across multiple slots in one pass.

## Anti-patterns

- Spinning all 5 slots on the same task "for redundancy". You'll spend the time merging conflicts.
- Treating slots as long-lived workspaces. They are scratch checkouts — keep them transient.
- Editing the same files in two slots at once. Stay in one slot for any given file.
