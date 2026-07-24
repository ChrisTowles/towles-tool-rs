import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { COLOR_THEMES, type ColorTheme, type Theme } from "@/components/theme-provider";
import { SettingRow, type FilterSection } from "./common";

export function appearanceSections(
  theme: Theme,
  setTheme: (t: Theme) => void,
  colorTheme: ColorTheme,
  setColorTheme: (c: ColorTheme) => void,
): FilterSection[] {
  return [
    {
      rows: [
        {
          label: "Theme",
          keywords: ["appearance", "color", "light", "dark", "system"],
          node: (
            <SettingRow label="Theme" description="Light, dark, or follow the system.">
              <Select value={theme} onValueChange={(v) => setTheme(v as Theme)}>
                <SelectTrigger className="w-32">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="light">Light</SelectItem>
                  <SelectItem value="dark">Dark</SelectItem>
                  <SelectItem value="system">System</SelectItem>
                </SelectContent>
              </Select>
            </SettingRow>
          ),
        },
        {
          label: "Color theme",
          keywords: [
            "appearance",
            "color",
            "palette",
            "dracula",
            "nord",
            "gruvbox",
            "tokyo night",
            "catppuccin",
            "one dark",
          ],
          node: (
            <SettingRow label="Color theme" description="Palette used in dark mode.">
              <Select value={colorTheme} onValueChange={(v) => setColorTheme(v as ColorTheme)}>
                <SelectTrigger className="w-40">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {COLOR_THEMES.map((t) => (
                    <SelectItem key={t.id} value={t.id}>
                      <span className="flex items-center gap-2">
                        <span
                          className="size-2.5 rounded-full"
                          style={{ backgroundColor: t.swatch }}
                        />
                        {t.label}
                      </span>
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </SettingRow>
          ),
        },
      ],
    },
  ];
}
