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
    const [monaco, api, configuration, languages, textmate, theme, editorWorker, textmateWorker] =
      await Promise.all([
      import("monaco-editor"),
      import("@codingame/monaco-vscode-api"),
      import("@codingame/monaco-vscode-configuration-service-override"),
      import("@codingame/monaco-vscode-languages-service-override"),
      import("@codingame/monaco-vscode-textmate-service-override"),
      import("@codingame/monaco-vscode-theme-service-override"),
      import("monaco-editor/esm/vs/editor/editor.worker?worker"),
      import("@codingame/monaco-vscode-textmate-service-override/worker?worker"),
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
    ]);
    self.MonacoEnvironment = {
      getWorker(_workerId: string, label: string): Worker {
        return label === "TextMateWorker"
          ? new textmateWorker.default()
          : new editorWorker.default();
      },
    };
    // Through user config, seeded before the services start — `setTheme`
    // races the theme service's own async startup restore and loses.
    await configuration.initUserConfiguration(
      JSON.stringify({ "workbench.colorTheme": "Default Dark Modern" }),
    );
    await api.initialize({
      ...configuration.default(),
      ...languages.default(),
      ...textmate.default(),
      ...theme.default(),
    });
    await themeDefaults.whenReady();
    return monaco;
  })();
  return loading;
}
