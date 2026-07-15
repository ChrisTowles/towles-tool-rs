#!/usr/bin/env bash
# Blocks pkill/killall (or `kill $(pgrep ...)`) commands that target a
# dev-server process name shared across every worktree slot (tt-app, vite,
# tauri) without scoping the pattern to this slot's own directory name.
#
# Why: pkill/killall match by command-line (or bare process name) substring
# across the WHOLE OS process table -- they know nothing about cwd, ports, or
# which worktree slot spawned a process. Every slot in this repo runs its own
# `target/debug/tt-app` and `vite` on its own port (see CLAUDE.md's "Worktree
# slots" section), so an unscoped `pkill -f "target/debug/tt-app"` kills every
# sibling slot's dev server too, not just the one you meant to stop.
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

# Process-name substrings known to be identical across every slot's dev build.
risky='tt-app|vite|dev-drive|tauri'
if ! printf '%s\n' "$cmd" | grep -qE "$risky"; then
  exit 0
fi

slotdir=$(printf '%s' "$input" | jq -r '.cwd // empty' 2>/dev/null)
[ -z "$slotdir" ] && slotdir="${CLAUDE_PROJECT_DIR:-$PWD}"
slot=$(basename "$slotdir")

# Already scoped -- the slot's own directory name appears in the pattern --
# so it can only match this slot's processes. Allow.
if printf '%s' "$cmd" | grep -qF "$slot"; then
  exit 0
fi

reason="Blocked: this pkill/killall pattern matches a process name (tt-app/vite/tauri) that is identical across every worktree slot in this repo -- it would kill sibling slots' dev servers too, not just $slot's. Scope it instead: use pkill -f with '$slot' in the pattern (e.g. pkill -f \"$slot.*tt-app\"; killall can't be scoped since it matches bare process names only), or find and kill the exact PID with pgrep -af \"$slot.*tt-app\"."
jq -n --arg reason "$reason" '{hookSpecificOutput:{hookEventName:"PreToolUse",permissionDecision:"deny",permissionDecisionReason:$reason}}'
