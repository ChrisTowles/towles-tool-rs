---
paths:
  - "crates/**/*.rs"
  - "crates-cli/**/*.rs"
  - "crates-tauri/**/*.rs"
---

# Rust conventions

- **Errors:** `thiserror` enums in library crates (`crates/`); flatten to
  user-facing messages and exit codes only at the CLI boundary in `tt-cli`.
  No `unwrap()`/`expect()` on IO or user input in library code.
- **Tauri-free shared crates (hard rule):** nothing under `crates/` may depend
  on `tauri`. Tauri types stay in `crates-tauri/tt-app`.
- **Shared-file serde:** types serialized to files the TypeScript CLI also
  reads (settings, doctor history) must tolerate unknown fields — use
  `#[serde(default)]`, never `deny_unknown_fields` — and match the TS field
  names/casing exactly (camelCase where the TS record uses it).
- **Determinism for tests:** pass clocks (`now_ms`) and base paths in as
  parameters instead of reading `SystemTime`/`$HOME` deep in logic.
- **TTY guards:** every interactive prompt must fail with a clear error or
  no-op cleanly when stdin/stdout is not a TTY, so CI and tests never hang.
- **Tests:** unit tests in `#[cfg(test)] mod tests` alongside the logic;
  black-box CLI tests with `assert_cmd` under `crates-cli/tt-cli/tests/`.
- **Style:** rustfmt at 100 columns (`cargo fmt --check`);
  `cargo clippy --all -- -D warnings` must pass — warnings are errors.
- **Porting:** when deriving code from the TS CLI, cite the slot-1 source path
  (e.g. `src/commands/...`) in the commit message.
