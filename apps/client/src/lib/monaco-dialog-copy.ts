/**
 * Text adjustments for the confirmations the VS Code layer raises. Kept apart
 * from `monaco-dialogs.ts` because that module pulls in the whole
 * `@codingame/monaco-vscode-api` graph (CSS and all), which a logic-only
 * vitest run can't load — this file stays importable by tests.
 */

/** VS Code writes mnemonics as `&&Delete`; show a plain label. */
export function stripMnemonic(label: string): string {
  return label.replace(/&&/g, "");
}

/**
 * Make the delete confirmation tell the truth.
 *
 * The file service thinks trashing is unsupported — `OverlayFileSystemProvider`
 * drops the `Trash` capability our provider advertises — so VS Code asks to
 * "permanently delete" and warns the action is irreversible. Our provider
 * trashes anyway (untracked files are unrecoverable otherwise), which would
 * make that wording a lie. Rewrite it to match what actually happens.
 *
 * Narrow on purpose: an unrelated confirmation passes through untouched.
 */
export function deleteCopyForTrash(
  message: string,
  detail: string | undefined,
): { message: string; detail?: string } {
  if (!/permanently delete/i.test(message)) return { message, detail };
  return {
    message: message.replace(/permanently delete/gi, "delete"),
    detail: "You can restore it from your system Trash.",
  };
}

/** Destructive actions get the destructive button. VS Code doesn't tag
 * confirmations with a severity we can trust here, so key off the verb the
 * action put on its own primary button. */
export function isDangerous(primary: string, message: string): boolean {
  return /delete|remove|discard|trash/i.test(`${primary} ${message}`);
}
