#!/usr/bin/env bash
# Blocks `gh pr merge` invocations that don't explicitly request --rebase.
#
# Why: this repo (ChrisTowles/towles-tool-rs) has squash and merge-commit
# disabled at the GitHub repo-settings level -- only rebase merges are
# allowed (confirmed via `gh repo view --json
# mergeCommitAllowed,squashMergeAllowed,rebaseMergeAllowed`). `gh pr merge`
# defaults to an interactive strategy prompt (or the repo default) when no
# strategy flag is given, and `--squash`/`--merge` fail outright with
# "Squash merges are not allowed on this repository." -- hit live merging
# PR #332. Catching this before the call avoids the failed round trip.
#
# Fails open: any parse hiccup here just allows the command through -- a
# guardrail hook should never be the reason a legitimate command breaks.
input=$(cat)
cmd=$(printf '%s' "$input" | jq -r '.tool_input.command // empty' 2>/dev/null) || exit 0
[ -z "$cmd" ] && exit 0

# Only `gh pr merge` (or the `gh merge` shorthand isn't a thing, but `gh pr
# merge` covers the real command and its common `gh pr merge <number>` form)
# is in scope.
if ! printf '%s\n' "$cmd" | grep -qE '(^|[;&|(])[[:space:]]*gh[[:space:]]+pr[[:space:]]+merge([[:space:]]|$)'; then
  exit 0
fi

# Already asking for rebase -- allow.
if printf '%s\n' "$cmd" | grep -qE '(^|[[:space:]])--rebase([[:space:]]|=|$)'; then
  exit 0
fi

reason="Blocked: this repo only allows rebase merges on GitHub (squash and merge-commit are disabled in repo settings -- confirmed via 'gh repo view --json mergeCommitAllowed,squashMergeAllowed,rebaseMergeAllowed'). Re-run with '--rebase --delete-branch' instead of --squash/--merge or no strategy flag."
jq -n --arg reason "$reason" '{hookSpecificOutput:{hookEventName:"PreToolUse",permissionDecision:"deny",permissionDecisionReason:$reason}}'
