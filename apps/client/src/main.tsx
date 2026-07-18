import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { MotionConfig } from "motion/react";
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
      {/* App-level motion policy, and deliberately only that: reducedMotion
          honors prefers-reduced-motion for every motion component at once, so
          no component hand-rolls motion-reduce classes. Per-animation tuning
          (durations, easings) belongs with the animation — see
          lib/rail-motion.ts — not in this global default.

          Yaak wraps this in <LazyMotion strict> to code-split motion's feature
          bundle, which is why their components use `m.*`. That split only
          works when every AnimatePresence consumer is behind a lazy chunk (its
          toasts/dialogs are). Ours isn't — the agentboard screen is statically
          imported — so a build puts motion in the initial chunk either way,
          and LazyMotion would buy nothing but the `m.*` ergonomic tax. */}
      <MotionConfig reducedMotion="user">
        <App />
      </MotionConfig>
    </ThemeProvider>
  </StrictMode>,
);
