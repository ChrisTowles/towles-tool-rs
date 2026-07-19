#!/usr/bin/env bash
# Blocks `gh pr merge` invocations that request a merge strategy GitHub will
# reject, and names the strategies that actually work.
#
# Why: which strategies a repo permits is *mutable remote state*, so this hook
# reads it live via `gh repo view` rather than hardcoding it. The previous
# version baked in "rebase only" (true when written against PR #332); the repo
# later became squash-only, and the stale hook then blocked the one strategy
# that worked while recommending the one GitHub rejects -- see issue #348. Any
# guard that asserts a remote setting from memory drifts the moment it changes,
# so the check has to be the query, not a comment citing one.
#
# Fails open everywhere: an unparseable payload, a missing `gh`, a network
# blip, or an unreadable response all allow the command through. A guardrail
# hook must never be the reason a legitimate merge breaks -- a wrong strategy
# just costs one failed round trip, but a false denial costs trust in the hook.
input=$(cat)
cmd=$(printf '%s' "$input" | jq -r '.tool_input.command // empty' 2>/dev/null) || exit 0
[ -z "$cmd" ] && exit 0

# Only `gh pr merge` is in scope.
if ! printf '%s\n' "$cmd" | grep -qE '(^|[;&|(])[[:space:]]*gh[[:space:]]+pr[[:space:]]+merge([[:space:]]|$)'; then
  exit 0
fi

# Which strategy is this command asking for? No flag means "repo default or
# interactive prompt", which we treat as unspecified.
requested=""
printf '%s\n' "$cmd" | grep -qE '(^|[[:space:]])--squash([[:space:]]|=|$)' && requested="squash"
printf '%s\n' "$cmd" | grep -qE '(^|[[:space:]])--rebase([[:space:]]|=|$)' && requested="rebase"
printf '%s\n' "$cmd" | grep -qE '(^|[[:space:]])--merge([[:space:]]|=|$)' && requested="merge"

# Ask GitHub what it will actually accept. Any failure here fails open.
settings=$(gh repo view --json mergeCommitAllowed,squashMergeAllowed,rebaseMergeAllowed 2>/dev/null) || exit 0
[ -z "$settings" ] && exit 0

allowed=$(printf '%s' "$settings" | jq -r '
  [ (if .squashMergeAllowed then "squash" else empty end)
  , (if .rebaseMergeAllowed then "rebase" else empty end)
  , (if .mergeCommitAllowed then "merge"  else empty end)
  ] | join(" ")' 2>/dev/null) || exit 0

# No readable answer, or a repo that somehow permits nothing -- stay out of it.
[ -z "$allowed" ] && exit 0

# An explicit, permitted strategy: nothing to say.
if [ -n "$requested" ] && printf '%s\n' "$allowed" | grep -qw "$requested"; then
  exit 0
fi

flags=$(printf '%s\n' "$allowed" | tr ' ' '\n' | sed 's/^/--/' | paste -sd' ' -)

if [ -n "$requested" ]; then
  reason="Blocked: '--$requested' is disabled for this repo on GitHub. Allowed strategy flags: $flags. Re-run with one of those."
else
  # Unspecified: only worth interrupting when the choice is forced anyway,
  # otherwise let gh prompt or apply the repo default as usual.
  count=$(printf '%s\n' "$allowed" | wc -w | tr -d '[:space:]')
  [ "$count" != "1" ] && exit 0
  reason="Blocked: no merge strategy given, and this repo permits only '$flags'. Re-run with '$flags' so the strategy is explicit."
fi

jq -n --arg reason "$reason" '{hookSpecificOutput:{hookEventName:"PreToolUse",permissionDecision:"deny",permissionDecisionReason:$reason}}'
