# E2E UI tests (WebdriverIO + Tauri)

Real end-to-end tests that drive the **actual Tauri shell** — the WebKitGTK
WebView, not a bare browser — so they exercise real Rust IPC commands
(`app_slot`, `settings_get`, `ab_discover_repos`, …) and can mock IPC. This is
the automated counterpart to the manual runbook in
[../docs/UI-SCREENSHOTS.md](../docs/UI-SCREENSHOTS.md).

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
