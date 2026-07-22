/**
 * Browser stand-in for `node:fs/promises`, wired up by the alias in
 * `vite.config.ts`.
 *
 * `@vscode/diff` (a transitive dep of `@codingame/monaco-vscode-api`) reads its
 * `.wasm` off disk in `initWasm()` when it detects it's running under Node —
 * `if (process.versions?.node) { const { readFile } = await import('node:fs/
 * promises'); … }`. That branch is dead in a WebView, but the bundler still
 * sees the import and, without an alias, externalizes it with a warning on
 * every chunk that pulls the module in.
 *
 * Aliasing it here is not just warning suppression: a Node builtin has no
 * meaning in this bundle, so an externalized stub would fail opaquely at
 * runtime if anything ever *did* reach it. This throws with a name instead.
 */

export function readFile(): never {
  throw new Error("node:fs/promises.readFile is not available in the webview");
}

export default { readFile };
