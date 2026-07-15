# CLAUDE.md — crates/tt-vt

Terminal-state engine on top of **libghostty-vt**: feeds raw PTY bytes in,
emits dirty-row `Frame`s out (style runs, cursor, title, scrollback). Does
no rendering and no PTY handling itself — both are deliberately excluded
from Ghostty's library, and live in `crates-tauri/tt-app/src/terminal.rs`
(PTY host) and `apps/client/src/lib/term-protocol.ts` (rendering) instead.
See [`apps/client/CLAUDE.md`](../../apps/client/CLAUDE.md) for the frontend
half of this wire protocol.

## Build requirement

Needs **zig 0.15.x** on `PATH` to compile `libghostty-vt-sys` (dotfiles
`functions/18-zig.sh` sets this up). Without it, or with a mismatched zig
version, the build fails at the `libghostty-vt-sys` crate, not here.

## The Debug-mode parser trap

A **Debug-mode** libghostty-vt parser is ~1000x slower than release — it
saturates a CPU core at only ~130 KB/s of PTY output, so any moderately
busy terminal (a build log, `git log -p`, anything scrolling fast) reads as
laggy with no obvious cause. `cargo run`/`cargo test` default to Zig Debug
for dev builds unless overridden.

This is why the workspace `Cargo.toml` has a
`[profile.dev.package.libghostty-vt-sys]` override forcing zig
`ReleaseFast` even in `cargo build` (non-release) profiles. If this
override regresses (e.g. a Cargo.toml merge drops it), terminals will feel
broken for a totally non-obvious reason. `tt_vt::parser_optimize_mode()`
reports the actual compiled mode at runtime — `tt-app`'s `lib.rs` checks it
on startup and prints a loud warning if it's `"Debug"`, and the Doctor
screen (`tt-doctor`) runs the same check. If you're debugging "why do
terminals feel slow," check this before anything else.

## Threading and flow control

`Session::spawn` runs one dedicated engine thread per terminal — PTY bytes
never block the Tauri async runtime or the GTK main thread. Synchronized
output (DEC mode 2026, used by TUIs to batch redraws) is honored with a
**bounded** hold: a stuck or misbehaving TUI that never closes the
synchronized-output block can't freeze a pane forever, since the hold caps
out and flushes anyway.
