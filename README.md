# Towles Tool (Rust)

A Rust rewrite of the [`towles-tool`](https://github.com/ChrisTowles/towles-tool)
CLI, paired with a [Tauri 2](https://v2.tauri.app/) desktop shell. The repository
is built from the [Yaak](https://github.com/mountain-loop/yaak) golden template —
a Cargo workspace with Tauri-free shared crates, a `clap` CLI, and a React + Vite
frontend.

During the migration the Rust binary is named **`ttr`**. Once it reaches feature
parity with the TypeScript CLI, it takes over the `tt` name in a hard cutover.

> **Status:** early. Milestone 0 (scaffold) is complete. Features are being
> ported from the TypeScript CLI one at a time — see
> [docs/MIGRATION.md](docs/MIGRATION.md).

## Quick start

**Prerequisites**

- Node.js 24+
- Rust (stable toolchain)
- Linux: `webkit2gtk` and the usual Tauri system dependencies
  (see the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/))

**Run the desktop shell**

```sh
npm install
npm run dev      # tauri dev — launches the app with the Vite frontend
```

**Run the CLI**

```sh
cargo run -p tt-cli -- doctor
```

## More

- [ATTRIBUTION.md](ATTRIBUTION.md) — derivation from Yaak and its MIT license
- [docs/MIGRATION.md](docs/MIGRATION.md) — the feature-port backlog
- [CLAUDE.md](CLAUDE.md) — project instructions and architecture

## License

MIT © 2026 Chris Towles. See [LICENSE](LICENSE).
