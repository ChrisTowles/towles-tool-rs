#!/usr/bin/env bash
#
# PostToolUse guard: flag a clock reading used as a unique identifier.
#
# Two ids minted in the same millisecond are the same id, and the failure is
# silent — it presents as "one of them didn't happen". This has bitten the repo
# twice (window ids, openFile nonce); both now use the monotonic counters in
# `apps/client/src/lib/agentboard.ts`.
#
# Advisory only — it appends context rather than blocking, because the pattern
# is legitimate for timestamps and deadlines and this check keys on naming, not
# meaning. Like every hook here it fails open: any parse problem exits 0.

set -uo pipefail

input=$(cat) || exit 0
command -v jq >/dev/null 2>&1 || exit 0

file=$(printf '%s' "$input" | jq -r '.tool_input.file_path // empty' 2>/dev/null) || exit 0
[ -n "$file" ] || exit 0
[ -f "$file" ] || exit 0

# Frontend sources only. The Rust side mints ids by hashing a monotonic
# per-folder counter alongside the clock (see `tt-agentboard`'s `gen_id`), so
# it is not exposed to this.
case "$file" in
  *apps/client/src/*.ts | *apps/client/src/*.tsx) ;;
  *) exit 0 ;;
esac

# Two shapes: a clock interpolated into a string id (`\`w${Date.now()}\``), or
# a clock assigned to something *named* like an identifier. Deadlines and
# display timestamps (`until:`, `startedAt:`, `at:`) deliberately don't match.
hits=$(grep -nE \
  -e '\$\{[[:space:]]*Date\.now\(\)[[:space:]]*\}' \
  -e '\<(id|Id|ID|key|Key|nonce|Nonce|uuid|token)\>[[:space:]]*[:=][^=]*Date\.now\(\)' \
  "$file" 2>/dev/null | head -5) || exit 0

[ -n "$hits" ] || exit 0

reason="Possible time-based identifier in ${file}:

${hits}

Date.now() repeats within a millisecond, so two ids minted in the same tick
collide — and the symptom is a silent no-op, not an error. If this value is an
identity (an id, key, or a nonce whose *change* triggers an effect), use a
monotonic counter instead; apps/client/src/lib/agentboard.ts has nextWindowId()
and nextOpenFileNonce() as the local precedents. Ignore this if the value is a
timestamp or a deadline rather than an identity."

jq -n --arg r "$reason" \
  '{hookSpecificOutput:{hookEventName:"PostToolUse",additionalContext:$r}}' 2>/dev/null || exit 0
exit 0
