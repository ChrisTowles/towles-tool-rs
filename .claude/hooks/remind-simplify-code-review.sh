#!/usr/bin/env bash
#
# Stop hook: reminds (does not block) to run /simplify and /code-review low
# --fix before wrapping up, when the working tree has uncommitted changes
# that haven't already been nudged about.
#
# This is deliberately advisory only, via additionalContext rather than
# decision:"block" -- see github.com/ChrisTowles/towles-tool-rs/issues/351 for
# the open question of whether to upgrade this to a hard block that actually
# forces the commands to run before Claude is allowed to stop. Dedupes on a
# hash of the actual diff content (tracked changes vs HEAD + untracked file
# paths) stored under this worktree's own .git dir, so it nudges once per
# change set instead of on every single Stop while the tree stays dirty --
# `git status --porcelain` alone isn't enough here since it only reports
# per-file flags (M/A/??), so a second edit to an already-dirty file would
# hash identically to the first and silently suppress the re-nudge. Fails
# open: any parse hiccup, missing tool, or non-git checkout just exits
# quietly -- a reminder hook should never be the reason a session breaks.

set -uo pipefail

input=$(cat) || exit 0
command -v jq >/dev/null 2>&1 || exit 0
command -v git >/dev/null 2>&1 || exit 0

dir=$(printf '%s' "$input" | jq -r '.cwd // empty' 2>/dev/null)
[ -n "$dir" ] || dir="${CLAUDE_PROJECT_DIR:-$PWD}"

git -C "$dir" rev-parse --is-inside-work-tree >/dev/null 2>&1 || exit 0

status=$(git -C "$dir" status --porcelain 2>/dev/null) || exit 0
[ -n "$status" ] || exit 0

tracked_diff=$(git -C "$dir" diff HEAD -- . 2>/dev/null)
untracked=$(git -C "$dir" ls-files --others --exclude-standard 2>/dev/null)
changeset="${tracked_diff}
${untracked}"

git_dir=$(git -C "$dir" rev-parse --git-dir 2>/dev/null) || exit 0
case "$git_dir" in
  /*) ;;
  *) git_dir="$dir/$git_dir" ;;
esac
marker="$git_dir/tt-simplify-review-nudge"

if command -v sha256sum >/dev/null 2>&1; then
  hash=$(printf '%s' "$changeset" | sha256sum | cut -d' ' -f1)
elif command -v shasum >/dev/null 2>&1; then
  hash=$(printf '%s' "$changeset" | shasum -a 256 | cut -d' ' -f1)
else
  hash=$(printf '%s' "$changeset" | cksum | cut -d' ' -f1)
fi
[ -n "$hash" ] || exit 0

if [ -f "$marker" ] && [ "$(cat "$marker" 2>/dev/null)" = "$hash" ]; then
  exit 0
fi
printf '%s' "$hash" >"$marker" 2>/dev/null

reason="This turn left uncommitted changes in the working tree. Before finishing, run /simplify to clean up the changed code, then /code-review low --fix to catch and auto-fix correctness/reuse issues. This is a soft reminder, not enforced -- see github.com/ChrisTowles/towles-tool-rs/issues/351 for whether this should become a hard block instead."

jq -n --arg r "$reason" '{hookSpecificOutput:{hookEventName:"Stop",additionalContext:$r}}' 2>/dev/null || exit 0
