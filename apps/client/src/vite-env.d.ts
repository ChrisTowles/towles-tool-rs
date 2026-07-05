/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** Set to "1" for E2E builds to load the WebdriverIO Tauri plugin. */
  readonly VITE_WDIO?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
