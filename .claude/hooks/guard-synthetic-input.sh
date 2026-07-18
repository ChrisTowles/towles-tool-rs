#!/usr/bin/env bash
# Blocks verifying app behavior by dispatching synthetic input events into the
# webview (drive.mjs eval / e2e with `new ClipboardEvent`, `new KeyboardEvent`,
# `new DragEvent`, `new DataTransfer`).
#
# Why: a synthetic event enters the DOM *below* the platform layer, which in
# this app is usually the layer that is actually broken. This has already
# shipped a dead feature: image paste was verified by dispatching a
# ClipboardEvent carrying a hand-built DataTransfer, which exercised the
# handler and passed -- while the real feature did nothing, because on Linux a
# Ctrl+V image paste never reaches the webview's `paste` event at all (see
# `read_clipboard_image` in crates-tauri/tt-app/src/slots.rs, and the same note
# in components/terminal-view.tsx). The synthetic event proved only that the
# handler works when handed data the platform never hands it.
#
# Verify through something real instead:
#   - put real data on the real clipboard (wl-copy) and invoke the host
#     command (drive.mjs invoke read_clipboard_image), or click the real UI
#     affordance (drive.mjs click)
#   - for handler logic in isolation, write a vitest unit test on the pure
#     function in lib/ -- that is honest about being a unit test, whereas a
#     synthetic event in the real shell reads like end-to-end proof
#
# Escape hatch: prefix the command with SYNTHETIC_INPUT_OK=1 when the
# dispatch genuinely is a probe (e.g. asking "does this event fire at all?")
# rather than verification that a feature works.
#
# Fails open: any parse hiccup here just allows the command through -- a
# guardrail hook should never be the reason a legitimate command breaks.
input=$(cat)
cmd=$(printf '%s' "$input" | jq -r '.tool_input.command // empty' 2>/dev/null) || exit 0
[ -z "$cmd" ] && exit 0

# Deliberate override, stated in the command itself.
printf '%s\n' "$cmd" | grep -qE '(^|[[:space:]])SYNTHETIC_INPUT_OK=1([[:space:]]|$)' && exit 0

# Only when driving the real app -- a synthetic event inside a vitest file is
# a unit test and reads as one.
printf '%s\n' "$cmd" | grep -qE 'drive\.mjs|npm run (e2e|dev:drive)' || exit 0

# The platform-input family: events whose real-world delivery depends on the
# OS/compositor/WebKit, so a fabricated one proves nothing about the feature.
# (MouseEvent/click are deliberately not listed -- those dispatch reliably and
# drive.mjs's own `click` verb is built on them.)
printf '%s\n' "$cmd" |
  grep -qE 'new[[:space:]]+(ClipboardEvent|KeyboardEvent|DragEvent|InputEvent|DataTransfer)' ||
  exit 0

reason="Blocked: this verifies behavior with a synthetic input event, which enters the DOM below the platform layer -- the layer most likely to be the broken one. This exact pattern already shipped a dead feature: image paste passed a synthetic ClipboardEvent test while the real Ctrl+V did nothing, because on Linux an image paste never reaches the webview's paste event. Verify through something real instead: put real data on the real clipboard (wl-copy) and call the host command (node scripts/drive.mjs invoke <cmd>), or click the real affordance (node scripts/drive.mjs click). For handler logic alone, write a vitest unit test on the pure function in apps/client/src/lib/. If this dispatch really is a diagnostic probe rather than feature verification, re-run it with SYNTHETIC_INPUT_OK=1 as a command prefix."
jq -n --arg reason "$reason" '{hookSpecificOutput:{hookEventName:"PreToolUse",permissionDecision:"deny",permissionDecisionReason:$reason}}'
