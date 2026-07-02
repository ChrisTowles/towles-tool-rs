import { createContext, useContext, useMemo } from "react";
import type { CSSProperties, ReactNode } from "react";
import type { AgentStatus } from "../types";
import { PALETTE_KEYS, resolveTheme } from "../lib/themes";
import type { Theme } from "../lib/themes";

const ThemeContext = createContext<Theme | null>(null);

const STATUS_KEYS: AgentStatus[] = [
  "idle",
  "running",
  "done",
  "error",
  "waiting",
  "question",
  "interrupted",
];

/** CSS custom properties for the resolved theme: `--pal-*` + `--status-*`. */
function themeVars(theme: Theme): CSSProperties {
  const vars: Record<string, string> = {};
  for (const key of PALETTE_KEYS) vars[`--pal-${key}`] = theme.palette[key];
  for (const key of STATUS_KEYS) vars[`--status-${key}`] = theme.status[key];
  return vars as CSSProperties;
}

export interface ThemeProviderProps {
  themeName: string | undefined;
  children: ReactNode;
}

/**
 * Resolves `themeName` to a Theme, publishes it via context, and applies the
 * palette as CSS custom properties on a wrapping div (theme switch = one var
 * swap). Structural colors come from the vars; derived accent/status colors are
 * read from the theme object via `useTheme`.
 */
export function ThemeProvider({ themeName, children }: ThemeProviderProps) {
  const theme = useMemo(() => resolveTheme(themeName), [themeName]);
  return (
    <ThemeContext.Provider value={theme}>
      <div className="ab-theme-root" style={themeVars(theme)}>
        {children}
      </div>
    </ThemeContext.Provider>
  );
}

export function useTheme(): Theme {
  const theme = useContext(ThemeContext);
  if (!theme) throw new Error("useTheme must be used within a ThemeProvider");
  return theme;
}
