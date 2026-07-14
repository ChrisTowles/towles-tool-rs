/**
 * Lazy Monaco loader. Everything is bundled locally by Vite (`?worker`
 * imports become self-contained worker chunks) — no CDN, works offline
 * inside the Tauri shell. The editor chunk is only fetched when a code
 * viewer actually mounts.
 */

let loading: Promise<typeof import("monaco-editor")> | null = null;

export function loadMonaco(): Promise<typeof import("monaco-editor")> {
  loading ??= (async () => {
    const [monaco, editorWorker, tsWorker, jsonWorker, cssWorker, htmlWorker] = await Promise.all([
      import("monaco-editor"),
      import("monaco-editor/esm/vs/editor/editor.worker?worker"),
      import("monaco-editor/esm/vs/language/typescript/ts.worker?worker"),
      import("monaco-editor/esm/vs/language/json/json.worker?worker"),
      import("monaco-editor/esm/vs/language/css/css.worker?worker"),
      import("monaco-editor/esm/vs/language/html/html.worker?worker"),
    ]);
    self.MonacoEnvironment = {
      getWorker(_workerId: string, label: string): Worker {
        switch (label) {
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
    return monaco;
  })();
  return loading;
}

/** Monaco theme name matching the app's current light/dark mode. */
export function monacoTheme(): string {
  return document.documentElement.classList.contains("dark") ? "vs-dark" : "vs";
}
