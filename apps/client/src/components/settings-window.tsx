import { Info, Keyboard, NotebookPen, Palette, RefreshCw, SlidersHorizontal } from "lucide-react";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useTheme, type Theme } from "@/components/theme-provider";
import { settingsJson, settingsPath } from "@/lib/mock-data";
import { closeCurrentWindow } from "@/lib/open-settings";

const TABS = [
  { id: "general", label: "General", icon: SlidersHorizontal },
  { id: "appearance", label: "Appearance", icon: Palette },
  { id: "journal", label: "Journal", icon: NotebookPen },
  { id: "collectors", label: "Collectors", icon: RefreshCw },
  { id: "shortcuts", label: "Shortcuts", icon: Keyboard },
  { id: "about", label: "About", icon: Info },
] as const;

const SHORTCUTS = [
  { keys: "⌘K", action: "Open command palette / search" },
  { keys: "⌘,", action: "Open Settings" },
  { keys: "⌘B", action: "Toggle sidebar" },
  { keys: "⌘W", action: "Close the active tab" },
  { keys: "⌘J", action: "Quick log to today's journal" },
  { keys: "⌘D", action: "Split the focused terminal" },
];

/** Toggle/select row: label + description on the left, control on the right. */
function SettingRow({
  label,
  description,
  children,
}: {
  label: string;
  description: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-center justify-between gap-4">
      <div>
        <div className="text-sm font-medium">{label}</div>
        <div className="text-sm text-muted-foreground">{description}</div>
      </div>
      {children}
    </div>
  );
}

/** Stacked field: label above a full-width control, optional hint below. */
function Field({
  label,
  hint,
  mono,
  defaultValue,
}: {
  label: string;
  hint?: string;
  mono?: boolean;
  defaultValue: string;
}) {
  return (
    <div className="flex flex-col gap-1.5">
      <div className="text-sm font-medium">{label}</div>
      <Input defaultValue={defaultValue} className={mono ? "font-mono text-xs" : undefined} />
      {hint && <div className="text-xs text-muted-foreground">{hint}</div>}
    </div>
  );
}

function TabHeading({ title, note }: { title: string; note: string }) {
  return (
    <div className="flex flex-col gap-1">
      <h2 className="text-sm font-semibold">{title}</h2>
      <p className="text-sm text-muted-foreground">{note}</p>
    </div>
  );
}

export function SettingsWindow() {
  const { theme, setTheme } = useTheme();
  // Local-only placeholders until settings read/write goes through a Tauri
  // command. Seeded uncontrolled from the shared towles-tool.settings.json shape
  // (tt-config's UserSettings) so the UI reflects real defaults; Theme is the
  // one exception below — it's wired to useTheme() and persists now.
  const c = settingsJson.collectors;
  const j = settingsJson.journalSettings;

  return (
    <div className="flex h-screen flex-col bg-background text-foreground">
      <header className="flex items-center border-b border-border bg-card px-4 py-3">
        <h1 className="font-heading text-sm font-semibold">Settings</h1>
      </header>

      <Tabs orientation="vertical" defaultValue="general" className="min-h-0 flex-1 gap-0">
        <TabsList
          variant="line"
          className="h-full w-44 shrink-0 items-stretch gap-1 rounded-none border-r border-border bg-card p-2"
        >
          {TABS.map((t) => (
            <TabsTrigger key={t.id} value={t.id} className="justify-start gap-2 px-2 py-1.5">
              <t.icon className="size-4" />
              {t.label}
            </TabsTrigger>
          ))}
        </TabsList>

        <div className="min-h-0 flex-1 overflow-y-auto">
          <TabsContent value="general" className="flex flex-col gap-5 p-4">
            <TabHeading title="General" note="Editor and startup behavior." />
            <Field
              label="Preferred editor"
              hint="Command used to open files and notes."
              mono
              defaultValue={settingsJson.preferredEditor}
            />
            <SettingRow
              label="Open journal on launch"
              description="Show today's note when the app starts."
            >
              <Switch defaultChecked />
            </SettingRow>
            <SettingRow
              label="Doctor status in status bar"
              description="Show check results at the bottom of the window."
            >
              <Switch defaultChecked />
            </SettingRow>
          </TabsContent>

          <TabsContent value="appearance" className="flex flex-col gap-5 p-4">
            <TabHeading title="Appearance" note="Theme applies immediately across all windows." />
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
          </TabsContent>

          <TabsContent value="journal" className="flex flex-col gap-5 p-4">
            <TabHeading
              title="Journal"
              note="Where notes live and how their file paths are templated."
            />
            <Field label="Base folder" mono defaultValue={j.baseFolder} />
            <Field label="Daily note path" mono defaultValue={j.dailyPathTemplate} />
            <Field label="Meeting path" mono defaultValue={j.meetingPathTemplate} />
            <Field label="Note path" mono defaultValue={j.notePathTemplate} />
            <Field label="Template directory" mono defaultValue={j.templateDir} />
          </TabsContent>

          <TabsContent value="collectors" className="flex flex-col gap-5 p-4">
            <TabHeading
              title="Collectors"
              note="Background jobs that fill the data hub. Each has an enable flag and cadence."
            />
            <SettingRow
              label="Calendar"
              description={`Via claude -p · every ${c.calendar.refreshMinutes} min`}
            >
              <div className="flex items-center gap-3">
                <Select defaultValue={c.calendar.provider}>
                  <SelectTrigger className="w-28">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="google">Google</SelectItem>
                    <SelectItem value="outlook">Outlook</SelectItem>
                  </SelectContent>
                </Select>
                <Switch defaultChecked={c.calendar.enabled} />
              </div>
            </SettingRow>
            <SettingRow
              label="Pull requests"
              description={`Via gh · every ${c.prs.refreshSeconds}s`}
            >
              <Switch defaultChecked={c.prs.enabled} />
            </SettingRow>
            <SettingRow label="Issues" description={`Via gh · every ${c.issues.refreshMinutes} min`}>
              <Switch defaultChecked={c.issues.enabled} />
            </SettingRow>
          </TabsContent>

          <TabsContent value="shortcuts" className="flex flex-col gap-5 p-4">
            <TabHeading title="Shortcuts" note="Keyboard shortcuts (⌘ on macOS, Ctrl elsewhere)." />
            <div className="flex flex-col">
              {SHORTCUTS.map((s, i) => (
                <div
                  key={s.keys}
                  className={`flex items-center justify-between py-2 ${
                    i > 0 ? "border-t border-border" : ""
                  }`}
                >
                  <span className="text-sm text-muted-foreground">{s.action}</span>
                  <Kbd>{s.keys}</Kbd>
                </div>
              ))}
            </div>
          </TabsContent>

          <TabsContent value="about" className="flex flex-col gap-5 p-4">
            <TabHeading title="About" note="Towles Tool desktop app." />
            <div className="flex flex-col gap-2 text-sm">
              <div className="flex justify-between">
                <span className="text-muted-foreground">Version</span>
                <span className="font-mono">ttr v0.1.0</span>
              </div>
              <div className="flex justify-between">
                <span className="text-muted-foreground">Identifier</span>
                <span className="font-mono">dev.towles.tool</span>
              </div>
              <div className="flex flex-col gap-1">
                <span className="text-muted-foreground">Settings file</span>
                <span className="font-mono text-xs break-all">{settingsPath}</span>
              </div>
            </div>
            <p className="text-sm text-muted-foreground">
              This file is shared with the TypeScript CLI. Settings above are placeholders until the
              Tauri command that reads and writes it lands — Theme is the exception and persists now.
            </p>
          </TabsContent>
        </div>
      </Tabs>

      <footer className="flex justify-end border-t border-border bg-card px-4 py-3">
        <Button variant="outline" size="sm" onClick={() => void closeCurrentWindow()}>
          Done
        </Button>
      </footer>
    </div>
  );
}
