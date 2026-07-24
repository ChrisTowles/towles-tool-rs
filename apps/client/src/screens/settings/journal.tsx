import { Input } from "@/components/ui/input";
import type { UserSettings } from "@/lib/settings";
import { FieldRow, type FilterRow, type FilterSection, type Flush, type Update } from "./common";

export function journalSections(
  settings: UserSettings,
  update: Update,
  flush: Flush,
): FilterSection[] {
  const j = settings.journalSettings;
  // Every field here is free text, so all of them defer.
  const setJournal = (patch: Partial<UserSettings["journalSettings"]>) =>
    update(
      (s) => ({
        ...s,
        journalSettings: { ...s.journalSettings, ...patch },
      }),
      { defer: true },
    );
  const field = (
    key: keyof UserSettings["journalSettings"],
    label: string,
    description: string,
    keywords: string[],
  ): FilterRow => ({
    label,
    keywords: ["journal", "note", "path", "template", ...keywords],
    node: (
      <FieldRow label={label} description={description}>
        <Input
          value={j[key]}
          onChange={(e) => setJournal({ [key]: e.target.value })}
          onBlur={() => void flush()}
          className="font-mono text-xs"
          spellCheck={false}
        />
      </FieldRow>
    ),
  });
  return [
    {
      rows: [
        field("baseFolder", "Base folder", "Root directory all journal files are written under.", [
          "folder",
          "directory",
          "root",
        ]),
        field(
          "dailyPathTemplate",
          "Daily-note path",
          "Template for daily notes, relative to the base folder. Tokens like {yyyy}/{MM}/{dd}.",
          ["daily"],
        ),
        field(
          "meetingPathTemplate",
          "Meeting-note path",
          "Template for meeting notes. Supports a {title} token.",
          ["meeting"],
        ),
        field(
          "notePathTemplate",
          "Note path",
          "Template for ad-hoc notes. Supports a {title} token.",
          ["ad-hoc"],
        ),
        field(
          "templateDir",
          "Template directory",
          "Directory holding external note templates (built-ins used when absent).",
          ["directory"],
        ),
      ],
    },
  ];
}
