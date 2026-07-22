/**
 * Browser stand-in for `node:fs/promises`, wired up by the `vscodeDiffNodeShim`
 * plugin in `vite.config.ts` — see there for why it exists and why it is scoped
 * to `@vscode/diff` alone.
 *
 * A Node builtin has no meaning in this bundle, so the alternative (Vite's own
 * externalized stub) would fail opaquely if anything ever did reach it. This
 * throws with a name instead.
 */

export function readFile(): never {
  throw new Error("node:fs/promises.readFile is not available in the webview");
}
