#!/usr/bin/env bash
# Blocks `tsc --noEmit` as a typecheck of apps/client.
#
# Why: apps/client builds with `tsc -b` (project references, see its
# tsconfig). `tsc --noEmit` does not build referenced projects, so it happily
# reports success while the real build fails -- it has already passed a stale
# prop type straight through to a broken `npm run build`. The only command
# that actually typechecks this workspace is its build script.
#
# Fails open: any parse hiccup here just allows the command through -- a
# guardrail hook should never be the reason a legitimate command breaks.
input=$(cat)
cmd=$(printf '%s' "$input" | jq -r '.tool_input.command // empty' 2>/dev/null) || exit 0
[ -z "$cmd" ] && exit 0

# Only the no-emit form is in scope: `tsc -b`, `tsc --build`, and a plain
# `tsc` invoked by a package script are all fine.
printf '%s\n' "$cmd" | grep -qE '(^|[;&|(])[[:space:]]*(npx[[:space:]]+)?tsc([[:space:]]|$)' || exit 0
printf '%s\n' "$cmd" | grep -qE -- '--noEmit' || exit 0

reason="Blocked: \`tsc --noEmit\` does not typecheck apps/client. That workspace uses project references and builds with \`tsc -b\`, which --noEmit skips -- it reports success on code the real build rejects. Run \`npm run build --workspace @towles-tool/client\` from the repo root instead (it runs \`tsc -b && vite build\`, so it is both the typecheck and the build)."
jq -n --arg reason "$reason" '{hookSpecificOutput:{hookEventName:"PreToolUse",permissionDecision:"deny",permissionDecisionReason:$reason}}'
