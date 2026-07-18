/**
 * Commands the VS Code layer registers that this app can't honor — shadowed
 * out at startup.
 *
 * `@codingame/monaco-vscode-api`'s entry point is `services.js`, which
 * side-effect-imports the workbench's format and file-action contributions
 * whether we want them or not. Two of those reach `IDialogService.confirm`,
 * and the standalone dialog service implements confirm as a literal
 * `window.confirm()` — a blocking native script dialog. Inside the Tauri
 * WebView that spins a nested GTK main loop while the app dispatches sync IPC
 * on the same thread, and the window wedges.
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
  // contrib/files/browser/fileActions.contribution.js — confirms through
  // window.confirm, then writes through a provider that is read-only by
  // design (edits belong on CodeViewer's mtime-guarded ide_write_file path).
  // Delete is the gap: unlike New File/Rename/Cut it carries no writable
  // precondition, and with no trash capability plain Delete maps to it too.
  "deleteFile",
  "moveFileToTrash",
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
