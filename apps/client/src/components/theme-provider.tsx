import { createContext, useContext, useEffect, useState } from "react";

export type Theme = "dark" | "light" | "system";

/** Dark-mode color palettes, applied via `data-color-theme` under `.dark`.
 * Ported from vetted shadcn/ui community themes (not hand-derived) — see
 * index.css for sources. Only palettes with a real ported source are listed;
 * no from-scratch color guessing. */
export type ColorTheme = "default" | "nord" | "catppuccin";

export const COLOR_THEMES: { id: ColorTheme; label: string; swatch: string }[] = [
  { id: "default", label: "Default", swatch: "#a3a3a3" },
  { id: "nord", label: "Nord", swatch: "#86c0d0" },
  { id: "catppuccin", label: "Catppuccin Mocha", swatch: "#cba6f7" },
];

interface ThemeProviderProps {
  children: React.ReactNode;
  defaultTheme?: Theme;
  storageKey?: string;
  defaultColorTheme?: ColorTheme;
  colorThemeStorageKey?: string;
}

interface ThemeProviderState {
  theme: Theme;
  setTheme: (theme: Theme) => void;
  colorTheme: ColorTheme;
  setColorTheme: (colorTheme: ColorTheme) => void;
}

const initialState: ThemeProviderState = {
  theme: "system",
  setTheme: () => null,
  colorTheme: "default",
  setColorTheme: () => null,
};

const ThemeProviderContext = createContext<ThemeProviderState>(initialState);

export function ThemeProvider({
  children,
  defaultTheme = "system",
  storageKey = "tt-ui-theme",
  defaultColorTheme = "default",
  colorThemeStorageKey = "tt-ui-color-theme",
  ...props
}: ThemeProviderProps) {
  const [theme, setTheme] = useState<Theme>(
    () => (localStorage.getItem(storageKey) as Theme) || defaultTheme,
  );
  const [colorTheme, setColorTheme] = useState<ColorTheme>(
    () => (localStorage.getItem(colorThemeStorageKey) as ColorTheme) || defaultColorTheme,
  );

  useEffect(() => {
    const root = window.document.documentElement;
    root.classList.remove("light", "dark");

    if (theme === "system") {
      const systemTheme = window.matchMedia("(prefers-color-scheme: dark)").matches
        ? "dark"
        : "light";
      root.classList.add(systemTheme);
      return;
    }

    root.classList.add(theme);
  }, [theme]);

  useEffect(() => {
    window.document.documentElement.dataset.colorTheme = colorTheme;
  }, [colorTheme]);

  // Keep separate windows (e.g. the standalone Settings window) in sync: the
  // storage event fires in every other same-origin document when the theme
  // key changes, so a theme switch in one window updates the others live.
  useEffect(() => {
    const onStorage = (e: StorageEvent) => {
      if (e.key === storageKey && e.newValue) setTheme(e.newValue as Theme);
      if (e.key === colorThemeStorageKey && e.newValue) {
        setColorTheme(e.newValue as ColorTheme);
      }
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, [storageKey, colorThemeStorageKey]);

  const value = {
    theme,
    setTheme: (next: Theme) => {
      localStorage.setItem(storageKey, next);
      setTheme(next);
    },
    colorTheme,
    setColorTheme: (next: ColorTheme) => {
      localStorage.setItem(colorThemeStorageKey, next);
      setColorTheme(next);
    },
  };

  return (
    <ThemeProviderContext.Provider {...props} value={value}>
      {children}
    </ThemeProviderContext.Provider>
  );
}

export function useTheme() {
  const context = useContext(ThemeProviderContext);
  if (context === undefined) throw new Error("useTheme must be used within a ThemeProvider");
  return context;
}
