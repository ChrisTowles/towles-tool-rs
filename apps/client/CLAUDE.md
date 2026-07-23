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

## IPC failures are values — the call site picks the UX

`src/lib/tauri.ts` exports one `invoke`, returning
`Result<T, IpcError>` ([better-result](https://better-result.dev)). It
**never throws and never rejects**: no Tauri host, a rejected command, a Zod
schema mismatch, and a timeout all come back as typed `Err`s
(`src/lib/errors.ts` — `NotInTauri`, `IpcFailed`, `SchemaMismatch`,
`IpcTimeout`).

That is deliberate. There used to be four wrappers, each hardcoding one
failure UX, and picking the wrong one silently changed behavior — the two
that degraded to `null`/`false` made a real backend error indistinguishable
from "not wired in browser". Now each call site states its own intent:

```ts
const repos = (await invoke<Repo[]>("list_repos")).unwrapOr([]);      // degrade
if ((await taskDelete({ id })).isErr()) revertOptimisticDelete();      // branch
result.match({ ok: setView, err: (e) => {                             // report
  if (!NotInTauri.is(e)) toast.error(e.message);                      // …but not in browser dev
} });
```

Three rules that follow from this:

- **Browser dev is `NotInTauri`, not a failure.** Test for it with
  `NotInTauri.is(e)` — never `e._tag === "…"`, which oxlint rejects.
- **Fire-and-forget is safe by construction.** An ignored `Result` can't
  produce an unhandled rejection, so `void invoke(…)` needs no `.catch`.
  The hot PTY-write path in `components/terminal-view.tsx` relies on this.
  A `.catch` on an `invoke` is dead code.
- **Use `errorMessage(e)` (`src/lib/errors.ts`), not `String(e)`**, for
  display. Tauri rejects with a bare string, which `String()` renders as
  `"[object Object]"`.

Two boundaries deliberately keep a *throwing* contract because a foreign
interface demands it: `lib/monaco-fs.ts` (monaco's `IFileSystemProvider`
expects thrown `FileSystemProviderError`s) and `lib/lsp.ts` (vscode-jsonrpc
requires a rejecting `write`). Translate `Err` → throw at those edges only.

`.claude/hooks/guard-better-result.sh` flags drift back to the old shapes
on every edit.

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

## A pane has no PTY until it is rendered

`term_start` runs from `TerminalView`'s mount effect, and `screens/
agentboard.tsx` renders only the **active folder's active window** panes.
So a session can exist in the rail, and even be reported as agent-running
(the watcher reads Claude's on-disk state, not the PTY), while its pane has
never mounted and no shell exists.

Anything that writes to a PTY must therefore `selectSession(folderDir, id)`
first and then `await waitForFirstFrame(id)`. `termWriteRetry` alone is not
enough — it only covers the few hundred ms before `term_start` registers the
id, not the case where the pane was never mounted at all. **A write to an
unmounted pane resolves `Err`** (`term_write` is `Result<(), String>` in
Rust), and an unchecked one surfaces as an action that appeared to work and
did nothing — worse when an optimistic overlay then reports a state change
that never happened. Check it: `if ((await termWrite(id, data)).isErr())`.

This is why every lifecycle action in `SessionActions` takes `folderDir`,
including `stopClaude`/`compactClaude` — their triggers (rail kebab, cache
badge) render for *every* folder, not just the active one.

Restoring several sessions at once must additionally drain **serially** —
select a folder, await its first frame, write, then move to the next — because
only one folder is active at a time. See the open-session drain effect in
`screens/agentboard.tsx`; firing the requests concurrently leaves every folder
but the last with a placed-but-never-started pane.

## Clickable rows can't be `<button>`s

Radix's `Checkbox`, `Switch`, `RadioGroupItem` and `*Trigger` primitives render
real `<button>`s, and a `<button>` may not contain interactive descendants.
The established patterns here:

- **Checkbox row** → `<label htmlFor>` wrapping the `Checkbox`; the label makes
  the whole row a click target natively, with no extra handler
  (`components/resume-picker.tsx`).
- **Inline rename input** → swap the element rather than nesting one: render
  *either* the input *or* the chip button, never an input inside the button
  (the window tab strip in `screens/agentboard.tsx`).
- **Row with trailing actions** → keep the action buttons as *siblings* in a
  flex row, with only the identity cluster inside the button
  (`components/agentboard-rail.tsx`'s folder header).
- A `stopPropagation` on a child of a clickable parent is a smell that the
  nesting is wrong.

React reports these only at **runtime**, and nothing else in this repo can see
them: there is no linter, `tsc` doesn't model the DOM, and vitest runs in a
node environment with no renderer. `node scripts/drive.mjs console` is the
check — see below.

## Two animation idioms — the choice is mechanical, not stylistic

`tw-animate-css` (imported in `index.css`) is the default: the vendored
`components/ui/*` animate with `data-open:animate-in fade-in-0 …`, which works
because Radix keeps a closing element mounted until its animation ends.

Nothing else has that luxury. The agentboard rail renders a backend snapshot
(`agentboard://state`), so a removed repo/task/session is just absent from the
next payload and React unmounts the row before any CSS can run. That case uses
`motion`: `<AnimatePresence>` holds the departed row on screen, and `layout`
slides the survivors into the space it frees. Config lives in
`lib/rail-motion.ts` — spread it rather than hand-rolling per-row variants.

Deliberately **not** wrapped in yaak's `<LazyMotion strict>` + `m.*`: that
splits motion into its own chunk only when every `AnimatePresence` consumer is
lazily imported, and screens here are static imports (`screens/index.tsx`), so
a build puts motion in the initial chunk regardless. `main.tsx` keeps only
`<MotionConfig reducedMotion="user">`, which is real app-wide a11y policy.

## Testing convention: logic-only

Logic tests (`*.test.ts`) run in the fast Node env — the default. Components
are kept deliberately thin, so most branching logic lives in pure `lib/*.ts`
functions that are unit-testable without a DOM (e.g. `workspace-persistence.ts`
exists solely to make tab-restore logic testable). Prefer that seam when you
can.

Render-level tests (`*.test.tsx`) exist too, for the case a pure function
can't cover — that a screen mounts and renders its shell without throwing.
They opt into jsdom per-file with a `// @vitest-environment jsdom` docblock
(so the Node suite stays quick) and render through `src/test/render.tsx`'s
`renderWithProviders`, which wraps the component in App.tsx's provider tree.
**The backend seam is jsdom itself:** there is no `__TAURI_INTERNALS__`, so
every `invoke` returns `NotInTauri` and each component paints its colocated
browser-dev fallback — stub at that seam, never mock component internals.
`renderWithProviders` polyfills the browser APIs jsdom omits (`matchMedia`,
`ResizeObserver`, pointer capture). Keep these as smoke/regression guards
(does it render? are the tabs there?), not a substitute for driving the real
shell.

**A green test run still says little about whether the page rendered
*correctly*.** These smoke tests catch a throw on mount, but the authoritative
signal for a runtime React complaint (invalid DOM nesting, a bad hook order)
is the page's own console, which the app buffers under `VITE_WDIO`
(`lib/wdio-console.ts`). Every `scripts/drive.mjs` verb prints a `⚠ N console
error(s)` summary when the buffer is non-empty, and `drive.mjs console` dumps
it (exiting non-zero on real errors, so it can gate a script). Verify real
UI/IPC behavior by driving the shell (`npm run e2e` / `npm run dev:drive` —
see the root CLAUDE.md's Commands section); if you changed UI and never looked
at that output, the change is unverified — an invalid-DOM warning otherwise
reaches only the `dev:drive` terminal, a different process from `drive.mjs`.
