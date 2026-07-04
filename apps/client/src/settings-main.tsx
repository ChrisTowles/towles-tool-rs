import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "./index.css";
import { ThemeProvider } from "@/components/theme-provider";
import { TooltipProvider } from "@/components/ui/tooltip";
import { SettingsWindow } from "@/components/settings-window";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <ThemeProvider defaultTheme="system" storageKey="tt-ui-theme">
      <TooltipProvider>
        <SettingsWindow />
      </TooltipProvider>
    </ThemeProvider>
  </StrictMode>,
);
