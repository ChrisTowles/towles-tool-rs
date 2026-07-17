# CLAUDE.md — apps/client

React 19 + Vite frontend — see the root [`CLAUDE.md`](../../CLAUDE.md) for
the shell overview (sidebar nav, command palette, Focus screens, product
rules). This file covers the frontend-internal conventions that a single
read of the code won't surface.

## Three unrelated things are called "tab" in this repo

- **Workspace tabs** — the open-screens bookkeeping in `useWorkspace()`
  (`openTabs`/`activeTab`/`openTab`/`closeTab`, `src/lib/workspace.tsx`,
  persisted via `src/lib/workspace-persistence.ts`). There's no visible tab
  strip — the sidebar is the only nav UI — but screens still stay mounted in
  the background when you switch away (e.g. an Agentboard terminal keeps
  running), and `close-tab`/`next-tab`/`prev-tab`/`tab-1`…`9`
  (`src/lib/shortcuts.tsx`) still operate on this set headlessly. This is
  what "tab" means in most of this codebase's docs/comments.
- **Settings' sub-tab panel** — the General/Appearance/Agentboard/etc. panes
  inside the Settings screen, built on the vendored shadcn/Radix `Tabs`
  primitive (`src/components/ui/tabs.tsx`), consumed only by
  `src/screens/settings.tsx`. Unrelated to the tab bar above — it's a
  generic tabbed-panel widget, not app-level screen navigation.
- **IDE editor/diff tabs** — `crates/tt-ide` and `crates-tauri/tt-app/src/
  ide.rs`'s `tabs`/`close_tab`/`closeAllDiffTabs`, part of the Claude Code
  IDE-protocol integration (see
  [docs/CLAUDE-CODE-IDE.md](../../docs/CLAUDE-CODE-IDE.md)). A VS
  Code-style concept with no shared code path with either of the above.

## Adding a screen is a 4-file ritual — there's no single source of truth

1. Register the `ScreenId` + `ScreenMeta` (icon/keywords/`fullBleed`) in
   `src/lib/screens.ts`.
2. Wire the component into `SCREEN_COMPONENTS` in `src/screens/index.tsx`.
3. Add it to a `NAV_SECTIONS` group in `src/lib/screens.ts` — miss this and
   the screen is only reachable via the command palette / tab restore, not
   the sidebar.
4. If it needs shortcuts, extend `SHORTCUTS` in `src/lib/shortcuts.tsx`.

`fullBleed` is load-bearing, not cosmetic: `App.tsx` branches per-screen on
`SCREENS[id].fullBleed` to skip the centered `max-w-3xl` `ScrollArea`
wrapper. A new full-screen/canvas screen that forgets this flag gets
squeezed into the narrow content column.

Screens stay mounted forever once visited — `App.tsx` toggles `hidden`
rather than unmounting, so a screen's local state (e.g. terminal buffers)
survives tab switches. `closeTab` (`src/lib/workspace.tsx`) is the only
unmount path, and it refuses to close the last tab.

## `invoke*` wrappers have different failure semantics — pick deliberately

`src/lib/tauri.ts` exports four, and using the wrong one silently changes
UX:

- `invokeCmd` — degrades to `null` (optional Zod validation).
- `invokeOrThrow` — propagates the error (optional timeout).
- `invokeToast` — degrades to `null` + shows an error toast.
- `invokeOk` — returns `boolean`, toasts on both browser-dev *and* failure.

Using `invokeOk` for a read, for example, hides a real error as "not wired
in browser."

## Mock-data fallback is colocated per-module, not a single file

There is no `mock-data.ts`. Each module owns its own fallback (e.g.
`mockSnapshot` in `src/lib/data.ts`, `mockView` in `src/lib/slack.ts`),
gated on `!isTauri()` (`src/lib/tauri.ts`) so plain-Vite browser dev still
renders something. Add new fallbacks the same way — colocated, not in a
shared mock file.

## Shortcuts registry validates at build time

`defineShortcuts`/`parseKeys` (`src/lib/shortcuts.tsx`) throw at
module-eval time on a bad spec or duplicate id — a typo'd shortcut fails
the build, not silently mismatches at runtime.

`allowInEditable` is a two-sided contract: it only works if the owning
component *also* checks `matchesEditableOverride` to yield the keystroke
instead of consuming it (see `components/terminal-view.tsx`). The whole
opt-out is further gated behind the `agentboard.shortcutsWorkInTerminal`
setting via `useShortcutsWorkInTerminal`, which refreshes on window focus and
on the `tt:settings-saved` event fired right after a successful Settings save
(`SETTINGS_SAVED_EVENT` in `lib/settings.ts`) — a save on the Settings tab
propagates immediately, no relaunch or app-level refocus needed.

## Terminal rendering is a custom protocol, not xterm.js

`src/lib/term-protocol.ts` defines the `terminal://frame` wire shape
(dirty-row `RowUpdate`/`Run` diffs, packed `0xRRGGBB` colors, style bit
flags) mirroring the Rust `tt-vt` crate, plus a hand-rolled DOM-key→escape
encoder (`encodeKey`) and grapheme-cluster-aware wide-char handling
(`isWideRun`). A new terminal feature must be threaded through both the
Rust frame struct (`crates/tt-vt`) and this file's types in lockstep — see
[`crates/tt-vt/CLAUDE.md`](../../crates/tt-vt/CLAUDE.md) for the Rust side.

## Testing convention: logic-only

Vitest tests live under `lib/` (`*.test.ts`, `npm run test` = `vitest run`)
— there are no `*.test.tsx` component tests by design. Components are kept
deliberately thin; push branching logic into pure `lib/*.ts` functions
specifically so it's unit-testable without a DOM (e.g.
`workspace-persistence.ts` exists solely to make tab-restore logic
testable). Verify actual UI/IPC behavior by driving the real shell
(`npm run e2e` / `npm run dev:drive` — see the root CLAUDE.md's Commands
section), not by adding component tests.
