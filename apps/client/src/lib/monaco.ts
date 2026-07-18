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
 */

let loading: Promise<typeof import("monaco-editor")> | null = null;

export function loadMonaco(): Promise<typeof import("monaco-editor")> {
  loading ??= (async () => {
    const [
      monaco,
      api,
      configuration,
      languages,
      textmate,
      theme,
      model,
      quickaccess,
      search,
      tauriFs,
      editorWorker,
      textmateWorker,
      tsWorker,
      jsonWorker,
      cssWorker,
      htmlWorker,
    ] = await Promise.all([
      import("monaco-editor"),
      import("@codingame/monaco-vscode-api"),
      import("@codingame/monaco-vscode-configuration-service-override"),
      import("@codingame/monaco-vscode-languages-service-override"),
      import("@codingame/monaco-vscode-textmate-service-override"),
      import("@codingame/monaco-vscode-theme-service-override"),
      import("@codingame/monaco-vscode-model-service-override"),
      import("@codingame/monaco-vscode-quickaccess-service-override"),
      import("@codingame/monaco-vscode-search-service-override"),
      import("@/lib/monaco-fs"),
      import("monaco-editor/esm/vs/editor/editor.worker?worker"),
      import("@codingame/monaco-vscode-textmate-service-override/worker?worker"),
      import("@codingame/monaco-vscode-standalone-typescript-language-features/worker?worker"),
      import("@codingame/monaco-vscode-standalone-json-language-features/worker?worker"),
      import("@codingame/monaco-vscode-standalone-css-language-features/worker?worker"),
      import("@codingame/monaco-vscode-standalone-html-language-features/worker?worker"),
      // Importing a default-extension package registers its TextMate grammars
      // (or themes) as a built-in VS Code extension — side-effect imports.
      // (themeDefaults is awaited below: setTheme races its registration.)
    ]);
    const [themeDefaults] = await Promise.all([
      import("@codingame/monaco-vscode-theme-defaults-default-extension"),
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
      // Standalone language features: monaco's classic worker-based smarts
      // (completions/hovers/diagnostics for ts/js/json/css/html), rebuilt for
      // the vscode-api stack — no extension host needed.
      import("@codingame/monaco-vscode-standalone-typescript-language-features"),
      import("@codingame/monaco-vscode-standalone-json-language-features"),
      import("@codingame/monaco-vscode-standalone-css-language-features"),
      import("@codingame/monaco-vscode-standalone-html-language-features"),
    ]);
    self.MonacoEnvironment = {
      getWorker(_workerId: string, label: string): Worker {
        switch (label) {
          case "TextMateWorker":
            return new textmateWorker.default();
          case "typescript":
          case "javascript":
            return new tsWorker.default();
          case "json":
            return new jsonWorker.default();
          case "css":
          case "scss":
          case "less":
            return new cssWorker.default();
          case "html":
          case "handlebars":
          case "razor":
            return new htmlWorker.default();
          default:
            return new editorWorker.default();
        }
      },
    };
    // Through user config, seeded before the services start — `setTheme`
    // races the theme service's own async startup restore and loses.
    await configuration.initUserConfiguration(
      JSON.stringify({
        "workbench.colorTheme": "Default Dark Modern",
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
      ...search.default(),
    });
    tauriFs.registerTauriFileSystem();
    await themeDefaults.whenReady();
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
  })();
  return loading;
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
  workspaceDir = dir;
  const { reinitializeWorkspace } = await import(
    "@codingame/monaco-vscode-configuration-service-override"
  );
  await reinitializeWorkspace({ id: dir, uri: monaco.Uri.file(dir) });
}

type OpenFileHandler = (absolutePath: string) => void;
let openHandler: OpenFileHandler | null = null;

/** Where "open this file" requests from the VS Code layer (quick-open picks)
 * land — the active Files pane registers itself; null to unregister. */
export function setMonacoOpenHandler(handler: OpenFileHandler | null): void {
  openHandler = handler;
}
