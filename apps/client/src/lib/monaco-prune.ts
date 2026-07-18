/**
 * Formatting commands the VS Code layer registers that this app can't
 * honor — shadowed out at startup.
 *
 * `@codingame/monaco-vscode-api`'s entry point is `services.js`, which
 * side-effect-imports the workbench's format contributions whether we want
 * them or not. No formatter is registered for any language here (the
 * standalone language features were removed — see `lib/monaco.ts`), so every
 * one of these funnels into the "install a formatter?" prompt. Formatting is
 * uniformly absent rather than inconsistently broken, and these ids are the
 * loose ends of that decision.
 *
 * Shadowing works because `CommandsRegistry.registerCommand` unshifts onto a
 * per-id list and `getCommand` returns the first entry, so a later
 * registration of the same id wins. Keybindings and the command palette both
 * dispatch by id through `ICommandService`, so one no-op registration closes
 * every route to the original handler.
 */

export const PRUNED_COMMANDS: readonly string[] = [
  // contrib/format/browser/formatActionsNone.js — prompts "install a
  // formatter?" through window.confirm. Bound to Shift+Alt+F, and on Linux
  // also to Ctrl+Shift+I (which is the devtools chord, so it gets hit by
  // accident).
  "editor.action.formatDocument.none",
  // contrib/format/browser/formatActions.js — no formatter is registered for
  // any language now that the standalone language features are gone, so these
  // only ever reach the same "none" prompt.
  "editor.action.formatDocument",
  "editor.action.formatSelection",
  "editor.action.formatDocument.multiple",
  "editor.action.formatSelection.multiple",
  "editor.action.formatChanges",
  // NOT listed here: deleteFile / moveFileToTrash. Those used to be shadowed
  // because the file provider was read-only and their confirm() wedged the
  // window. Both causes are gone — the provider writes through ide_* commands
  // and IDialogService renders a real in-app dialog — so the Explorer's
  // delete works for real now.
];

/**
 * Ids that are no longer registered upstream. A shadow over a renamed command
 * is a silent no-op: the real handler stays live and the hazard comes back
 * with nothing to notice it, so the caller reports these loudly.
 */
export function staleCommands(known: Iterable<string>): string[] {
  const live = new Set(known);
  return PRUNED_COMMANDS.filter((id) => !live.has(id));
}
