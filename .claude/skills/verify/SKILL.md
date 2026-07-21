---
name: verify
description: Project recipe for driving the real Tauri shell (apps/client) to verify a UI/IPC change end-to-end, rather than only running tests/typecheck. Use as the handle for the generic verify skill in this repo.
---

# Verifying a UI/IPC change in this repo

This is a Tauri 2 desktop app (`crates-tauri/tt-app` + `apps/client`). The
real surface is the WebKitGTK WebView driven over real Rust IPC — never the
mock-data browser dev server, never just `vitest`/`tsc`.

## Get a handle

```sh
npm run dev:drive > /tmp/.../dev-drive.log 2>&1 &   # background; builds + launches the real app
disown
```

The Rust build (cargo) + Vite dev server take **1–3 minutes** cold (less if
`target/` is warm). Poll readiness instead of a fixed sleep:

```sh
until node scripts/drive.mjs status; do sleep 3; done   # blocks until the automation server answers
```

`node scripts/drive.mjs status` → `{"ready": true}` once up. Ports are
per-task/deterministic (`TT_DEV_PORT`-derived); `drive.mjs` finds them with no
args, so nothing needs manual configuration.

## Drive it

```sh
node scripts/drive.mjs invoke ab_get_state      # real Agentboard state (repos/folders/sessions)
node scripts/drive.mjs invoke store_snapshot    # real store snapshot (prs/issues/tasks/events)
node scripts/drive.mjs shot <name>              # → e2e/screenshots/<name>.png (gitignored) — Read it
node scripts/drive.mjs eval '<js expression>'   # runs in the live window, must be one expression
node scripts/drive.mjs clicktext "Board"        # click by visible text
```

**Gotcha: the left nav rail icons carry no visible text**, only
`aria-label` — `clicktext` can't find them. Click by `eval` instead:

```sh
node scripts/drive.mjs eval '(() => { document.querySelector(`[aria-label="Agentboard"]`).click(); return "clicked"; })()'
```

**Gotcha: `eval`'s arg is wrapped as `await (<expr>)`** — pass an expression
(an IIFE), not a statement list with `;` at the top level.

## Synthetic state, when real state won't cooperate

The frontend hydrates from two live event streams: `agentboard://state`
(`useAgentboardState`, `apps/client/src/lib/agentboard.ts`) and
`store://snapshot` (`useStoreSnapshot`, `apps/client/src/lib/data.ts`). Both
can be driven directly from the page's own JS context — no Rust command
needed — via the same IPC call `@tauri-apps/api/event`'s `emit()` uses under
the hood:

```js
await window.__TAURI_INTERNALS__.invoke('plugin:event|emit', {
  event: 'agentboard://state', // or 'store://snapshot'
  payload: { ...real payload from ab_get_state, with your fixture mutated in... },
});
```

Take the real payload (`invoke ab_get_state`/`invoke store_snapshot`), mutate
just the field(s) under test, bump `StatePayload.ts` (any event with a lower
`ts` than the current one is dropped), and emit. **This loses the race to the
real backend's own ~2s poll** if you wait even a second before screenshotting
— which is a feature, not a bug: real ground truth always wins, so a
disagreement between your fixture and what renders means the real watcher
already overwrote it with (possibly more interesting) real data. Screenshot
immediately after the `eval` call.

**Do the emit + navigate + click in one `eval` call, not several.** Splitting
across multiple `eval`/`shot` round-trips (even ~1s apart) gives the real
watcher time to win the race and silently overwrite your fixture before the
screenshot — you'll see a *partial* result (e.g. a fabricated PR persists
because PRs poll slowly, but `dirty`/diff stats already got refreshed back to
real values because git-stats polls fast). One combined async IIFE that
emits, clicks the nav item, and clicks the target row, immediately followed
by `shot`, is reliable; anything split across calls isn't.

Also useful: if the real machine has *another* Towles Tool instance running
(the user's daily driver), you'll see it in the log —
`slack socket: another instance already holds the singleton lock, parking` —
and should expect the tracked-repos/PR state to keep moving under you between
polls, independent of anything you did.

## Don't click destructive-looking buttons

The auto-mode permission classifier blocks clicks on buttons whose visible
text reads as destructive (e.g. "delete"), even when the actual handler only
opens a confirmation `AlertDialog` (as `requestDeleteWorktree` →
`performDeleteWorktree` does in `screens/agentboard.tsx`). Don't try to work
around it — verify that code path by reading the guard instead of clicking
through it live.

## Shut down

`dev:drive` spawns `tauri` as its own process-group leader; a background `&`
launch has no wrapper to relay a signal, so kill the whole group:

```sh
ps aux | grep -E "dev-drive|tauri dev|target/debug/tt-app|vite" | grep -v grep
kill -- -$(ps -o pgid= -p <any pid in the tree> | tr -d ' ')
```

Confirm with `node scripts/drive.mjs status` (should fail to connect) rather
than trusting `ps` alone — `vite`/esbuild can survive as an orphan.
