#!/usr/bin/env bash
#
# PostToolUse guard: keep TypeScript error handling on the better-result path.
#
# `apps/client/src/lib/tauri.ts` is the single IPC boundary: its `invoke`
# returns `Result<T, IpcError>` and never throws, so callers pick their own
# failure UX instead of inheriting one. The failure mode this guards against is
# quiet drift back to the old shapes — a direct `@tauri-apps/api/core` import
# that skips the boundary, or a `.catch`/`String(e)` habit carried over from
# before. None of that breaks the build, which is exactly why it needs a check.
#
# Advisory only — it appends context rather than blocking, because every
# pattern here has a legitimate exception (foreign throwing APIs, non-error
# `String()` calls). Like every hook here it fails open: any parse problem
# exits 0.

set -uo pipefail

input=$(cat) || exit 0
command -v jq >/dev/null 2>&1 || exit 0

file=$(printf '%s' "$input" | jq -r '.tool_input.file_path // empty' 2>/dev/null) || exit 0
[ -n "$file" ] || exit 0
[ -f "$file" ] || exit 0

case "$file" in
  *apps/client/src/*.ts | *apps/client/src/*.tsx) scope=client ;;
  *scripts/*.mjs) scope=scripts ;;
  *) exit 0 ;;
esac

findings=""
add() { findings="${findings}${findings:+
}$1
$2"; }

# Drop comment/doc lines — these checks key on syntax, and prose that merely
# names a discouraged pattern (including this repo's own docstrings explaining
# why it is discouraged) is not a violation.
code_only() { grep -vE '^[0-9]+:[[:space:]]*(//|\*|/\*)'; }

# The four wrappers this repo deliberately collapsed into one `invoke`. Their
# differing failure semantics were the original footgun; a new one reopens it.
hits=$(grep -nE '\<(invokeCmd|invokeOrThrow|invokeToast|invokeOk)\>' "$file" 2>/dev/null | code_only | head -3)
[ -n "$hits" ] && add "Resurrected invoke wrapper:" "$hits"

# oxlint's no-underscore-dangle rejects `._tag` as an error; the tagged-error
# classes expose a type guard for this.
hits=$(grep -nE '\._tag[[:space:]]*[!=]==' "$file" 2>/dev/null | code_only | head -3)
[ -n "$hits" ] && add "Raw ._tag comparison — use the class guard, e.g. NotInTauri.is(e):" "$hits"

if [ "$scope" = client ]; then
  # `String(e)` degrades to "[object Object]" on a rejected Tauri command
  # (Tauri rejects with a bare string) and on any non-Error throw. Scoped to
  # the client because `errorMessage` lives there; `lib/errors.ts` defines it,
  # so its own last-resort String() call is the implementation, not a lapse.
  case "$file" in
    *apps/client/src/lib/errors.ts) ;;
    *)
      hits=$(grep -nE '\<String\((e|err|error|ex|cause)\)' "$file" 2>/dev/null | code_only | head -3)
      [ -n "$hits" ] && add "String() on an error — use errorMessage() from @/lib/errors:" "$hits"
      ;;
  esac

  # Everything must route through lib/tauri.ts so failures stay typed. That
  # file is the one legitimate importer.
  case "$file" in
    *apps/client/src/lib/tauri.ts) ;;
    *)
      hits=$(grep -n '@tauri-apps/api/core' "$file" 2>/dev/null | code_only | head -3)
      [ -n "$hits" ] && add "Direct @tauri-apps/api/core import bypasses the Result boundary:" "$hits"
      ;;
  esac

  # `invoke` never rejects, so a .catch chained onto one can never run.
  hits=$(grep -nE 'invoke[A-Za-z]*\(.*\)[[:space:]]*\.catch|\.catch\(.*\).*invoke\(' "$file" 2>/dev/null | code_only | head -3)
  [ -n "$hits" ] && add "Dead .catch on invoke — it never rejects; branch on the Result:" "$hits"
fi

[ -n "$findings" ] || exit 0

reason="better-result convention drift in ${file}:

${findings}

apps/client/src/lib/tauri.ts's \`invoke\` returns Result<T, IpcError> and never
throws or rejects — every failure (no Tauri host, rejected command, schema
mismatch, timeout) is a typed Err. Handle it at the call site: .unwrapOr(fallback)
to degrade, .match({ ok, err }) to branch, .isErr() to test. Fire-and-forget needs
no .catch, since an ignored Result cannot produce an unhandled rejection.

Ignore any line above where the exception is real: a genuinely throwing foreign
API (navigator.clipboard, monaco-languageclient's dispose) still needs .catch,
monaco-fs.ts deliberately keeps a throwing contract at the IFileSystemProvider
boundary, and String() on a non-error value is fine."

jq -n --arg r "$reason" \
  '{hookSpecificOutput:{hookEventName:"PostToolUse",additionalContext:$r}}' 2>/dev/null || exit 0
exit 0
