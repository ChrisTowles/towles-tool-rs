#!/usr/bin/env bash
# PostToolUse(Bash): if the command just run was a `gh pr` or `gh issue`
# mutation (merge/create/close/reopen/ready/edit/comment/review/draft/undraft
# for PRs; create/close/reopen/edit/comment/transfer for issues), nudge a
# running towles-tool app instance to refresh the matching view immediately
# instead of waiting for its normal poll interval (`tt collect nudge
# prs`/`tt collect nudge issues` -- see crates-tauri/tt-app/src/scheduler.rs's
# nudge-dir watch).
#
# Fails open throughout: this plugin can be enabled globally in Claude Code,
# so it runs for every Bash command in every project, not just towles-tool
# checkouts. Any parse hiccup, missing `jq`/`tt`, or a failed nudge just
# exits 0 -- a pure UI-responsiveness accelerant should never block or fail
# a Claude Code turn.
input=$(cat)
cmd=$(printf '%s' "$input" | jq -r '.tool_input.command // empty' 2>/dev/null) || exit 0
[ -z "$cmd" ] && exit 0

# Which nudge target(s) this command touches, and which verb matched -- the
# verb (not the full command, which may carry a PR body/token) is passed
# through to `tt collect nudge --trigger` so the resulting telemetry event
# names what fired without ever recording command content. Separator- or
# line-start-anchored (same heuristic as the repo's own
# .claude/hooks/guard-task-pkill.sh) so this never fires on a bare mention of
# the phrase inside prose, a commit message, or a code span. A chained
# command (`gh pr create && gh issue close 5`) can match both.
pr_verb=$(printf '%s\n' "$cmd" |
  grep -oE '(^|[;&|(])[[:space:]]*gh[[:space:]]+pr[[:space:]]+(merge|create|close|reopen|ready|edit|comment|review|draft|undraft)([[:space:]]|$)' |
  grep -oE '(merge|create|close|reopen|ready|edit|comment|review|draft|undraft)' | head -n1)
is_pr_command=0
[ -n "$pr_verb" ] && is_pr_command=1

issue_verb=$(printf '%s\n' "$cmd" |
  grep -oE '(^|[;&|(])[[:space:]]*gh[[:space:]]+issue[[:space:]]+(create|close|reopen|edit|comment|transfer)([[:space:]]|$)' |
  grep -oE '(create|close|reopen|edit|comment|transfer)' | head -n1)
is_issue_command=0
[ -n "$issue_verb" ] && is_issue_command=1

if [ "$is_pr_command" -ne 1 ] && [ "$is_issue_command" -ne 1 ]; then
  exit 0
fi

cwd=$(printf '%s' "$input" | jq -r '.cwd // empty' 2>/dev/null)
[ -z "$cwd" ] && cwd="${CLAUDE_PROJECT_DIR:-$PWD}"

# Only nudge when this session is actually towles-tool-relevant, via either
# signal:
#   1. An env value the app itself stamps into every terminal it spawns
#      (TT_SESSION_ID/TT_APP_INSTANCE -- crates-tauri/tt-app/src/terminal.rs).
#   2. `cwd` is inside a towles-tool-rs checkout (primary or a worktree
#      task), recognised the same way tt_config::task_scope_from_dir does:
#      a `crates/tt-config` directory at some ancestor.
# Without this, a hook enabled globally would still fire for `gh` commands
# run against completely unrelated projects in a plain tmux pane, silently
# nudging whichever towles-tool-app instance happens to be running unscoped.
# Guard-fail is intentionally not logged anywhere: it's the expected,
# high-volume case for a globally-enabled hook running against unrelated
# projects, and logging it would just be noise. The one audit trail this
# hook contributes is the `hook.nudge` telemetry event emitted by `tt collect
# nudge` itself below, which only happens once both the matcher and this
# guard have already passed.
in_app_terminal=0
[ -n "${TT_SESSION_ID:-}" ] && in_app_terminal=1
[ -n "${TT_APP_INSTANCE:-}" ] && in_app_terminal=1

in_checkout=0
dir="$cwd"
while [ -n "$dir" ] && [ "$dir" != "/" ]; do
  if [ -d "$dir/crates/tt-config" ]; then
    in_checkout=1
    break
  fi
  parent=$(dirname "$dir")
  # A malformed/relative `cwd` can make `dirname` stop shortening (e.g. "."
  # maps to itself) -- bail immediately rather than spin until the hook's
  # own timeout kills it.
  [ "$parent" = "$dir" ] && break
  dir="$parent"
done

if [ "$in_app_terminal" -ne 1 ] && [ "$in_checkout" -ne 1 ]; then
  exit 0
fi

command -v tt >/dev/null 2>&1 || exit 0
[ "$is_pr_command" -eq 1 ] && (cd "$cwd" && tt collect nudge prs --trigger "pr:$pr_verb" >/dev/null 2>&1)
[ "$is_issue_command" -eq 1 ] && (cd "$cwd" && tt collect nudge issues --trigger "issue:$issue_verb" >/dev/null 2>&1)
exit 0
