# Screenshotting and driving the Tauri app UI

How to capture and manipulate the running desktop app from an agent/terminal
session on this machine (Pop!_OS, COSMIC desktop, Wayland). Verified 2026-07-03.

## Taking screenshots

COSMIC on Wayland rules out the usual tools:

- `grim` fails — cosmic-comp doesn't implement `wlr-screencopy-unstable-v1`.
- The GNOME Shell D-Bus screenshot API isn't present (not GNOME).
- X11 tools (`import`, `scrot`, `xdotool`-based capture) only see XWayland
  windows; the Tauri/GTK app runs native Wayland.

What works is the portal-backed COSMIC tool:

```sh
cosmic-screenshot --interactive=false --notify=false --save-dir <dir>
# prints the saved file path; captures ALL monitors as one wide PNG
```

Then crop to the app window with ImageMagick (find the window region by
viewing the full capture once; the app window position is stable between
shots as long as it isn't moved):

```sh
convert full.png -crop <W>x<H>+<X>+<Y> +repage app.png
```

## Driving the UI (no input injection)

There is no working synthetic input on this setup — `xdotool` can't reach
native Wayland surfaces and nothing like `ydotool` is configured. Two
approaches that do work:

### 1. Vite HMR against the live Tauri window (preferred)

`npm run dev` keeps the Vite dev server attached to the real WebView, so any
source edit hot-reloads into the running app in ~1s. To put the UI in a
desired state, temporarily hard-code that state, screenshot, revert:

```tsx
<Dialog open>          {/* temporarily controlled-open */}
```

**Gotcha:** React Fast Refresh preserves component state, so *initial-state*
props like `defaultOpen` or changed `useState` initializers do nothing on
hot reload. Use a **controlled** prop (`open`, `value`, …), which takes
effect on re-render. Revert the edit when done — never commit it.

### 2. Bare browser via Chrome DevTools (for real interaction)

The same frontend runs at `http://localhost:1420` in a normal browser
(`npm run client:dev` if the Tauri app isn't already running the dev
server). There, browser-automation tooling (Chrome DevTools MCP) can click,
type, and screenshot the page normally. Caveat: the app renders the
"bare browser" code path (`__TAURI_INTERNALS__` is absent), so
Tauri-specific behavior (IPC commands, WebView quirks) is only observable in
the real shell via approach 1.

## Misc

- Port 1420 is `strictPort` — if `npm run dev` dies with "Port 1420 is
  already in use", another worktree slot's dev server has it:
  `lsof -i :1420` and kill the old `vite` process.
- If the tt-app build script fails reading plugin permissions from a *stale
  absolute path* (another checkout's `target/`), the cargo build cache was
  copied between worktrees: `rm -rf target/debug/build/tauri-* target/debug/build/tt-app-*`
  and rebuild.
