#!/usr/bin/env bash
# PostToolUse(Bash): if the command just run was `gh pr merge` or `gh pr
# create`, nudge a running towles-tool app instance to refresh its PR data
# immediately instead of waiting for its normal poll interval (`tt collect
# nudge` -- see crates-tauri/tt-app/src/scheduler.rs's nudge-dir watch).
#
# Fails open throughout: this plugin can be enabled globally in Claude Code,
# so it runs for every Bash command in every project, not just towles-tool
# checkouts. Any parse hiccup, missing `jq`/`tt`, or a failed nudge just
# exits 0 -- a pure UI-responsiveness accelerant should never block or fail
# a Claude Code turn.
input=$(cat)
cmd=$(printf '%s' "$input" | jq -r '.tool_input.command // empty' 2>/dev/null) || exit 0
[ -z "$cmd" ] && exit 0

# Only an actual `gh pr merge`/`gh pr create` invocation is in scope --
# separator- or line-start-anchored (same heuristic as the repo's own
# .claude/hooks/guard-slot-pkill.sh) so this never fires on a bare mention of
# the phrase inside prose, a commit message, or a code span.
if ! printf '%s\n' "$cmd" | grep -qE '(^|[;&|(])[[:space:]]*gh[[:space:]]+pr[[:space:]]+(merge|create)([[:space:]]|$)'; then
  exit 0
fi

cwd=$(printf '%s' "$input" | jq -r '.cwd // empty' 2>/dev/null)
[ -z "$cwd" ] && cwd="${CLAUDE_PROJECT_DIR:-$PWD}"

# Only nudge when this session is actually towles-tool-relevant, via either
# signal:
#   1. An env value the app itself stamps into every terminal it spawns
#      (TT_SESSION_ID/TT_APP_INSTANCE -- crates-tauri/tt-app/src/terminal.rs).
#   2. `cwd` is inside a towles-tool-rs checkout (primary or a worktree
#      slot), recognised the same way tt_config::slot_scope_from_dir does:
#      a `crates/tt-config` directory at some ancestor.
# Without this, a hook enabled globally would still fire for `gh` commands
# run against completely unrelated projects in a plain tmux pane, silently
# nudging whichever towles-tool-app instance happens to be running unscoped.
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
(cd "$cwd" && tt collect nudge >/dev/null 2>&1)
exit 0
