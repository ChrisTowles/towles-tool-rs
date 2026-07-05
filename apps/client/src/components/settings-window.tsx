import { useEffect, useState } from "react";
import {
  FolderGit2,
  Info,
  Keyboard,
  NotebookPen,
  Palette,
  RefreshCw,
  SlidersHorizontal,
} from "lucide-react";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Kbd } from "@/components/ui/kbd";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { useTheme, type Theme } from "@/components/theme-provider";
import { abInvoke } from "@/lib/agentboard";
import { useUserSettings, type UserSettings } from "@/lib/settings";
import { closeCurrentWindow } from "@/lib/open-settings";

/** Real, known location of the settings file (shared with the TypeScript CLI). */
const SETTINGS_PATH = "~/.config/towles-tool/towles-tool.settings.json";

const TABS = [
  { id: "general", label: "General", icon: SlidersHorizontal },
  { id: "appearance", label: "Appearance", icon: Palette },
  { id: "agentboard", label: "Agentboard", icon: FolderGit2 },
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

function TabHeading({ title, note }: { title: string; note: string }) {
  return (
    <div className="flex flex-col gap-1">
      <h2 className="text-sm font-semibold">{title}</h2>
      <p className="text-sm text-muted-foreground">{note}</p>
    </div>
  );
}

type Update = (fn: (prev: UserSettings) => UserSettings) => void;

/** Stacked label + description above a full-width control (text/number rows). */
function FieldRow({
  label,
  description,
  children,
}: {
  label: string;
  description: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex flex-col gap-1.5">
      <div className="text-sm font-medium">{label}</div>
      <div className="text-sm text-muted-foreground">{description}</div>
      {children}
    </div>
  );
}

/** Toggle row: label + description on the left, a Switch on the right. */
function ToggleRow({
  label,
  description,
  checked,
  onCheckedChange,
}: {
  label: string;
  description: string;
  checked: boolean;
  onCheckedChange: (v: boolean) => void;
}) {
  return (
    <SettingRow label={label} description={description}>
      <Switch checked={checked} onCheckedChange={onCheckedChange} />
    </SettingRow>
  );
}

/** Small number field with a trailing unit (e.g. cadence in minutes). */
function CadenceRow({
  label,
  description,
  value,
  unit,
  onValue,
}: {
  label: string;
  description: string;
  value: number;
  unit: string;
  onValue: (n: number) => void;
}) {
  return (
    <SettingRow label={label} description={description}>
      <div className="flex items-center gap-2">
        <Input
          type="number"
          min={1}
          value={value}
          onChange={(e) => {
            const n = Number(e.target.value);
            if (Number.isFinite(n) && n >= 1) onValue(Math.floor(n));
          }}
          className="w-20"
        />
        <span className="text-sm text-muted-foreground">{unit}</span>
      </div>
    </SettingRow>
  );
}

/** Shown in wired tabs while settings load, or when there's no Tauri host. */
function SettingsLoading() {
  return <div className="text-sm text-muted-foreground">Loading settings…</div>;
}

function GeneralSettings({
  settings,
  update,
}: {
  settings: UserSettings | null;
  update: Update;
}) {
  if (!settings) return <SettingsLoading />;
  return (
    <FieldRow
      label="Preferred editor"
      description="Command used to open a repo (e.g. code, cursor, nvim). Runs as “<editor> <dir>”."
    >
      <Input
        value={settings.preferredEditor}
        onChange={(e) =>
          update((s) => ({ ...s, preferredEditor: e.target.value }))
        }
        placeholder="code"
        className="max-w-xs font-mono text-xs"
        spellCheck={false}
      />
    </FieldRow>
  );
}

function JournalSettingsForm({
  settings,
  update,
}: {
  settings: UserSettings | null;
  update: Update;
}) {
  if (!settings) return <SettingsLoading />;
  const j = settings.journalSettings;
  const setJournal = (patch: Partial<UserSettings["journalSettings"]>) =>
    update((s) => ({
      ...s,
      journalSettings: { ...s.journalSettings, ...patch },
    }));
  const field = (
    key: keyof UserSettings["journalSettings"],
    label: string,
    description: string,
  ) => (
    <FieldRow label={label} description={description}>
      <Input
        value={j[key]}
        onChange={(e) => setJournal({ [key]: e.target.value })}
        className="font-mono text-xs"
        spellCheck={false}
      />
    </FieldRow>
  );
  return (
    <>
      {field(
        "baseFolder",
        "Base folder",
        "Root directory all journal files are written under.",
      )}
      {field(
        "dailyPathTemplate",
        "Daily-note path",
        "Template for daily notes, relative to the base folder. Tokens like {yyyy}/{MM}/{dd}.",
      )}
      {field(
        "meetingPathTemplate",
        "Meeting-note path",
        "Template for meeting notes. Supports a {title} token.",
      )}
      {field(
        "notePathTemplate",
        "Note path",
        "Template for ad-hoc notes. Supports a {title} token.",
      )}
      {field(
        "templateDir",
        "Template directory",
        "Directory holding external note templates (built-ins used when absent).",
      )}
    </>
  );
}

function CollectorsSettingsForm({
  settings,
  update,
}: {
  settings: UserSettings | null;
  update: Update;
}) {
  if (!settings) return <SettingsLoading />;
  const c = settings.collectors;
  const setCal = (patch: Partial<UserSettings["collectors"]["calendar"]>) =>
    update((s) => ({
      ...s,
      collectors: {
        ...s.collectors,
        calendar: { ...s.collectors.calendar, ...patch },
      },
    }));
  const setPrs = (patch: Partial<UserSettings["collectors"]["prs"]>) =>
    update((s) => ({
      ...s,
      collectors: { ...s.collectors, prs: { ...s.collectors.prs, ...patch } },
    }));
  const setIssues = (patch: Partial<UserSettings["collectors"]["issues"]>) =>
    update((s) => ({
      ...s,
      collectors: {
        ...s.collectors,
        issues: { ...s.collectors.issues, ...patch },
      },
    }));
  return (
    <div className="flex flex-col gap-6">
      <div className="rounded-md border border-dashed p-3 text-xs text-muted-foreground">
        Collector cadence is read when the app starts, so changes here take
        effect on the next launch.
      </div>

      <section className="flex flex-col gap-4">
        <h3 className="text-sm font-semibold">Calendar</h3>
        <ToggleRow
          label="Enabled"
          description="Fetches your next meeting via claude -p (costs tokens)."
          checked={c.calendar.enabled}
          onCheckedChange={(v) => setCal({ enabled: v })}
        />
        <SettingRow label="Provider" description="Which calendar MCP to drive.">
          <Select
            value={c.calendar.provider}
            onValueChange={(v) => setCal({ provider: v })}
          >
            <SelectTrigger className="w-32">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="google">Google</SelectItem>
              <SelectItem value="outlook">Outlook</SelectItem>
            </SelectContent>
          </Select>
        </SettingRow>
        <CadenceRow
          label="Refresh every"
          description="How often to re-fetch the calendar."
          value={c.calendar.refreshMinutes}
          unit="min"
          onValue={(n) => setCal({ refreshMinutes: n })}
        />
      </section>

      <section className="flex flex-col gap-4">
        <h3 className="text-sm font-semibold">Pull requests</h3>
        <ToggleRow
          label="Enabled"
          description="Polls your PRs across repos via gh."
          checked={c.prs.enabled}
          onCheckedChange={(v) => setPrs({ enabled: v })}
        />
        <CadenceRow
          label="Refresh every"
          description="How often to re-poll PRs."
          value={c.prs.refreshSeconds}
          unit="sec"
          onValue={(n) => setPrs({ refreshSeconds: n })}
        />
      </section>

      <section className="flex flex-col gap-4">
        <h3 className="text-sm font-semibold">Issues</h3>
        <ToggleRow
          label="Enabled"
          description="Feeds the cross-repo board via gh."
          checked={c.issues.enabled}
          onCheckedChange={(v) => setIssues({ enabled: v })}
        />
        <CadenceRow
          label="Refresh every"
          description="How often to re-poll issues."
          value={c.issues.refreshMinutes}
          unit="min"
          onValue={(n) => setIssues({ refreshMinutes: n })}
        />
      </section>
    </div>
  );
}

/**
 * Scan-root editor for the Agentboard add-repo picker. Reads/writes `scanRoots`
 * in `~/.config/towles-tool/agentboard/repos.json` over the `ab_*` Tauri
 * commands (no shared settings file, no zod — pure Rust round-trip). One root
 * per line; empty falls back to `~/code`.
 */
function AgentboardSettings() {
  const [roots, setRoots] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    void abInvoke<string[]>("ab_get_scan_roots").then((r) =>
      setRoots((r ?? []).join("\n")),
    );
  }, []);

  const save = async () => {
    const list = (roots ?? "")
      .split("\n")
      .map((s) => s.trim())
      .filter(Boolean);
    await abInvoke("ab_set_scan_roots", { roots: list });
    setRoots(list.join("\n"));
    setSaved(true);
    window.setTimeout(() => setSaved(false), 1500);
  };

  if (roots === null) {
    return <div className="text-sm text-muted-foreground">Loading…</div>;
  }

  return (
    <div className="flex flex-col gap-3">
      <div>
        <div className="text-sm font-medium">Scan roots</div>
        <p className="text-sm text-muted-foreground">
          One directory per line. The{" "}
          <span className="font-mono">Add repo</span> picker scans these for git
          repos. Leave empty to use <span className="font-mono">~/code</span>. A
          leading <span className="font-mono">~</span> expands to your home
          directory.
        </p>
      </div>
      <Textarea
        value={roots}
        onChange={(e) => setRoots(e.target.value)}
        rows={5}
        placeholder="~/code"
        className="font-mono text-xs"
        spellCheck={false}
      />
      <div className="flex items-center gap-3">
        <Button size="sm" onClick={() => void save()}>
          Save
        </Button>
        {saved && <span className="text-xs text-muted-foreground">Saved.</span>}
      </div>
    </div>
  );
}

export function SettingsWindow() {
  const { theme, setTheme } = useTheme();
  const { settings, saveState, update, save } = useUserSettings();

  return (
    <div className="flex h-screen flex-col bg-background text-foreground">
      <header className="flex items-center border-b border-border bg-card px-4 py-3">
        <h1 className="font-heading text-sm font-semibold">Settings</h1>
      </header>

      <Tabs
        orientation="vertical"
        defaultValue="general"
        className="min-h-0 flex-1 gap-0"
      >
        <TabsList
          variant="line"
          className="h-full w-44 shrink-0 items-stretch gap-1 rounded-none border-r border-border bg-card p-2"
        >
          {TABS.map((t) => (
            <TabsTrigger
              key={t.id}
              value={t.id}
              className="justify-start gap-2 px-2 py-1.5"
            >
              <t.icon className="size-4" />
              {t.label}
            </TabsTrigger>
          ))}
        </TabsList>

        <div className="min-h-0 flex-1 overflow-y-auto">
          <TabsContent value="general" className="flex flex-col gap-5 p-4">
            <TabHeading title="General" note="Editor used to open repos." />
            <GeneralSettings settings={settings} update={update} />
          </TabsContent>

          <TabsContent value="appearance" className="flex flex-col gap-5 p-4">
            <TabHeading
              title="Appearance"
              note="Theme applies immediately across all windows."
            />
            <SettingRow
              label="Theme"
              description="Light, dark, or follow the system."
            >
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

          <TabsContent value="agentboard" className="flex flex-col gap-5 p-4">
            <TabHeading
              title="Agentboard"
              note="Where the add-repo picker looks for your git repos."
            />
            <AgentboardSettings />
          </TabsContent>

          <TabsContent value="journal" className="flex flex-col gap-5 p-4">
            <TabHeading
              title="Journal"
              note="Where notes live and how their file paths are templated."
            />
            <JournalSettingsForm settings={settings} update={update} />
          </TabsContent>

          <TabsContent value="collectors" className="flex flex-col gap-5 p-4">
            <TabHeading
              title="Collectors"
              note="Background jobs that fill the data hub. Each has an enable flag and cadence."
            />
            <CollectorsSettingsForm settings={settings} update={update} />
          </TabsContent>

          <TabsContent value="shortcuts" className="flex flex-col gap-5 p-4">
            <TabHeading
              title="Shortcuts"
              note="Keyboard shortcuts (⌘ on macOS, Ctrl elsewhere)."
            />
            <div className="flex flex-col">
              {SHORTCUTS.map((s, i) => (
                <div
                  key={s.keys}
                  className={`flex items-center justify-between py-2 ${
                    i > 0 ? "border-t border-border" : ""
                  }`}
                >
                  <span className="text-sm text-muted-foreground">
                    {s.action}
                  </span>
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
                <span className="font-mono text-xs break-all">
                  {SETTINGS_PATH}
                </span>
              </div>
            </div>
            <p className="text-sm text-muted-foreground">
              Shared with the TypeScript CLI. The General, Journal, and
              Collectors tabs read and write it directly; unknown keys the CLI
              owns are preserved on save. Theme and Agentboard scan roots
              persist separately.
            </p>
          </TabsContent>
        </div>
      </Tabs>

      <footer className="flex items-center justify-end gap-3 border-t border-border bg-card px-4 py-3">
        {saveState === "saved" && (
          <span className="text-xs text-muted-foreground">Saved.</span>
        )}
        {saveState === "error" && (
          <span className="text-xs text-destructive">Save failed.</span>
        )}
        <Button
          size="sm"
          onClick={() => void save()}
          disabled={!settings || saveState === "saving"}
        >
          {saveState === "saving" ? "Saving…" : "Save"}
        </Button>
        <Button
          variant="outline"
          size="sm"
          onClick={() => void closeCurrentWindow()}
        >
          Done
        </Button>
      </footer>
    </div>
  );
}
