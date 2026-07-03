import { Moon, Sun } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useTheme } from "@/components/theme-provider";

/**
 * Flips between explicit light and dark. "system" (the default until the user
 * clicks) resolves against the OS preference here so the first click always
 * inverts what's currently on screen.
 */
export function ThemeToggle() {
  const { theme, setTheme } = useTheme();

  const resolved =
    theme === "system"
      ? window.matchMedia("(prefers-color-scheme: dark)").matches
        ? "dark"
        : "light"
      : theme;

  return (
    <Button
      variant="outline"
      size="icon"
      aria-label="Toggle theme"
      onClick={() => setTheme(resolved === "dark" ? "light" : "dark")}
    >
      {resolved === "dark" ? <Sun /> : <Moon />}
    </Button>
  );
}
