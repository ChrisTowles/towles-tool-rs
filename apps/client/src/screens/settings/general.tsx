import { Input } from "@/components/ui/input";
import type { UserSettings } from "@/lib/settings";
import { FieldRow, type FilterSection, type Flush, type Update } from "./common";

export function generalSections(
  settings: UserSettings,
  update: Update,
  flush: Flush,
): FilterSection[] {
  return [
    {
      rows: [
        {
          label: "Preferred editor",
          keywords: ["repo", "open", "editor", "code", "cursor", "nvim"],
          node: (
            <FieldRow
              label="Preferred editor"
              description="Command used to open a repo (e.g. code, cursor, nvim). Runs as “<editor> <dir>”."
            >
              <Input
                value={settings.preferredEditor}
                onChange={(e) =>
                  update((s) => ({ ...s, preferredEditor: e.target.value }), { defer: true })
                }
                onBlur={() => void flush()}
                placeholder="code"
                className="max-w-xs font-mono text-xs"
                spellCheck={false}
              />
            </FieldRow>
          ),
        },
      ],
    },
  ];
}
