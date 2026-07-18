import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "./index.css";
import { ThemeProvider } from "@/components/theme-provider";
import { installConsoleCollector } from "@/lib/wdio-console";
import { App } from "./App";

// E2E only: load the WebdriverIO Tauri plugin so tests get browser.tauri.execute /
// .mock. Gated on VITE_WDIO so it's tree-shaken out of normal/production bundles.
if (import.meta.env.VITE_WDIO) {
  // Synchronous, and before `createRoot` below: React reports invalid markup as
  // a console.error during the first render, so a dynamic import would install
  // the collector too late to catch the very warnings worth catching.
  installConsoleCollector();
  void import("@wdio/tauri-plugin");
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <ThemeProvider defaultTheme="system" storageKey="tt-ui-theme">
      <App />
    </ThemeProvider>
  </StrictMode>,
);
