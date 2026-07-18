/**
 * Lazy Monaco loader, backed by @codingame/monaco-vscode-api: `monaco-editor`
 * is aliased to `@codingame/monaco-vscode-editor-api` (same API, real VS Code
 * services underneath), with the TextMate + theme service overrides so files
 * highlight with VS Code's actual grammars and Dark Modern theme. Everything
 * is bundled locally by Vite (`?worker` imports become self-contained worker
 * chunks) — no CDN, works offline inside the Tauri shell. The editor chunk is
 * only fetched when a code viewer actually mounts.
 *
 * The editor renders one theme (Default Dark Modern) regardless of the app's
 * light/dark mode.
 *
 * Two guards keep the workbench half of this stack from taking the window
 * down: `PRUNED_COMMANDS` is shadowed with no-ops below (the commands that
 * would open a native dialog or write through the read-only file provider),
 * and `lib/monaco-dialogs` replaces the standalone dialog service, whose
 * `confirm` is a literal blocking `window.confirm()`. See `lib/monaco-prune.ts`.
 */

import { PRUNED_COMMANDS, staleCommands } from "@/lib/monaco-prune";

let loading: Promise<typeof import("monaco-editor")> | null = null;

/**
 * The editor API only if some other consumer has already booted it — for
 * callers that want to decorate with Monaco but must not pay for its
 * multi-megabyte bootstrap on their own account (the Markdown preview's
 * syntax highlighting). Null until then, and never triggers a load.
 */
export function loadedMonaco(): Promise<typeof import("monaco-editor")> | null {
  return loading;
}

export function loadMonaco(): Promise<typeof import("monaco-editor")> {
  // The catch clears the cache: without it one failed bootstrap poisons
  // every editor, diff and quick-open for the life of the window.
  loading ??= start().catch((e: unknown) => {
    loading = null;
    throw e;
  });
  return loading;
}

async function start(): Promise<typeof import("monaco-editor")> {
  const [
    monaco,
    api,
    ,
    configuration,
    languages,
    textmate,
    theme,
    model,
    quickaccess,
    views,
    explorer,
    search,
    tauriFs,
    dialogs,
    editorWorker,
    textmateWorker,
  ] = await Promise.all([
    import("monaco-editor"),
    import("@codingame/monaco-vscode-api"),
    // Local extension host — the LSP bridge's monaco-languageclient runs
    // as a local extension against the vscode API (must load before
    // initialize).
    import("vscode/localExtensionHost"),
    import("@codingame/monaco-vscode-configuration-service-override"),
    import("@codingame/monaco-vscode-languages-service-override"),
    import("@codingame/monaco-vscode-textmate-service-override"),
    import("@codingame/monaco-vscode-theme-service-override"),
    import("@codingame/monaco-vscode-model-service-override"),
    import("@codingame/monaco-vscode-quickaccess-service-override"),
    import("@codingame/monaco-vscode-views-service-override"),
    import("@codingame/monaco-vscode-explorer-service-override"),
    import("@codingame/monaco-vscode-search-service-override"),
    import("@/lib/monaco-fs"),
    import("@/lib/monaco-dialogs"),
    import("monaco-editor/esm/vs/editor/editor.worker?worker"),
    import("@codingame/monaco-vscode-textmate-service-override/worker?worker"),
    // Importing a default-extension package registers its TextMate grammars
    // (or themes) as a built-in VS Code extension — side-effect imports.
    // (themeDefaults is awaited below: setTheme races its registration.)
  ]);
  const [themeDefaults, setiIcons] = await Promise.all([
    import("@codingame/monaco-vscode-theme-defaults-default-extension"),
    // VS Code's own file-icon theme — the Explorer's per-filetype icons come
    // from here rather than a hand-rolled extension→glyph map.
    import("@codingame/monaco-vscode-theme-seti-default-extension"),
    import("@codingame/monaco-vscode-rust-default-extension"),
    import("@codingame/monaco-vscode-typescript-basics-default-extension"),
    import("@codingame/monaco-vscode-javascript-default-extension"),
    import("@codingame/monaco-vscode-json-default-extension"),
    import("@codingame/monaco-vscode-css-default-extension"),
    import("@codingame/monaco-vscode-html-default-extension"),
    import("@codingame/monaco-vscode-markdown-basics-default-extension"),
    import("@codingame/monaco-vscode-yaml-default-extension"),
    import("@codingame/monaco-vscode-shellscript-default-extension"),
    import("@codingame/monaco-vscode-python-default-extension"),
    import("@codingame/monaco-vscode-log-default-extension"),
    import("@codingame/monaco-vscode-diff-default-extension"),
    // No standalone language features: this pane is a file *browser*, and
    // their TS worker has no tsconfig, no node_modules resolution and no
    // project graph, so every real source file lit up with bogus "cannot
    // find module" errors. They were also the only formatting providers in
    // the app, which is what made Format Document work for a handful of
    // languages and prompt-then-hang for the rest.
  ]);
  self.MonacoEnvironment = {
    getWorker(_workerId: string, label: string): Worker {
      // Highlighting runs in the TextMate worker; everything else (diff
      // computation, model ops) is the plain editor worker.
      return label === "TextMateWorker" ? new textmateWorker.default() : new editorWorker.default();
    },
  };
  // Through user config, seeded before the services start — `setTheme`
  // races the theme service's own async startup restore and loses.
  await configuration.initUserConfiguration(
    JSON.stringify({
      "workbench.colorTheme": "Default Dark Modern",
      "workbench.iconTheme": "vs-seti",
      "editor.stickyScroll.enabled": true,
      "editor.bracketPairColorization.enabled": true,
      "editor.guides.bracketPairs": "active",
      // Quick-open walks the workspace through the Tauri fs bridge — keep
      // it out of the build/dependency trees.
      "search.exclude": {
        "**/node_modules": true,
        "**/target": true,
        "**/dist": true,
        "**/.git": true,
      },
      // Keep the Explorer focused on source — build trees stay reachable
      // via a terminal, not the tree.
      "files.exclude": {
        "**/.git": true,
        "**/node_modules": true,
        "**/target": true,
      },
    }),
  );
  await api.initialize({
    ...configuration.default(),
    ...languages.default(),
    ...textmate.default(),
    ...theme.default(),
    // Resolves file: URIs into models through the file service (the Tauri
    // fs bridge) — quick-open's Enter path needs this.
    ...model.default(),
    ...quickaccess.default({
      // The app has no VS Code keybindings UI; always use the real picker.
      isKeybindingConfigurationVisible: () => false,
      shouldUseGlobalPicker: () => true,
    }),
    // Workbench views (the Files pane hosts the real Explorer via
    // attachExplorer). Spread after quickaccess so the workbench's own
    // quick-input wiring wins where they overlap. No editor part is ever
    // attached — the fallback routes Explorer opens to the app's viewer.
    ...views.default(async (modelRef) => {
      const uri = modelRef.object.textEditorModel.uri;
      modelRef.dispose();
      if (uri.scheme === "file" && openHandler != null) openHandler(uri.path);
      return undefined;
    }),
    ...explorer.default(),
    ...search.default(),
    // Last: nothing above may reinstate the standalone dialog service,
    // whose confirm() is a blocking native window.confirm().
    ...dialogs.default(),
  });
  tauriFs.registerTauriFileSystem();
  // Checked before shadowing — afterwards every id exists by construction,
  // so a rename would look healthy while the real handler stayed live.
  const { CommandsRegistry } =
    await import("@codingame/monaco-vscode-api/vscode/vs/platform/commands/common/commands");
  const stale = staleCommands(CommandsRegistry.getCommands().keys());
  if (stale.length > 0) {
    console.error(
      `[monaco] shadowed commands are gone upstream (renamed?), so they are live again: ${stale.join(", ")}`,
    );
  }
  // After initialize, so these land on top of the workbench contributions
  // they shadow (CommandsRegistry keeps the newest handler for an id).
  for (const id of PRUNED_COMMANDS) monaco.editor.registerCommand(id, () => {});
  // Both themes register asynchronously and the configured ids above race
  // that registration — await them or the editor falls back to the default
  // theme and the Explorer to no icons.
  await Promise.all([themeDefaults.whenReady(), setiIcons.whenReady()]);
  // Quick-open (and anything else workbench-y) resolves picked files
  // through the editor opener — route them to the app's own viewer.
  monaco.editor.registerEditorOpener({
    openCodeEditor(_source, resource) {
      if (resource.scheme !== "file" || openHandler == null) return false;
      openHandler(resource.path);
      return true;
    },
  });
  return monaco;
}

let workspaceDir: string | null = null;

/**
 * Point the VS Code workspace at one folder (quick-open's search root). The
 * Files pane calls this as it mounts/changes — one workspace at a time, last
 * pane wins.
 */
export async function setMonacoWorkspace(dir: string): Promise<void> {
  const monaco = await loadMonaco();
  if (workspaceDir === dir) return;
  const { reinitializeWorkspace } =
    await import("@codingame/monaco-vscode-configuration-service-override");
  await reinitializeWorkspace({ id: dir, uri: monaco.Uri.file(dir) });
  // Only marked done once the switch actually lands. Stamping it before the
  // await (as this used to) meant a rejection here left the guard above
  // believing a folder's workspace was already set, silently skipping every
  // later retry for that exact dir — the same class of bug b110362 fixed on
  // the LSP side, where an uncaught throw wedged its switch chain for good.
  workspaceDir = dir;
  // The LSP bridge follows the workspace (rust-analyzer per Rust checkout).
  const { syncLspWorkspace } = await import("@/lib/lsp");
  syncLspWorkspace(dir);
}

let detachSidebar: (() => void) | null = null;

/**
 * Host the VS Code Explorer (the workbench sidebar part) inside `container`.
 * The sidebar is a singleton — the pane that attached last owns it, and a
 * newer attach silently steals it (same last-wins semantics as the
 * workspace). Returns a detach that no-ops if someone else took over.
 */
export async function attachExplorer(container: HTMLElement): Promise<() => void> {
  await loadMonaco();
  const [views, layout] = await Promise.all([
    import("@codingame/monaco-vscode-views-service-override"),
    import("@codingame/monaco-vscode-api/vscode/vs/workbench/services/layout/browser/layoutService"),
  ]);
  detachSidebar?.();
  const attached = views.attachPart(layout.Parts.SIDEBAR_PART, container);
  const mine = () => {
    attached.dispose();
    if (detachSidebar === mine) detachSidebar = null;
  };
  detachSidebar = mine;
  return mine;
}

/** Run a VS Code command by id (e.g. the Explorer's refresh action). Command
 * failures are the command's problem, not the caller's — log and move on. */
export async function runMonacoCommand(id: string): Promise<void> {
  try {
    await loadMonaco();
    const api = await import("@codingame/monaco-vscode-api");
    const commands = await api.getService(api.ICommandService);
    await commands.executeCommand(id);
  } catch (e) {
    console.error(`[monaco] command ${id} failed`, e);
  }
}

type OpenFileHandler = (absolutePath: string) => void;
let openHandler: OpenFileHandler | null = null;

/** Where "open this file" requests from the VS Code layer (quick-open picks)
 * land — the active Files pane registers itself; null to unregister. */
export function setMonacoOpenHandler(handler: OpenFileHandler | null): void {
  openHandler = handler;
}
