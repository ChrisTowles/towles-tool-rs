import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "./index.css";
import { ThemeProvider } from "@/components/theme-provider";
import { App } from "./App";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <ThemeProvider defaultTheme="system" storageKey="tt-ui-theme">
      <App />
    </ThemeProvider>
  </StrictMode>,
);
