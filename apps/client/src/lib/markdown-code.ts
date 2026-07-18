/**
 * Fence language → Monaco language id, for highlighting code blocks in the
 * Markdown preview.
 *
 * Markdown fences use short aliases (```ts) that are not VS Code language ids
 * (`typescript`), and Monaco silently renders an unknown id as plaintext — so
 * a wrong mapping looks like "highlighting is broken" with no error anywhere.
 * The aliases below were checked against the grammars this app actually loads
 * (`lib/monaco.ts`): ids not listed here pass through unchanged because they
 * are already correct (json, css, html, yaml, diff, log, rust, python,
 * typescript, javascript, shellscript).
 */

const ALIASES: Readonly<Record<string, string>> = {
  ts: "typescript",
  tsx: "typescript",
  mts: "typescript",
  cts: "typescript",
  js: "javascript",
  jsx: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  rs: "rust",
  py: "python",
  // `shell` and `bash` are aliases, not ids — the grammar registers as
  // `shellscript`, and the other two render as plaintext.
  sh: "shellscript",
  bash: "shellscript",
  zsh: "shellscript",
  shell: "shellscript",
  console: "shellscript",
  yml: "yaml",
  md: "markdown",
};

/**
 * The Monaco language id for a fence, or null when there is nothing to
 * highlight with. `className` is what react-markdown puts on `<code>`
 * (`language-ts`); a fence with no language has none.
 */
export function monacoLanguageFor(className: string | undefined): string | null {
  const match = /(?:^|\s)language-([\w+-]+)/.exec(className ?? "");
  if (!match) return null;
  const lang = match[1].toLowerCase();
  return ALIASES[lang] ?? lang;
}
