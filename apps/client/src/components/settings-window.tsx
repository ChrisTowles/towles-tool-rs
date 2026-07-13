import { Fragment, useEffect, useRef, useState } from "react";
import {
  Check,
  ChevronsUpDown,
  Eye,
  EyeOff,
  FolderGit2,
  Info,
  Keyboard,
  NotebookPen,
  Palette,
  RefreshCw,
  Search,
  SlidersHorizontal,
} from "lucide-react";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Kbd, KbdGroup } from "@/components/ui/kbd";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { COLOR_THEMES, useTheme, type ColorTheme, type Theme } from "@/components/theme-provider";
import { abInvoke } from "@/lib/agentboard";
import { closeCurrentWindow } from "@/lib/open-settings";
import { isEmptyQuery, matchesFilter } from "@/lib/settings-filter";
import { slackListUsers, type SlackUser } from "@/lib/slack";
import { useUserSettings, type UserSettings } from "@/lib/settings";
import { DEFAULT_TERMINAL_FONT_SIZE, clampTerminalFontSize } from "@/lib/terminal-prefs";
import { SHORTCUTS, shortcutKeys, type ShortcutScope } from "@/lib/shortcuts";
import { useAppVersion } from "@/lib/version";
import { cn } from "@/lib/utils";

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

const SCOPE_LABELS: Record<ShortcutScope, string> = {
  global: "",
  agentboard: "Agentboard",
};

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

/** Default context-usage % at which a session is flagged for compaction
 * (mirrors `tt_config::DEFAULT_COMPACT_RECOMMEND_PERCENT`). */
const DEFAULT_COMPACT_RECOMMEND_PERCENT = 30;

/** Parse a text hour into a 0–23 int (ignoring junk by clamping). */
function clampHour(raw: string): number {
  const n = Math.floor(Number(raw));
  if (!Number.isFinite(n)) return 0;
  return Math.min(23, Math.max(0, n));
}

/** Password-style input with a show/hide toggle, for secret tokens. */
function RevealInput({
  value,
  onChange,
  placeholder,
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
}) {
  const [shown, setShown] = useState(false);
  return (
    <div className="relative">
      <Input
        type={shown ? "text" : "password"}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        className="pr-9 font-mono text-xs"
        spellCheck={false}
        autoComplete="off"
      />
      <button
        type="button"
        onClick={() => setShown((s) => !s)}
        aria-label={shown ? "Hide token" : "Show token"}
        className="absolute top-1/2 right-2 -translate-y-1/2 text-muted-foreground hover:text-foreground"
      >
        {shown ? <EyeOff className="size-4" /> : <Eye className="size-4" />}
      </button>
    </div>
  );
}

/** Weekday chips (0 = Monday … 6 = Sunday, matching the Rust quiet-hours mask). */
const WEEKDAY_LABELS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

function WeekdayChips({
  value,
  onChange,
}: {
  value: number[];
  onChange: (days: number[]) => void;
}) {
  const toggle = (day: number) => {
    const next = value.includes(day) ? value.filter((d) => d !== day) : [...value, day];
    next.sort((a, b) => a - b);
    onChange(next);
  };
  return (
    <div className="flex flex-wrap gap-1.5">
      {WEEKDAY_LABELS.map((label, day) => {
        const on = value.includes(day);
        return (
          <button
            key={day}
            type="button"
            onClick={() => toggle(day)}
            aria-pressed={on}
            className={cn(
              "rounded-md border px-2.5 py-1 text-xs font-medium transition-colors",
              on
                ? "border-primary bg-primary text-primary-foreground"
                : "border-border bg-background text-muted-foreground hover:bg-muted",
            )}
          >
            {label}
          </button>
        );
      })}
    </div>
  );
}

/**
 * Pick the watched user from the workspace directory (users.list) so a name is
 * chosen instead of pasting a member id. Loads members lazily; when the token is
 * empty/invalid, the fetch fails, or outside the Tauri shell, it degrades to a
 * plain member-id text input.
 */
function SlackUserPicker({
  userId,
  userName,
  onPick,
  onIdChange,
}: {
  userId: string;
  userName: string;
  onPick: (user: SlackUser) => void;
  onIdChange: (id: string) => void;
}) {
  const [users, setUsers] = useState<SlackUser[] | null>(null);
  const [failed, setFailed] = useState(false);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    let alive = true;
    void slackListUsers()
      .then((list) => alive && setUsers(list))
      .catch(() => alive && setFailed(true));
    return () => {
      alive = false;
    };
  }, []);

  // No usable directory (browser dev, empty/invalid token, load error, or an
  // empty workspace): fall back to a plain member-id input.
  if (failed || (users !== null && users.length === 0)) {
    return (
      <Input
        value={userId}
        onChange={(e) => onIdChange(e.target.value)}
        className="font-mono text-xs"
        placeholder="U0123ABCD"
        spellCheck={false}
      />
    );
  }
  if (users === null) {
    return <div className="text-xs text-muted-foreground">Loading members…</div>;
  }

  const selected = users.find((u) => u.id === userId);
  const label = selected?.name || userName || userId || "Select a person…";
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          variant="outline"
          role="combobox"
          aria-expanded={open}
          className="w-full justify-between font-normal"
        >
          <span className="truncate">{label}</span>
          <ChevronsUpDown className="ml-2 size-4 shrink-0 opacity-50" />
        </Button>
      </PopoverTrigger>
      <PopoverContent className="w-[--radix-popover-trigger-width] p-0" align="start">
        <Command>
          <CommandInput placeholder="Search people…" />
          <CommandList>
            <CommandEmpty>No match.</CommandEmpty>
            <CommandGroup>
              {users.map((u) => (
                <CommandItem
                  key={u.id}
                  value={`${u.name} ${u.id}`}
                  onSelect={() => {
                    onPick(u);
                    setOpen(false);
                  }}
                >
                  <Check
                    className={cn("mr-2 size-4", u.id === userId ? "opacity-100" : "opacity-0")}
                  />
                  <span className="truncate">{u.name}</span>
                  <span className="ml-2 font-mono text-[10px] text-muted-foreground">{u.id}</span>
                </CommandItem>
              ))}
            </CommandGroup>
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}

/** Shown in wired tabs while settings load, or when there's no Tauri host. */
function SettingsLoading() {
  return <div className="text-sm text-muted-foreground">Loading settings…</div>;
}

/**
 * One filterable row: `label` + `keywords` feed the filter predicate; `node` is
 * the rendered control. Keywords carry synonyms and the section name so a row
 * like "Enabled" is still discoverable by typing "slack".
 */
type FilterRow = {
  label: string;
  keywords?: string[];
  node: React.ReactNode;
};

/** A named (or anonymous) group of rows. Its heading hides when no row matches. */
type FilterSection = {
  heading?: string;
  keywords?: string[];
  rows: FilterRow[];
};

function rowKeywords(section: FilterSection, row: FilterRow): string[] {
  return [
    ...(row.keywords ?? []),
    ...(section.keywords ?? []),
    ...(section.heading ? [section.heading] : []),
  ];
}

/** Empty state shown when the current filter hides every row in a tab. */
function NoMatches({ query }: { query: string }) {
  return (
    <div className="rounded-md border border-dashed p-6 text-center text-sm text-muted-foreground">
      No settings match “{query.trim()}”.
    </div>
  );
}

/**
 * Render a tab's sections, filtered by `query`. Rows that don't match are
 * dropped; a section with no remaining rows drops its heading too; if nothing
 * survives, the empty state renders instead. An empty query shows everything
 * (plus the optional `prelude`, which is a filtering-irrelevant note).
 */
function FilteredContent({
  query,
  sections,
  prelude,
}: {
  query: string;
  sections: FilterSection[];
  prelude?: React.ReactNode;
}) {
  const empty = isEmptyQuery(query);
  const visible = sections
    .map((section) => ({
      section,
      rows: empty
        ? section.rows
        : section.rows.filter((row) =>
            matchesFilter(query, row.label, rowKeywords(section, row)),
          ),
    }))
    .filter((entry) => entry.rows.length > 0);

  if (visible.length === 0) return <NoMatches query={query} />;

  return (
    <>
      {empty && prelude}
      {visible.map(({ section, rows }, i) => (
        <section key={section.heading ?? i} className="flex flex-col gap-4">
          {section.heading && (
            <h3 className="text-sm font-semibold">{section.heading}</h3>
          )}
          {rows.map((row) => (
            <Fragment key={row.label}>{row.node}</Fragment>
          ))}
        </section>
      ))}
    </>
  );
}

function generalSections(
  settings: UserSettings,
  update: Update,
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
                  update((s) => ({ ...s, preferredEditor: e.target.value }))
                }
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

function appearanceSections(
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
            <SettingRow
              label="Color theme"
              description="Palette used in dark mode."
            >
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

function journalSections(
  settings: UserSettings,
  update: Update,
): FilterSection[] {
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
    keywords: string[],
  ): FilterRow => ({
    label,
    keywords: ["journal", "note", "path", "template", ...keywords],
    node: (
      <FieldRow label={label} description={description}>
        <Input
          value={j[key]}
          onChange={(e) => setJournal({ [key]: e.target.value })}
          className="font-mono text-xs"
          spellCheck={false}
        />
      </FieldRow>
    ),
  });
  return [
    {
      rows: [
        field(
          "baseFolder",
          "Base folder",
          "Root directory all journal files are written under.",
          ["folder", "directory", "root"],
        ),
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

function collectorsSections(
  settings: UserSettings,
  update: Update,
): FilterSection[] {
  const c = settings.collectors;
  const setCollector = <K extends keyof UserSettings["collectors"]>(
    key: K,
    patch: Partial<UserSettings["collectors"][K]>,
  ) =>
    update((s) => ({
      ...s,
      collectors: {
        ...s.collectors,
        [key]: { ...s.collectors[key], ...patch },
      },
    }));
  const setCal = (patch: Partial<UserSettings["collectors"]["calendar"]>) =>
    setCollector("calendar", patch);
  const setCalQuiet = (patch: Partial<UserSettings["collectors"]["calendar"]["quietHours"]>) =>
    setCal({ quietHours: { ...c.calendar.quietHours, ...patch } });
  const setPrs = (patch: Partial<UserSettings["collectors"]["prs"]>) =>
    setCollector("prs", patch);
  const setIssues = (patch: Partial<UserSettings["collectors"]["issues"]>) =>
    setCollector("issues", patch);
  const setSlack = (patch: Partial<UserSettings["collectors"]["slack"]>) =>
    setCollector("slack", patch);

  return [
    {
      heading: "Calendar",
      keywords: ["collector", "meeting", "google", "outlook"],
      rows: [
        {
          label: "Enabled",
          node: (
            <ToggleRow
              label="Enabled"
              description="Fetches your next meeting via claude -p (costs tokens)."
              checked={c.calendar.enabled}
              onCheckedChange={(v) => setCal({ enabled: v })}
            />
          ),
        },
        {
          label: "Provider",
          keywords: ["google", "outlook", "mcp"],
          node: (
            <SettingRow
              label="Provider"
              description="Which calendar MCP to drive."
            >
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
          ),
        },
        {
          label: "Refresh every",
          keywords: ["cadence", "interval"],
          node: (
            <CadenceRow
              label="Refresh every"
              description="How often to re-fetch the calendar."
              value={c.calendar.refreshMinutes}
              unit="min"
              onValue={(n) => setCal({ refreshMinutes: n })}
            />
          ),
        },
        {
          label: "Quiet hours",
          keywords: ["working hours", "window", "nights", "weekends", "gate", "tokens"],
          node: (
            <ToggleRow
              label="Quiet hours"
              description="Only run the token-costing calendar collector inside a working-hours window (skips nights and weekends)."
              checked={c.calendar.quietHours.enabled}
              onCheckedChange={(v) => setCalQuiet({ enabled: v })}
            />
          ),
        },
        {
          label: "Active window",
          keywords: ["quiet hours", "start", "end", "hour", "working hours"],
          node: (
            <SettingRow
              label="Active window"
              description="Local hours the collector may run, start inclusive to end exclusive (0–23)."
            >
              <div className="flex items-center gap-2">
                <Input
                  type="number"
                  min={0}
                  max={23}
                  value={c.calendar.quietHours.startHour}
                  onChange={(e) => setCalQuiet({ startHour: clampHour(e.target.value) })}
                  disabled={!c.calendar.quietHours.enabled}
                  className="w-16"
                />
                <span className="text-sm text-muted-foreground">to</span>
                <Input
                  type="number"
                  min={0}
                  max={23}
                  value={c.calendar.quietHours.endHour}
                  onChange={(e) => setCalQuiet({ endHour: clampHour(e.target.value) })}
                  disabled={!c.calendar.quietHours.enabled}
                  className="w-16"
                />
              </div>
            </SettingRow>
          ),
        },
        {
          label: "Active days",
          keywords: ["quiet hours", "weekdays", "days", "monday", "weekend"],
          node: (
            <FieldRow
              label="Active days"
              description="Weekdays the collector may run."
            >
              <WeekdayChips
                value={c.calendar.quietHours.weekdays}
                onChange={(days) => setCalQuiet({ weekdays: days })}
              />
            </FieldRow>
          ),
        },
      ],
    },
    {
      heading: "Pull requests",
      keywords: ["collector", "pr", "prs", "gh", "github"],
      rows: [
        {
          label: "Enabled",
          node: (
            <ToggleRow
              label="Enabled"
              description="Polls your PRs across repos via gh."
              checked={c.prs.enabled}
              onCheckedChange={(v) => setPrs({ enabled: v })}
            />
          ),
        },
        {
          label: "Refresh every",
          keywords: ["cadence", "interval"],
          node: (
            <CadenceRow
              label="Refresh every"
              description="How often to re-poll PRs."
              value={c.prs.refreshSeconds}
              unit="sec"
              onValue={(n) => setPrs({ refreshSeconds: n })}
            />
          ),
        },
      ],
    },
    {
      heading: "Issues",
      keywords: ["collector", "issue", "gh", "github", "board"],
      rows: [
        {
          label: "Enabled",
          node: (
            <ToggleRow
              label="Enabled"
              description="Feeds the cross-repo board via gh."
              checked={c.issues.enabled}
              onCheckedChange={(v) => setIssues({ enabled: v })}
            />
          ),
        },
        {
          label: "Refresh every",
          keywords: ["cadence", "interval"],
          node: (
            <CadenceRow
              label="Refresh every"
              description="How often to re-poll issues."
              value={c.issues.refreshMinutes}
              unit="min"
              onValue={(n) => setIssues({ refreshMinutes: n })}
            />
          ),
        },
      ],
    },
    {
      heading: "Slack DM watch",
      keywords: ["collector", "slack", "dm", "message", "banner"],
      rows: [
        {
          label: "Enabled",
          node: (
            <ToggleRow
              label="Enabled"
              description="Watches one DM (e.g. your wife) and raises the attention banner on unanswered messages."
              checked={c.slack.enabled}
              onCheckedChange={(v) => setSlack({ enabled: v })}
            />
          ),
        },
        {
          label: "User token",
          keywords: ["oauth", "xoxp", "secret"],
          node: (
            <FieldRow
              label="User token"
              description="Slack user OAuth token (xoxp-…) with im:history + im:read scopes (chat:write to reply, files:read for images)."
            >
              <RevealInput
                value={c.slack.token}
                onChange={(v) => setSlack({ token: v })}
                placeholder="xoxp-…"
              />
            </FieldRow>
          ),
        },
        {
          label: "App-level token (Socket Mode)",
          keywords: ["socket", "realtime", "xapp", "app-level", "instant", "live"],
          node: (
            <FieldRow
              label="App-level token (Socket Mode)"
              description="Optional app-level token (xapp-…) with connections:write for real-time DM delivery. Empty = poll only."
            >
              <RevealInput
                value={c.slack.appToken}
                onChange={(v) => setSlack({ appToken: v })}
                placeholder="xapp-…"
              />
            </FieldRow>
          ),
        },
        {
          label: "Watch user",
          keywords: ["member", "user id", "person", "pick", "who"],
          node: (
            <FieldRow
              label="Watch user"
              description="The person to watch. Picked from your workspace when the token is set, otherwise paste their member ID."
            >
              <SlackUserPicker
                userId={c.slack.watchUserId}
                userName={c.slack.watchName}
                onPick={(u) => setSlack({ watchUserId: u.id, watchName: u.name })}
                onIdChange={(id) => setSlack({ watchUserId: id })}
              />
            </FieldRow>
          ),
        },
        {
          label: "Display name",
          keywords: ["name", "banner"],
          node: (
            <FieldRow
              label="Display name"
              description="Name shown in the banner (set automatically when you pick a user)."
            >
              <Input
                value={c.slack.watchName}
                onChange={(e) => setSlack({ watchName: e.target.value })}
                placeholder="Sarah"
                spellCheck={false}
              />
            </FieldRow>
          ),
        },
        {
          label: "Refresh every",
          keywords: ["cadence", "interval"],
          node: (
            <CadenceRow
              label="Refresh every"
              description="How often to poll the DM (min 30s)."
              value={c.slack.refreshSeconds}
              unit="sec"
              onValue={(n) => setSlack({ refreshSeconds: n })}
            />
          ),
        },
      ],
    },
  ];
}

function agentboardSections(
  settings: UserSettings | null,
  update: Update,
): FilterSection[] {
  const rows: FilterRow[] = [
    {
      label: "Scan roots",
      keywords: ["repo", "discovery", "directory", "picker", "add repo"],
      node: <AgentboardSettings />,
    },
  ];
  if (settings) {
    rows.push(
      {
        label: "Needs-you notifications",
        keywords: ["notification", "desktop", "needs you", "alert"],
        node: (
          <ToggleRow
            label="Needs-you notifications"
            description="Desktop notification when an agent session flips to needs-you while the app is unfocused. Status only — act in the session's terminal."
            checked={settings.agentboard?.notifyNeedsYou ?? true}
            onCheckedChange={(v) =>
              update((s) => ({
                ...s,
                agentboard: { ...s.agentboard, notifyNeedsYou: v },
              }))
            }
          />
        ),
      },
      {
        label: "Meeting-start notifications",
        keywords: ["notification", "desktop", "meeting", "countdown", "alert"],
        node: (
          <ToggleRow
            label="Meeting-start notifications"
            description="Desktop notification when the next meeting's countdown reaches zero, while the app is unfocused."
            checked={settings.agentboard?.notifyMeetingStart ?? true}
            onCheckedChange={(v) =>
              update((s) => ({
                ...s,
                agentboard: { ...s.agentboard, notifyMeetingStart: v },
              }))
            }
          />
        ),
      },
      {
        label: "Review-requested notifications",
        keywords: ["notification", "desktop", "pr", "review", "alert"],
        node: (
          <ToggleRow
            label="Review-requested notifications"
            description="Desktop notification when a PR newly needs your review, while the app is unfocused."
            checked={settings.agentboard?.notifyReviewRequested ?? true}
            onCheckedChange={(v) =>
              update((s) => ({
                ...s,
                agentboard: { ...s.agentboard, notifyReviewRequested: v },
              }))
            }
          />
        ),
      },
      {
        label: "CI-failing notifications",
        keywords: ["notification", "desktop", "pr", "ci", "checks", "failing", "alert"],
        node: (
          <ToggleRow
            label="CI-failing notifications"
            description="Desktop notification when one of your PRs' checks flip to failing, while the app is unfocused."
            checked={settings.agentboard?.notifyChecksFailed ?? true}
            onCheckedChange={(v) =>
              update((s) => ({
                ...s,
                agentboard: { ...s.agentboard, notifyChecksFailed: v },
              }))
            }
          />
        ),
      },
      {
        label: "Stale-collector notifications",
        keywords: ["notification", "desktop", "collector", "stale", "health", "alert"],
        node: (
          <ToggleRow
            label="Stale-collector notifications"
            description="Desktop notification when a collector stops refreshing or keeps failing (expired gh auth, revoked Slack token)."
            checked={settings.agentboard?.notifyStaleCollector ?? true}
            onCheckedChange={(v) =>
              update((s) => ({
                ...s,
                agentboard: { ...s.agentboard, notifyStaleCollector: v },
              }))
            }
          />
        ),
      },
      {
        label: "Compaction recommendation",
        keywords: ["context", "compact", "percent", "threshold", "session", "usage"],
        node: (
          <CadenceRow
            label="Compaction recommendation"
            description="Flag a session for compaction once its context usage exceeds this percentage."
            unit="%"
            value={settings.agentboard?.compactRecommendPercent ?? DEFAULT_COMPACT_RECOMMEND_PERCENT}
            onValue={(n) =>
              update((s) => ({
                ...s,
                agentboard: {
                  ...s.agentboard,
                  compactRecommendPercent: Math.min(100, Math.max(1, n)),
                },
              }))
            }
          />
        ),
      },
      {
        label: "Copy on select",
        keywords: ["terminal", "clipboard", "selection", "copy"],
        node: (
          <ToggleRow
            label="Copy on select"
            description="Copy the terminal selection to the clipboard as soon as you finish selecting, without Ctrl/⌘+Shift+C."
            checked={settings.agentboard?.copyOnSelect ?? true}
            onCheckedChange={(v) =>
              update((s) => ({
                ...s,
                agentboard: { ...s.agentboard, copyOnSelect: v },
              }))
            }
          />
        ),
      },
      {
        label: "Terminal font size",
        keywords: ["terminal", "font", "size", "zoom", "text"],
        node: (
          <CadenceRow
            label="Terminal font size"
            description="Font size (px) for the app's terminals. Zoom in/out live with Ctrl/⌘ +/- (Ctrl/⌘ 0 resets)."
            unit="px"
            value={settings.agentboard?.terminalFontSize ?? DEFAULT_TERMINAL_FONT_SIZE}
            onValue={(n) =>
              update((s) => ({
                ...s,
                agentboard: { ...s.agentboard, terminalFontSize: clampTerminalFontSize(n) },
              }))
            }
          />
        ),
      },
      {
        label: "Shortcuts work in terminal",
        keywords: ["shortcut", "keyboard", "terminal", "focus", "hotkey", "jump", "needs you"],
        node: (
          <ToggleRow
            label="Shortcuts work in terminal"
            description="Board-wide shortcuts (jump to next/prev session needing you, close/split session, toggle diff/rail) fire even while a terminal has focus, instead of being sent to the shell."
            checked={settings.agentboard?.shortcutsWorkInTerminal ?? true}
            onCheckedChange={(v) =>
              update((s) => ({
                ...s,
                agentboard: { ...s.agentboard, shortcutsWorkInTerminal: v },
              }))
            }
          />
        ),
      },
    );
  }
  return [{ rows }];
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

/** Shortcuts list, filtered by the same predicate (description + when + scope). */
function ShortcutsList({ query }: { query: string }) {
  const empty = isEmptyQuery(query);
  const rows = Object.values(SHORTCUTS).filter((s) =>
    empty
      ? true
      : matchesFilter(query, s.description, [
          s.when ?? "",
          SCOPE_LABELS[s.scope],
          ...shortcutKeys(s.id),
        ]),
  );
  if (rows.length === 0) return <NoMatches query={query} />;
  return (
    <div className="flex flex-col">
      {rows.map((s, i) => (
        <div
          key={s.id}
          className={`flex items-center justify-between py-2 ${
            i > 0 ? "border-t border-border" : ""
          }`}
        >
          <span className="text-sm text-muted-foreground">
            {s.description}
            {s.when && (
              <span className="text-muted-foreground/70"> — {s.when}</span>
            )}
            {s.scope !== "global" && (
              <span className="ml-2 text-xs text-muted-foreground/70">
                ({SCOPE_LABELS[s.scope]})
              </span>
            )}
          </span>
          <KbdGroup>
            {shortcutKeys(s.id).map((cap) => (
              <Kbd key={cap}>{cap}</Kbd>
            ))}
          </KbdGroup>
        </div>
      ))}
    </div>
  );
}

/** About tab: static facts, filtered so the whole window stays consistent. */
function AboutInfo({ query, version }: { query: string; version: string }) {
  const empty = isEmptyQuery(query);
  const rows: { label: string; keywords: string[]; node: React.ReactNode }[] = [
    {
      label: "Version",
      keywords: ["about", "app"],
      node: (
        <div className="flex justify-between">
          <span className="text-muted-foreground">Version</span>
          <span className="font-mono">{version}</span>
        </div>
      ),
    },
    {
      label: "Identifier",
      keywords: ["about", "bundle", "id"],
      node: (
        <div className="flex justify-between">
          <span className="text-muted-foreground">Identifier</span>
          <span className="font-mono">dev.towles.tool</span>
        </div>
      ),
    },
    {
      label: "Settings file",
      keywords: ["about", "path", "config", "json"],
      node: (
        <div className="flex flex-col gap-1">
          <span className="text-muted-foreground">Settings file</span>
          <span className="font-mono text-xs break-all">{SETTINGS_PATH}</span>
        </div>
      ),
    },
  ];
  const visible = empty
    ? rows
    : rows.filter((r) => matchesFilter(query, r.label, r.keywords));
  if (visible.length === 0) return <NoMatches query={query} />;
  return (
    <>
      <div className="flex flex-col gap-2 text-sm">
        {visible.map((r) => (
          <Fragment key={r.label}>{r.node}</Fragment>
        ))}
      </div>
      {empty && (
        <p className="text-sm text-muted-foreground">
          Shared with the TypeScript CLI. The General, Journal, and Collectors
          tabs read and write it directly; unknown keys the CLI owns are
          preserved on save. Theme and Agentboard scan roots persist separately.
        </p>
      )}
    </>
  );
}

/** The tab + prefilled filter the window was deep-linked to (see `openSettings`),
 * read once from the URL. An unknown tab falls back to General. */
function initialTarget(): { tab: string; filter: string } {
  const fallback = { tab: "general", filter: "" };
  if (typeof window === "undefined") return fallback;
  const params = new URLSearchParams(window.location.search);
  const tab = params.get("tab") ?? "";
  const known = TABS.some((t) => t.id === tab);
  return { tab: known ? tab : "general", filter: params.get("filter") ?? "" };
}

export function SettingsWindow() {
  const { theme, setTheme, colorTheme, setColorTheme } = useTheme();
  const { settings, saveState, update, save } = useUserSettings();
  const version = useAppVersion();
  const [target] = useState(initialTarget);
  const [tab, setTab] = useState(target.tab);
  const [query, setQuery] = useState(target.filter);
  const filterRef = useRef<HTMLInputElement>(null);

  // Escape clears the filter first; a second Escape (empty box) closes the window.
  const onFilterKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key !== "Escape") return;
    e.preventDefault();
    if (isEmptyQuery(query)) {
      void closeCurrentWindow();
    } else {
      setQuery("");
    }
  };

  const collectorsPrelude = (
    <div className="rounded-md border border-dashed p-3 text-xs text-muted-foreground">
      Changes take effect as soon as you Save — the scheduler re-reads its
      cadence live.
    </div>
  );

  return (
    <div className="flex h-screen flex-col bg-background text-foreground">
      <header className="flex items-center gap-3 border-b border-border bg-card px-4 py-3">
        <h1 className="font-heading text-sm font-semibold">Settings</h1>
        <div className="relative ml-auto w-56">
          <Search className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground" />
          <Input
            ref={filterRef}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={onFilterKeyDown}
            placeholder="Filter settings…"
            className="pl-7"
            autoFocus
            spellCheck={false}
            aria-label="Filter settings"
          />
        </div>
      </header>

      <Tabs
        orientation="vertical"
        value={tab}
        onValueChange={setTab}
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
            {settings ? (
              <FilteredContent
                query={query}
                sections={generalSections(settings, update)}
              />
            ) : (
              <SettingsLoading />
            )}
          </TabsContent>

          <TabsContent value="appearance" className="flex flex-col gap-5 p-4">
            <TabHeading
              title="Appearance"
              note="Theme applies immediately across all windows."
            />
            <FilteredContent
              query={query}
              sections={appearanceSections(theme, setTheme, colorTheme, setColorTheme)}
            />
          </TabsContent>

          <TabsContent value="agentboard" className="flex flex-col gap-5 p-4">
            <TabHeading
              title="Agentboard"
              note="Repo discovery and needs-you notifications."
            />
            <FilteredContent
              query={query}
              sections={agentboardSections(settings, update)}
            />
          </TabsContent>

          <TabsContent value="journal" className="flex flex-col gap-5 p-4">
            <TabHeading
              title="Journal"
              note="Where notes live and how their file paths are templated."
            />
            {settings ? (
              <FilteredContent
                query={query}
                sections={journalSections(settings, update)}
              />
            ) : (
              <SettingsLoading />
            )}
          </TabsContent>

          <TabsContent value="collectors" className="flex flex-col gap-5 p-4">
            <TabHeading
              title="Collectors"
              note="Background jobs that fill the data hub. Each has an enable flag and cadence."
            />
            {settings ? (
              <FilteredContent
                query={query}
                sections={collectorsSections(settings, update)}
                prelude={collectorsPrelude}
              />
            ) : (
              <SettingsLoading />
            )}
          </TabsContent>

          <TabsContent value="shortcuts" className="flex flex-col gap-5 p-4">
            <TabHeading
              title="Shortcuts"
              note="Keyboard shortcuts (⌘ on macOS, Ctrl elsewhere). Agentboard-scoped ones only fire while that tab is active. Press ? in the app for the same list."
            />
            <ShortcutsList query={query} />
          </TabsContent>

          <TabsContent value="about" className="flex flex-col gap-5 p-4">
            <TabHeading title="About" note="Towles Tool desktop app." />
            <AboutInfo query={query} version={version} />
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
