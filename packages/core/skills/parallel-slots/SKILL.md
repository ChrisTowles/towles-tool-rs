---
name: parallel-slots
description: Use when the user wants to dispatch parallel Claude Code agents across worktree slots of a repo, asks to "fan out", "run N in parallel", "use the slots", or wants to coordinate multiple isolated working copies of the same repo. Explains the nested .claude/worktrees layout, the `tt slot` lifecycle, when to fan out vs. work in a single slot, and the `gh`-driven workflow that ties slots together.
user_invocable: true
---

# Parallel slots

The slot pattern lets you run independent Claude Code sessions on the same
repo without stepping on each other. A slot is a branch-named **git worktree
nested inside the checkout** at `<checkout>/.claude/worktrees/<name>/` —
Claude Code's native worktree location — managed by `tt slot`, which adds
what raw worktrees lack: per-slot `.env` rendering with collision-free port
claims, a setup step (`TT_SLOT_SETUP` or lockfile-detected install), and
guarded removal (nothing with unsaved or unreachable work gets deleted).

## Layout

```
~/code/<scope>/<repo>/            # the main checkout — a normal clone
  .claude/worktrees/
    thing/                        # slot for branch feat/thing (.tt-slot marker at root)
    rail-overflow/                # slot for branch fix/rail-overflow
```

Any plain git checkout works — there is no layout to set up. All slots share
the main checkout's `.git`; branches survive slot removal.

## Lifecycle

```sh
tt slot new -b feat/thing [--base <ref>]  # create + render .env + setup
tt slot ls [--json]                       # fleet: branch, dirty count, ports
tt slot rm <name> [--force]               # guarded removal + docker cleanup
tt slot clean [--dry-run]                 # remove every merged/gone slot
```

Repos that wire `WorktreeCreate`/`WorktreeRemove` hooks to
`tt slot hook-create`/`hook-remove` in `.claude/settings.json` get the same
slots from Claude Code's own surfaces — `claude --worktree <name>` creates a
tt-managed slot on branch `feat/<name>` (tt names branches, never
`worktree-<name>`).

## When to fan out

Fan out (multiple slots) when:

- Three or more independent tasks would benefit from running simultaneously
  (e.g. one PR, one bug, one refactor).
- A task is risky and you want a clean, throwaway slot that won't pollute
  another slot's working tree.
- You're iterating on the agent harness itself and want to leave your
  current slot stable.

Stay in a single slot when:

- The work is sequential or all the changes need to land in the same commit.
- You're reading/exploring; spinning up more slots just adds overhead.

## Dispatch flow

1. `tt slot new -b <branch>` (or create from the Agentboard rail's `+`
   button: goal → branch → base, and Claude starts on the goal in the new
   slot's terminal). Branch off a GitHub issue with
   `gh issue develop <n>` first when one exists.
2. Hand the task to Claude in that slot.
3. Watch the Agentboard rail — it discovers every worktree of a tracked
   checkout automatically and surfaces needs-you status per slot.

## Coordination rules

- Never run two agents on the _same_ branch in two slots — push/pull races
  destroy work. One branch per slot, named after it.
- If a slot's working tree is dirty when you arrive, treat it as in-progress
  work — investigate before resetting.
- Never touch sibling slot directories; other agents work there concurrently.
- Ports come from the slot's rendered `.env` — never hardcode one.

## Verifying a slot's output

Before merging from a slot, run that repo's verify command (`/verify` in
towles-tool) inside it. Don't trust the slot's own self-report; the agent
that wrote the change is not the right reviewer.

## Shipping from a slot

Open the PR with `gh pr create` (use `--fill` to seed title/body from
commits, or pass `--title`/`--body` explicitly). Merge with
`gh pr merge --rebase --admin` — the standard merge style.

## Cleanup

After merging, `tt slot clean` removes every slot whose branch's work landed
(classic merge or squash-merge with the remote branch deleted) and deletes
the branch — guarded, so anything dirty or unpushed is kept and reported.
`tt slot rm <name>` for one-offs.

## Anti-patterns

- Spinning five slots on the same task "for redundancy". You'll spend the
  time merging conflicts.
- Treating slots as long-lived workspaces. They are scratch checkouts — keep
  them transient.
- Editing the same files in two slots at once. Stay in one slot for any
  given file.
- Raw `git worktree add`/`remove` in a tt-managed repo — you'd skip port
  claims, setup, and the removal guards.
