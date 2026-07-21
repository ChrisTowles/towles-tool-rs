#!/usr/bin/env bash
# Blocks pkill/killall (or `kill $(pgrep ...)`) commands that target a
# dev-server process name shared across every worktree task (tt-app, vite,
# tauri) without scoping the pattern to this task's own directory name.
#
# Why: pkill/killall match by command-line (or bare process name) substring
# across the WHOLE OS process table -- they know nothing about cwd, ports, or
# which worktree task spawned a process. Every task in this repo runs its own
# `target/debug/tt-app` and `vite` on its own port (see CLAUDE.md's "Worktree
# tasks" section), so an unscoped `pkill -f "target/debug/tt-app"` kills every
# sibling task's dev server too, not just the one you meant to stop.
#
# Fails open: any parse hiccup here just allows the command through -- a
# guardrail hook should never be the reason a legitimate command breaks.
input=$(cat)
cmd=$(printf '%s' "$input" | jq -r '.tool_input.command // empty' 2>/dev/null) || exit 0
[ -z "$cmd" ] && exit 0

# Only broad kill-by-name forms are in scope, and only where pkill/killall is
# actually a command being invoked -- immediately after a real shell
# separator (;, &&, ||, |, "(") or at the very start of a line -- never a
# bare mention of the word inside prose, a markdown code span, or a quoted
# PR-body/commit-message argument (grep's default per-line `^` handles the
# "start of a line inside a multi-line string" case for free). `kill <pid>`
# is always fine and never matches this.
if ! printf '%s\n' "$cmd" | grep -qE '(^|[;&|(])[[:space:]]*(pkill|killall)([[:space:]]|$)|(^|[;&|(])[[:space:]]*kill[[:space:]]+.*\$\([[:space:]]*pgrep'; then
  exit 0
fi

# Process-name substrings known to be identical across every task's dev build.
risky='tt-app|vite|dev-drive|tauri'
if ! printf '%s\n' "$cmd" | grep -qE "$risky"; then
  exit 0
fi

taskdir=$(printf '%s' "$input" | jq -r '.cwd // empty' 2>/dev/null)
[ -z "$taskdir" ] && taskdir="${CLAUDE_PROJECT_DIR:-$PWD}"
task=$(basename "$taskdir")

# Already scoped -- the task's own directory name appears in the pattern --
# so it can only match this task's processes. Allow.
if printf '%s' "$cmd" | grep -qF "$task"; then
  exit 0
fi

reason="Blocked: this pkill/killall pattern matches a process name (tt-app/vite/tauri) that is identical across every worktree task in this repo -- it would kill sibling tasks' dev servers too, not just $task's. Scope it instead: use pkill -f with '$task' in the pattern (e.g. pkill -f \"$task.*tt-app\"; killall can't be scoped since it matches bare process names only), or find and kill the exact PID with pgrep -af \"$task.*tt-app\"."
jq -n --arg reason "$reason" '{hookSpecificOutput:{hookEventName:"PreToolUse",permissionDecision:"deny",permissionDecisionReason:$reason}}'
