# Driving & testing the real Tauri shell

Two ways to drive the **actual Tauri shell** — the WebKitGTK WebView, not a bare
browser — both exercising real Rust IPC (`app_slot`, `settings_get`,
`ab_discover_repos`, …):

- **Live drive** (`npm run dev:drive` + `scripts/drive.mjs`) — one window you keep
  open that Claude drives *live* while you watch: screenshots, clicks, real IPC.
  Interactive development/debugging. See **[Live drive](#live-drive)** below.
- **Regression suite** (`npm run e2e`) — WebdriverIO specs that spawn a fresh
  window, run, and exit. CI-style pass/fail. See **[Regression suite](#regression-suite-webdriverio)** below.

Both are the automated counterpart to the manual runbook in
[../docs/UI-SCREENSHOTS.md](../docs/UI-SCREENSHOTS.md).

## Live drive

Open a persistent, automatable window and leave it up:

```sh
npm run dev:drive     # like `npm run dev` (HMR) but the window is automatable
```

Then drive that same window from another shell — this is how Claude debugs a UI
or IPC change against the app you're watching:

```sh
node scripts/drive.mjs status                     # is the automation server up?
node scripts/drive.mjs invoke settings_get        # call a real Rust IPC command
node scripts/drive.mjs invoke journal_log '{"text":"hi"}'
node scripts/drive.mjs eval "document.title"      # run JS in the live window
node scripts/drive.mjs shot cockpit               # → e2e/screenshots/cockpit.png
node scripts/drive.mjs click "nav button"         # click an element (CSS selector)
node scripts/drive.mjs clicktext "Board"          # click a button/link by visible text
node scripts/drive.mjs type "input" "board"       # type into an element
node scripts/drive.mjs url /                       # navigate the window
```

How it works: the app built with the `wdio` cargo feature runs an **in-process
W3C WebDriver server** (`tauri-plugin-wdio-webdriver`) on `wdPort` for the whole
lifetime of the window — no WebdriverIO involved. `drive.mjs` is a plain-`fetch`
client (no `@wdio/*` at runtime): session-less `POST /wdio/eval` for `status` /
`eval` / `invoke`, and a short-lived W3C session (created then deleted) for
`shot` / `click` / `type` / `url`. IPC goes through
`window.__TAURI_INTERNALS__.invoke`, so it doesn't depend on the frontend plugin.

Notes:
- **Ports come from `.env.local`** (same as `npm run dev`): `wdPort = TT_DEV_PORT
  + 3000` (override `TT_E2E_WEBDRIVER_PORT`). Deterministic per slot so
  `drive.mjs` finds the server without arguments; different slots don't collide.
- **`click`/`type` take CSS selectors only** (W3C `POST /element`), which can't
  match by text. To click a text-only button/link, use `clicktext "<text>"`
  instead — it matches trimmed visible text across clickable elements (buttons,
  links, `[role=button]`, …) and dispatches a real click. Multiple matches or no
  match fail with the list of clickable texts found, so you can pick a better
  string. (For a non-clickable element you still need the tag-then-select trick:
  `drive.mjs eval "document.querySelectorAll('div')[i].setAttribute('data-drive','x')"`
  then `drive.mjs click "[data-drive=x]"`.)
- `dev:drive` and `npm run e2e` **share a slot's ports**, so don't run both in the
  same slot at once.
- Automation mode: launched with `TAURI_WEBVIEW_AUTOMATION=true`, WebKitGTK may
  show a small "controlled by automation" banner and use ephemeral web storage.

## Regression suite (WebdriverIO)

Committed specs that spawn a fresh window, drive it, and exit — CI-style pass/fail.

Stack: [`@wdio/tauri-service`](https://github.com/webdriverio/desktop-mobile/tree/main/packages/tauri-service)
with its **embedded** WebDriver provider (`tauri-plugin-wdio-webdriver` runs a
W3C WebDriver server inside the app; `tauri-plugin-wdio` adds
`browser.tauri.execute()` / `.mock()`). Both are gated behind the Rust `wdio`
cargo feature and the `VITE_WDIO` frontend flag, so **nothing ships in normal or
release builds**.

## One-time setup (Linux)

The embedded provider removes the external `tauri-driver` process, but on Linux
the system **WebKitWebDriver** binary is still required to automate the
WebKitGTK WebView (macOS needs nothing). Install it once:

```sh
sudo apt-get install -y webkit2gtk-driver     # Debian/Ubuntu/Pop!_OS
# Fedora 40+: sudo dnf install -y webkit2gtk-driver
# Arch:       sudo pacman -S webkit2gtk-4.1
```

The JS dev-deps (`@wdio/*`, `tsx`) install with `npm install` at the repo root.

## Running

```sh
npm run e2e        # build the app (--features wdio) + serve + run the suite
npm run e2e:run    # run against an already-built binary (skips the build)
```

`scripts/e2e.mjs` orchestrates everything. **Ports come from `.env.local`** (the
same `TT_DEV_PORT` mechanism as `npm run dev`): the Vite dev server uses
`TT_DEV_PORT`, and the embedded WebDriver server uses `TT_DEV_PORT + 3000`
(override with `TT_E2E_WEBDRIVER_PORT`). Nothing is hardcoded, so slots don't
collide. `e2e:run` assumes the binary was already built for this slot's port —
use `e2e` after changing the port or the Rust commands.

## How it works (the non-obvious bits)

Driving the real WebKitGTK shell on Linux took three things that aren't in the
quick-start:

1. **`TAURI_WEBVIEW_AUTOMATION=true`** — set by `e2e.mjs` when launching the app.
   Without it wry never enables WebKit automation and WebKitWebDriver attaches to
   a blank `about:blank` context instead of the app's WebView.
2. **`devUrl` baked to the dev port** — `e2e.mjs` builds with
   `TAURI_CONFIG={build:{devUrl:"http://localhost:<TT_DEV_PORT>"}}`. The WebView
   must be served from a **trusted Tauri origin** or IPC invokes are rejected
   ("not allowed"). So the app loads the live dev server, which serves the
   `VITE_WDIO`-enabled frontend (loads `@wdio/tauri-plugin`).
3. **Eval port alignment** — `browser.tauri.execute()` reads the plugin eval
   port from `TAURI_WEBDRIVER_PORT`; `e2e.mjs` exports it to match the embedded
   server port.

`e2e/wdio.conf.ts` then navigates the WebView to the dev server in a `before`
hook and waits for `#root`.

## Layout

- `e2e/wdio.conf.ts` — service config; reads ports from env, debug binary at
  `target/debug/tt-app`.
- `e2e/specs/*.e2e.ts` — Mocha specs. `browser.tauri.execute(({core}) =>
  core.invoke('cmd'))` calls real commands; `browser.tauri.mock('cmd')` stubs
  them. Specs here are **read-only** — they never write your real settings file.
- Rust: `crates-tauri/tt-app/Cargo.toml` `[features] wdio`, registered in
  `src/lib.rs` under `#[cfg(feature = "wdio")]`, with the `wdio` capability
  added at runtime from `wdio-capability.json` (kept out of `capabilities/` so
  normal builds never reference the plugins' ACL).
- Frontend: `src/main.tsx` imports `@wdio/tauri-plugin` only when
  `import.meta.env.VITE_WDIO` is set (tree-shaken out otherwise).

## Notes

- The `@wdio/native-utils` override in the root `package.json` works around a
  broken version pin in `@wdio/tauri-service@1.2.0`; remove it once upstream
  ships a fix.
- CI would need `webkit2gtk-driver` plus a virtual display (`xvfb-run`).
