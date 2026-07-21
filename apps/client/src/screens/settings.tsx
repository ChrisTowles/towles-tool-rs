import { Fragment, useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react";
import {
  Check,
  ChevronsUpDown,
  Eye,
  EyeOff,
  FolderGit2,
  FolderPlus,
  GripVertical,
  Info,
  Keyboard,
  NotebookPen,
  Palette,
  RefreshCw,
  Search,
  SlidersHorizontal,
} from "lucide-react";
import { toast } from "sonner";
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
import { Checkbox } from "@/components/ui/checkbox";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { COLOR_THEMES, useTheme, type ColorTheme, type Theme } from "@/components/theme-provider";
import { CollectorFreshness } from "@/components/store-bits";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { useAgentboardState, type RepoCandidate, type RepoData } from "@/lib/agentboard";
import {
  applyRepoOrder,
  orderSettled,
  reorderDirs,
  sameOrder,
  showAddPath,
  untrackedCandidates,
} from "@/lib/repo-manager";
import {
  hasRepoColor,
  normalizeHex,
  repoAccentStyles,
  repoIcon,
  REPO_ICONS,
  REPO_PALETTE,
  type RepoIdentityStyle,
  type RepoMeta,
} from "@/lib/repo-identity";
import { liveSessionIds, trackRepo, untrackRepo } from "@/lib/repo-actions";
import { uiAction } from "@/lib/ui-action";
import { invoke } from "@/lib/tauri";
import { storeCollectNow, useStoreSnapshot, type CollectRun } from "@/lib/data";
import { useNow } from "@/lib/now";
import { isEmptyQuery, matchesFilter } from "@/lib/settings-filter";
import { settingsTargetStore, type SettingsTarget } from "@/lib/settings-target";
import { NotInTauri } from "@/lib/errors";
import { slackListUsers, type SlackUser } from "@/lib/slack";
import {
  nextCalendarSourceId,
  nextPromptImproverId,
  useUserSettings,
  type CalendarSource,
  type PromptImprover,
  type SaveState,
  type UserSettings,
} from "@/lib/settings";
import { PromptTemplateList } from "@/components/prompt-template-list";
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
  board: "Board",
};

/** Toggle/select row: label + description on the left, control on the right.
 * `extra` renders between the description and `children` (e.g. a freshness
 * badge ahead of a collector's enable switch). */
function SettingRow({
  label,
  description,
  extra,
  children,
}: {
  label: string;
  description: string;
  extra?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-center justify-between gap-4">
      <div>
        <div className="text-sm font-medium">{label}</div>
        <div className="text-sm text-muted-foreground">{description}</div>
      </div>
      <div className="flex items-center gap-3">
        {extra}
        {children}
      </div>
    </div>
  );
}

/** Tab heading: title + note on the left, an optional action (e.g. a
 * "Refresh now" button) top-right. */
function TabHeading({
  title,
  note,
  action,
}: {
  title: string;
  note: string;
  action?: React.ReactNode;
}) {
  return (
    <div className="flex items-start justify-between gap-3">
      <div className="flex flex-col gap-1">
        <h2 className="text-sm font-semibold">{title}</h2>
        <p className="text-sm text-muted-foreground">{note}</p>
      </div>
      {action}
    </div>
  );
}

/** `defer` debounces the write — see `useUserSettings`. Set it for anything the
 * user types into; leave it off for toggles, selects, and one-click choices. */
type Update = (fn: (prev: UserSettings) => UserSettings, opts?: { defer?: boolean }) => void;

/** Commits a pending deferred write immediately — wired to the blur of every
 * input that defers, so tabbing out of a field saves it. */
type Flush = () => Promise<void>;

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
  extra,
}: {
  label: string;
  description: string;
  checked: boolean;
  onCheckedChange: (v: boolean) => void;
  extra?: React.ReactNode;
}) {
  return (
    <SettingRow label={label} description={description} extra={extra}>
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
  onCommit,
}: {
  label: string;
  description: string;
  value: number;
  unit: string;
  onValue: (n: number) => void;
  /** Commit the debounced write now (blur) rather than waiting out the delay. */
  onCommit?: () => void;
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
          onBlur={onCommit}
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
  onCommit,
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  /** Commit the debounced write now (blur) rather than waiting out the delay. */
  onCommit?: () => void;
}) {
  const [shown, setShown] = useState(false);
  return (
    <div className="relative">
      <Input
        type={shown ? "text" : "password"}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onBlur={onCommit}
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
  onIdCommit,
}: {
  userId: string;
  userName: string;
  onPick: (user: SlackUser) => void;
  onIdChange: (id: string) => void;
  /** Commits the debounced write behind the typed-member-id fallback below. */
  onIdCommit?: () => void;
}) {
  const [users, setUsers] = useState<SlackUser[] | null>(null);
  const [failed, setFailed] = useState(false);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    let alive = true;
    void slackListUsers().then((listed) => {
      if (!alive) return;
      listed.match({ ok: setUsers, err: () => setFailed(true) });
    });
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
        onBlur={onIdCommit}
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
        : section.rows.filter((row) => matchesFilter(query, row.label, rowKeywords(section, row))),
    }))
    .filter((entry) => entry.rows.length > 0);

  if (visible.length === 0) return <NoMatches query={query} />;

  return (
    <>
      {empty && prelude}
      {visible.map(({ section, rows }, i) => (
        <section key={section.heading ?? i} className="flex flex-col gap-4">
          {section.heading && <h3 className="text-sm font-semibold">{section.heading}</h3>}
          {rows.map((row) => (
            <Fragment key={row.label}>{row.node}</Fragment>
          ))}
        </section>
      ))}
    </>
  );
}

function generalSections(settings: UserSettings, update: Update, flush: Flush): FilterSection[] {
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

function journalSections(settings: UserSettings, update: Update, flush: Flush): FilterSection[] {
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

/**
 * Editor for the calendar collector's per-source list: one card per calendar,
 * each with its own enable switch, label, and `claude -p` prompt.
 *
 * The prompt is a plain textarea rather than a provider picker on purpose. The
 * built-in prompts drive a Google/Outlook MCP, which isn't necessarily
 * configured on this machine — the escape hatch is pointing a source at
 * whatever does work here (a CLI like `gws`, a script, another MCP), so long as
 * it answers with the documented JSON array. Each source writes into its own
 * store lane keyed by `id`, so a second calendar never displaces the first;
 * that's why ids are assigned once at creation and shown read-only.
 */
function CalendarSourcesEditor({
  sources,
  onChange,
  onCommit,
}: {
  sources: CalendarSource[];
  onChange: (sources: CalendarSource[], opts?: { defer?: boolean }) => void;
  /** Commits the debounced write behind the typed label/prompt fields. */
  onCommit?: () => void;
}) {
  return (
    <PromptTemplateList
      items={sources}
      onChange={onChange}
      onCommit={onCommit}
      onAdd={() => {
        const label = `Calendar ${sources.length + 1}`;
        onChange([
          ...sources,
          { id: nextCalendarSourceId(sources, label), label, enabled: false, prompt: "" },
        ]);
        uiAction("calendar.source_added", "settings");
      }}
      onRemove={(index) => {
        const removed = sources[index];
        onChange(sources.filter((_, i) => i !== index));
        uiAction("calendar.source_removed", "settings", removed?.id);
      }}
      heading="Calendars"
      description={
        <>
          Each enabled calendar is pulled with its own <code>claude -p</code> prompt and stored
          separately, so several calendars merge into one timeline. The prompt must answer with only
          a JSON array of{" "}
          <code>{"{externalId, title, start, end, attendees, location, joinUrl}"}</code>, or{" "}
          <code>[]</code>. Times are RFC 3339 with the calendar's own UTC offset (
          <code>2026-07-20T15:00:00-05:00</code>) — keep the offset rather than converting, so a
          meeting booked as 3pm somewhere still reads that way.
        </>
      }
      addLabel="Add calendar"
      emptyText="No calendars configured — the collector has nothing to pull."
      idTitle="Store lane id"
      enableVerb={(s) => `Pull ${s.label || s.id}`}
      promptPlaceholder="Prompt claude with how to list today's events for this calendar, and what JSON to answer with."
      rowWarning={(s) =>
        s.enabled && s.prompt.trim() === ""
          ? "This calendar is on but has no prompt — it will be reported as a failed run until you write one."
          : null
      }
    />
  );
}

/** A prompt improver's "Preferred" toggle — whether it gets its own button in
 * the new-task form or sits under that form's "More" menu. */
function PreferredToggle({
  item,
  patch,
}: {
  item: PromptImprover;
  patch: (next: Partial<PromptImprover>) => void;
}) {
  return (
    <label className="flex cursor-pointer items-center gap-1.5 text-xs text-muted-foreground">
      <Checkbox
        checked={item.preferred}
        onCheckedChange={(v) => patch({ preferred: v === true })}
        aria-label={`Give ${item.label || item.id} its own button`}
      />
      Preferred
    </label>
  );
}

/**
 * Editor for the new-task form's prompt improvers (Direct / Plan / Brainstorm by
 * default) — the buttons that rewrite the goal you typed before the task starts.
 * Each improver's prompt is the *instruction* handed to `claude -p`, which fills
 * the form's goal + branch fields with the rewrite. Uses the same list editor as
 * the calendar sources, plus a per-row "Preferred" toggle deciding which get
 * their own button vs. sitting under the form's "More" menu.
 */
function PromptImproversEditor({
  improvers,
  onChange,
  onCommit,
}: {
  improvers: PromptImprover[];
  onChange: (improvers: PromptImprover[], opts?: { defer?: boolean }) => void;
  onCommit?: () => void;
}) {
  return (
    <PromptTemplateList
      items={improvers}
      onChange={onChange}
      onCommit={onCommit}
      onAdd={() => {
        const label = `Improver ${improvers.length + 1}`;
        onChange([
          ...improvers,
          {
            id: nextPromptImproverId(improvers, label),
            label,
            enabled: true,
            preferred: true,
            prompt: "",
          },
        ]);
        uiAction("prompt_improver.added", "settings");
      }}
      onRemove={(index) => {
        const removed = improvers[index];
        onChange(improvers.filter((_, i) => i !== index));
        uiAction("prompt_improver.removed", "settings", removed?.id);
      }}
      heading="Prompt improvers"
      description={
        <>
          The buttons above the branch field in the new-task form. Clicking one runs{" "}
          <code>claude -p</code> with its prompt as the <em>instruction</em> for how to rewrite the
          goal you typed, then fills the goal and branch fields with the result — editable, and Undo
          puts back what was there. So the prompt is an instruction <em>about</em> the task ("turn
          this into a request for a plan"), not a template containing it. <strong>Preferred</strong>{" "}
          improvers get their own button; the rest sit under “More”. Nothing here changes the model,
          effort, or permission mode.
        </>
      }
      addLabel="Add prompt improver"
      emptyText="No prompt improvers — the new-task form falls back to a single “Suggest name + goal” button."
      idTitle="Prompt-improver id"
      enableVerb={(t) => `Offer ${t.label || t.id}`}
      promptPlaceholder="e.g. Rewrite the task as a request for an implementation plan; research first, don't edit any files yet."
      rowExtra={PreferredToggle}
      rowWarning={(t) =>
        t.enabled && t.prompt.trim() === ""
          ? "No instruction — this button falls back to the built-in “restate it in one sentence”."
          : null
      }
    />
  );
}

/** Small, disable-while-running "Refresh now" button — forces the
 * issues/PRs/Slack collectors to run immediately instead of waiting for their
 * next scheduled tick (calendar stays on its cadence — it costs tokens). */
function RefreshNowButton() {
  const [running, setRunning] = useState(false);
  const refresh = async () => {
    if (running) return;
    setRunning(true);
    const started = await storeCollectNow();
    if (started.isErr() && !NotInTauri.is(started.error)) toast.error(started.error.message);
    setRunning(false);
  };
  return (
    <Button variant="outline" size="sm" disabled={running} onClick={() => void refresh()}>
      <RefreshCw className={running ? "size-3.5 animate-spin" : "size-3.5"} />
      {running ? "Refreshing…" : "Refresh now"}
    </Button>
  );
}

function collectorsSections(
  settings: UserSettings,
  update: Update,
  run: (key: string) => CollectRun | undefined,
  now: number,
  flush: Flush,
): FilterSection[] {
  const c = settings.collectors;
  // `typed` marks a patch as coming from a keystroke, so the write debounces
  // instead of handing the scheduler a partial cadence or half-pasted token.
  const typed = { defer: true };
  const setCollector = <K extends keyof UserSettings["collectors"]>(
    key: K,
    patch: Partial<UserSettings["collectors"][K]>,
    opts?: { defer?: boolean },
  ) =>
    update(
      (s) => ({
        ...s,
        collectors: {
          ...s.collectors,
          [key]: { ...s.collectors[key], ...patch },
        },
      }),
      opts,
    );
  const setCal = (
    patch: Partial<UserSettings["collectors"]["calendar"]>,
    opts?: { defer?: boolean },
  ) => setCollector("calendar", patch, opts);
  const setCalQuiet = (
    patch: Partial<UserSettings["collectors"]["calendar"]["quietHours"]>,
    opts?: { defer?: boolean },
  ) => setCal({ quietHours: { ...c.calendar.quietHours, ...patch } }, opts);
  const setPrs = (patch: Partial<UserSettings["collectors"]["prs"]>, opts?: { defer?: boolean }) =>
    setCollector("prs", patch, opts);
  const setIssues = (
    patch: Partial<UserSettings["collectors"]["issues"]>,
    opts?: { defer?: boolean },
  ) => setCollector("issues", patch, opts);
  const setSlack = (
    patch: Partial<UserSettings["collectors"]["slack"]>,
    opts?: { defer?: boolean },
  ) => setCollector("slack", patch, opts);

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
              extra={<CollectorFreshness run={run("claude:calendar")} now={now} />}
            />
          ),
        },
        {
          label: "Calendars",
          keywords: ["google", "outlook", "mcp", "prompt", "source", "gws", "sources"],
          node: (
            <CalendarSourcesEditor
              sources={c.calendar.sources}
              onChange={(sources, opts) => setCal({ sources }, opts)}
              onCommit={() => void flush()}
            />
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
              onValue={(n) => setCal({ refreshMinutes: n }, typed)}
              onCommit={() => void flush()}
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
                  onChange={(e) => setCalQuiet({ startHour: clampHour(e.target.value) }, typed)}
                  onBlur={() => void flush()}
                  disabled={!c.calendar.quietHours.enabled}
                  className="w-16"
                />
                <span className="text-sm text-muted-foreground">to</span>
                <Input
                  type="number"
                  min={0}
                  max={23}
                  value={c.calendar.quietHours.endHour}
                  onChange={(e) => setCalQuiet({ endHour: clampHour(e.target.value) }, typed)}
                  onBlur={() => void flush()}
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
            <FieldRow label="Active days" description="Weekdays the collector may run.">
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
              extra={<CollectorFreshness run={run("prs")} now={now} />}
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
              onValue={(n) => setPrs({ refreshSeconds: n }, typed)}
              onCommit={() => void flush()}
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
              extra={<CollectorFreshness run={run("issues")} now={now} />}
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
              onValue={(n) => setIssues({ refreshMinutes: n }, typed)}
              onCommit={() => void flush()}
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
                onChange={(v) => setSlack({ token: v }, typed)}
                onCommit={() => void flush()}
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
                onChange={(v) => setSlack({ appToken: v }, typed)}
                onCommit={() => void flush()}
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
                onIdChange={(id) => setSlack({ watchUserId: id }, typed)}
                onIdCommit={() => void flush()}
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
                onChange={(e) => setSlack({ watchName: e.target.value }, typed)}
                onBlur={() => void flush()}
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
              onValue={(n) => setSlack({ refreshSeconds: n }, typed)}
              onCommit={() => void flush()}
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
  flush: Flush,
): FilterSection[] {
  const rows: FilterRow[] = [
    {
      label: "Scan roots",
      keywords: ["repo", "discovery", "directory", "picker", "add repo"],
      node: <AgentboardSettings />,
    },
    {
      label: "Repos",
      keywords: [
        "repo",
        "track",
        "untrack",
        "add repo",
        "remove",
        "rail",
        "order",
        "reorder",
        "icon",
        "color",
        "tint",
      ],
      node: <RepoManager />,
    },
  ];
  if (settings) {
    rows.push(
      {
        label: "Prompt improvers",
        keywords: [
          "prompt",
          "improver",
          "improve",
          "goal",
          "plan",
          "brainstorm",
          "template",
          "new task",
        ],
        node: (
          <PromptImproversEditor
            improvers={settings.promptImprovers ?? []}
            onChange={(improvers, opts) =>
              update((s) => ({ ...s, promptImprovers: improvers }), opts)
            }
            onCommit={() => void flush()}
          />
        ),
      },
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
            value={
              settings.agentboard?.compactRecommendPercent ?? DEFAULT_COMPACT_RECOMMEND_PERCENT
            }
            onValue={(n) =>
              update(
                (s) => ({
                  ...s,
                  agentboard: {
                    ...s.agentboard,
                    compactRecommendPercent: Math.min(100, Math.max(1, n)),
                  },
                }),
                { defer: true },
              )
            }
            onCommit={() => void flush()}
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
              update(
                (s) => ({
                  ...s,
                  agentboard: { ...s.agentboard, terminalFontSize: clampTerminalFontSize(n) },
                }),
                { defer: true },
              )
            }
            onCommit={() => void flush()}
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
 * Scan-root editor for repo discovery. Reads/writes `scanRoots`
 * in `~/.config/towles-tool/agentboard/repos.json` over the `ab_*` Tauri
 * commands (no shared settings file, no zod — pure Rust round-trip). One root
 * per line; empty falls back to `~/code`.
 */
function AgentboardSettings() {
  const [roots, setRoots] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pending = useRef<string | null>(null);

  useEffect(() => {
    void invoke<string[]>("ab_get_scan_roots").then((r) => setRoots(r.unwrapOr([]).join("\n")));
  }, []);

  // Autosave, like the rest of this screen. Deliberately does *not* write the
  // normalized list back into the textarea: this fires mid-typing, and replacing
  // the value would eat the blank line you just opened and jump the cursor.
  const persist = useCallback(async () => {
    if (timer.current !== null) {
      clearTimeout(timer.current);
      timer.current = null;
    }
    const raw = pending.current;
    if (raw === null) return;
    pending.current = null;
    const list = raw
      .split("\n")
      .map((s) => s.trim())
      .filter(Boolean);
    const stored = await invoke("ab_set_scan_roots", { roots: list });
    if (stored.isErr()) {
      if (!NotInTauri.is(stored.error)) {
        toast.error(`Couldn't save scan roots — ${stored.error.message}`);
      }
      return;
    }
    setSaved(true);
    window.setTimeout(() => setSaved(false), 1500);
  }, []);

  const edit = (next: string) => {
    setRoots(next);
    pending.current = next;
    if (timer.current !== null) clearTimeout(timer.current);
    timer.current = setTimeout(() => void persist(), 600);
  };

  // Commit a pending edit if the pane unmounts (Radix drops it on tab switch).
  const persistRef = useRef(persist);
  persistRef.current = persist;
  useEffect(
    () => () => {
      void persistRef.current();
    },
    [],
  );

  if (roots === null) {
    return <div className="text-sm text-muted-foreground">Loading…</div>;
  }

  return (
    <div className="flex flex-col gap-3">
      <div>
        <div className="text-sm font-medium">Scan roots</div>
        <p className="text-sm text-muted-foreground">
          One directory per line. The repo list below scans these for git repos. Leave empty to use{" "}
          <span className="font-mono">~/code</span>. A leading <span className="font-mono">~</span>{" "}
          expands to your home directory.
        </p>
      </div>
      <Textarea
        value={roots}
        onChange={(e) => edit(e.target.value)}
        onBlur={() => void persist()}
        rows={5}
        placeholder="~/code"
        className="font-mono text-xs"
        spellCheck={false}
      />
      {saved && <span className="text-xs text-muted-foreground">Saved.</span>}
    </div>
  );
}

/**
 * The **one** place repos are managed. Track/untrack, drag to set the rail's
 * order, and give a repo its own glyph and color — all against the same
 * agentboard snapshot the rail renders. (There used to be a second surface, a
 * "Manage repos" command dialog on the Agentboard screen; it was deleted, and
 * its rail button now deep-links here.)
 *
 * Identity is only offered for *tracked* repos: a discovered-but-untracked
 * candidate has nowhere to render an icon, so those rows carry a Track action
 * and nothing else rather than dead controls.
 */
function RepoManager() {
  const { repos } = useAgentboardState();
  const [candidates, setCandidates] = useState<RepoCandidate[]>([]);
  const [query, setQuery] = useState("");
  const [confirm, setConfirm] = useState<{
    dir: string;
    name: string;
    /** Live session ids closed on confirm — see `untrack`. */
    sessionIds: string[];
  } | null>(null);
  // Optimistic order, held only until a poll reports the same sequence — a
  // dropped row must not snap back for the length of the IPC round-trip.
  const [order, setOrder] = useState<string[] | null>(null);
  const [dragDir, setDragDir] = useState<string | null>(null);
  const [dropBefore, setDropBefore] = useState<string | null>(null);

  const refresh = async () => {
    setCandidates((await invoke<RepoCandidate[]>("ab_discover_repos")).unwrapOr([]));
  };

  // This pane only exists while the Agentboard tab is the selected one (Radix
  // unmounts the other panes), so a mount is exactly "the tab was shown".
  useEffect(() => {
    void refresh();
  }, []);

  const snapshotDirs = repos.map((r) => r.dir);
  const ordered = applyRepoOrder(repos, order);
  const trackedDirs = new Set(snapshotDirs);
  // Drop the optimistic overlay once the snapshot reflects the drag.
  const settled = orderSettled(order, snapshotDirs);
  useEffect(() => {
    if (settled) setOrder(null);
  }, [settled]);

  const visibleRepos = ordered.filter((r) => matchesFilter(query, r.name, [r.dir]));
  const visibleCandidates = untrackedCandidates(candidates, trackedDirs).filter((c) =>
    matchesFilter(query, c.name, [c.dir]),
  );

  const track = async (path: string) => {
    if (await trackRepo(path, "settings")) await refresh();
  };

  const untrack = async (dir: string, name: string, sessionIds: string[] = []) => {
    if (await untrackRepo(dir, name, sessionIds, "settings")) await refresh();
  };

  // Untracking a repo whose sessions are still running stops them, so that
  // case confirms first (same guard the deleted dialog carried).
  const requestUntrack = (repo: RepoData) => {
    const liveIds = liveSessionIds(repo);
    if (liveIds.length === 0) {
      void untrack(repo.dir, repo.name);
      return;
    }
    setConfirm({ dir: repo.dir, name: repo.name, sessionIds: liveIds });
  };

  const drop = (beforeDir: string | "end") => {
    const dragged = dragDir;
    setDragDir(null);
    setDropBefore(null);
    if (!dragged) return;
    const current = ordered.map((r) => r.dir);
    const next = reorderDirs(current, dragged, beforeDir);
    if (sameOrder(current, next)) return;
    setOrder(next);
    uiAction("repo.reordered", "settings");
    void invoke("ab_set_repo_order", { dirs: next }).then((res) => {
      if (res.isErr() && !NotInTauri.is(res.error)) {
        toast.error(`Couldn't save the repo order — ${res.error.message}`);
        setOrder(null);
      }
    });
  };

  return (
    // A bottom rule + generous gap: this block ends in a list of rows, and the
    // settings rows that follow look just like them without a hard break.
    <div className="flex flex-col gap-3 border-b border-border pb-5">
      <div>
        <div className="text-sm font-medium">Repos</div>
        <p className="text-sm text-muted-foreground">
          Everything about the rail's repo list lives here: which repos are tracked, the order they
          sit in (drag a row), and each one's glyph and color so you can pick it out — especially in
          the collapsed icon strip — without reading names. Identity is decoration only: it never
          changes a status signal, and a repo waiting on you still shows amber.
        </p>
      </div>

      <Input
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        placeholder="Search repos, or type an absolute path…"
        spellCheck={false}
        aria-label="Search repos"
      />

      {repos.length === 0 && (
        <p className="text-sm text-muted-foreground/70">
          No repos tracked yet — track one from the list below.
        </p>
      )}

      <section aria-label="Tracked repos" className="flex flex-col gap-1">
        <h4 className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
          On the rail
        </h4>
        <div className="flex flex-col overflow-hidden rounded-md border border-border">
          {visibleRepos.map((repo) => (
            <RepoIdentityRow
              key={repo.key}
              repo={repo}
              dragging={dragDir === repo.dir}
              dropTarget={dropBefore === repo.dir}
              onDragStart={() => setDragDir(repo.dir)}
              onDragOverRow={() => setDropBefore(repo.dir)}
              onDropRow={() => drop(repo.dir)}
              onDragEnd={() => {
                setDragDir(null);
                setDropBefore(null);
              }}
              onUntrack={() => requestUntrack(repo)}
            />
          ))}
          {dragDir && (
            <div
              onDragOver={(e) => {
                e.preventDefault();
                setDropBefore(null);
              }}
              onDrop={(e) => {
                e.preventDefault();
                drop("end");
              }}
              className="m-1 h-6 rounded-md border border-dashed border-border/70"
            />
          )}
        </div>
      </section>

      {showAddPath(query, candidates, trackedDirs) && (
        <Button
          variant="outline"
          size="sm"
          className="self-start"
          onClick={() => void track(query.trim())}
        >
          <FolderPlus className="size-3.5" /> Add path {query.trim()}
        </Button>
      )}

      {visibleCandidates.length > 0 && (
        <section aria-label="Repos not tracked" className="flex flex-col gap-1">
          <h4 className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
            Found under your scan roots ({visibleCandidates.length})
          </h4>
          <p className="text-xs text-muted-foreground/70">
            Not on the rail. Track one to give it a glyph, a color, and a place in the order — or
            search above to narrow this list.
          </p>
          {/* Filled + bordered so this list reads as its own block: it sits
              between the tracked list and the notification settings below,
              and without containment its rows look like more settings. */}
          <div className="flex max-h-64 flex-col overflow-y-auto rounded-md border border-dashed border-border bg-muted/30">
            {visibleCandidates.map((c) => (
              <div
                key={c.dir}
                className="flex items-center gap-3 border-t border-border/60 px-2 py-2 first:border-t-0"
              >
                <FolderGit2 aria-hidden className="size-4 shrink-0 text-muted-foreground" />
                <div className="flex min-w-0 flex-1 flex-col">
                  <span className="truncate text-sm">{c.name}</span>
                  <span className="truncate font-mono text-xs text-muted-foreground">{c.dir}</span>
                </div>
                <Button
                  variant="outline"
                  size="sm"
                  className="h-7 px-2 text-xs"
                  onClick={() => void track(c.dir)}
                >
                  Track
                </Button>
              </div>
            ))}
          </div>
        </section>
      )}

      <AlertDialog open={confirm !== null} onOpenChange={(open) => !open && setConfirm(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Untrack {confirm?.name} from the rail?</AlertDialogTitle>
            <AlertDialogDescription>
              {confirm?.sessionIds.length}{" "}
              {confirm?.sessionIds.length === 1 ? "session is" : "sessions are"} still running.
              Untracking will stop {confirm?.sessionIds.length === 1 ? "it" : "them"}.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                if (confirm) void untrack(confirm.dir, confirm.name, confirm.sessionIds);
                setConfirm(null);
              }}
            >
              Stop &amp; untrack
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

function RepoIdentityRow({
  repo,
  dragging,
  dropTarget,
  onDragStart,
  onDragOverRow,
  onDropRow,
  onDragEnd,
  onUntrack,
}: {
  repo: RepoData;
  dragging: boolean;
  dropTarget: boolean;
  onDragStart: () => void;
  onDragOverRow: () => void;
  onDropRow: () => void;
  onDragEnd: () => void;
  onUntrack: () => void;
}) {
  // Local state is the truth once you have edited: the agentboard snapshot that
  // seeded it arrives on a poll, and re-syncing from it mid-edit would fight
  // the user's own clicks.
  const [meta, setMeta] = useState<RepoMeta | undefined>(repo.meta);
  const [hex, setHex] = useState(repo.meta?.color ?? "");
  const [hexError, setHexError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const dir = repo.dir;
  const Icon = repoIcon(meta);
  const accent = repoAccentStyles(meta);
  // The latest edit, readable synchronously. `meta` state lags an in-flight
  // commit by a full await, and `ab_set_repo_meta` replaces the identity
  // *wholesale* — so building the next edit off the render closure would let a
  // second click (pick an icon, then flick Tint before the first IPC lands)
  // send a payload missing the first field and silently erase it.
  const latest = useRef<RepoMeta | undefined>(repo.meta);

  const commit = async (next: RepoMeta | null, action: string, detail?: string) => {
    latest.current = next ?? undefined;
    const res = await invoke("ab_set_repo_meta", {
      dir,
      icon: next?.icon ?? null,
      color: next?.color ?? null,
      style: next?.style ?? null,
    });
    if (res.isErr()) {
      if (!NotInTauri.is(res.error)) {
        toast.error(`Couldn't save ${repo.name} — ${res.error.message}`);
      }
      // Roll the optimistic ref back so the next edit doesn't build on a value
      // the backend rejected.
      latest.current = meta;
      return;
    }
    setMeta(next ?? undefined);
    uiAction(action, "settings", detail);
    setSaved(true);
    window.setTimeout(() => setSaved(false), 1500);
  };

  const setIcon = (name: string) =>
    void commit({ ...latest.current, icon: name }, "repo.icon_set", name);
  const setColor = (raw: string, detail: string) => {
    // Rust stores a malformed color as null, which would silently blank the
    // repo — so a bad value never leaves the client.
    const canonical = normalizeHex(raw);
    if (!canonical) {
      setHexError("Use #rgb or #rrggbb");
      return;
    }
    setHexError(null);
    setHex(canonical);
    void commit({ ...latest.current, color: canonical }, "repo.color_set", detail);
  };

  // Autosave the typed hex, like every other control on this screen. A partial
  // value is *silently* ignored rather than reported: this runs while you're
  // still typing, and "#3b" is half-finished, not wrong. The error surfaces on
  // blur (below) and on Enter, where the input really is final.
  const hexTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const editHex = (raw: string) => {
    setHex(raw);
    setHexError(null);
    if (hexTimer.current !== null) clearTimeout(hexTimer.current);
    hexTimer.current = setTimeout(() => {
      if (normalizeHex(raw)) setColor(raw, "hex");
    }, 600);
  };
  const commitHex = (detail: string) => {
    if (hexTimer.current !== null) {
      clearTimeout(hexTimer.current);
      hexTimer.current = null;
    }
    // Leaving the field empty isn't an error — it just means "no custom color".
    if (hex.trim() === "") {
      setHexError(null);
      return;
    }
    setColor(hex, detail);
  };
  useEffect(
    () => () => {
      if (hexTimer.current !== null) clearTimeout(hexTimer.current);
    },
    [],
  );
  const setStyle = (tint: boolean) => {
    const style: RepoIdentityStyle = tint ? "tint" : "accent";
    void commit({ ...latest.current, style }, "repo.style_set", style);
  };
  const reset = () => {
    setHex("");
    setHexError(null);
    void commit(null, "repo.identity_reset");
  };

  return (
    <div
      onDragOver={(e) => {
        e.preventDefault();
        e.dataTransfer.dropEffect = "move";
        onDragOverRow();
      }}
      onDrop={(e) => {
        e.preventDefault();
        onDropRow();
      }}
      className={cn(
        "flex items-center gap-2 border-t border-border px-2 py-2 first:border-t-transparent",
        dragging && "opacity-50",
        dropTarget && "border-t-violet-500",
      )}
    >
      <span
        draggable
        onDragStart={(e) => {
          e.dataTransfer.effectAllowed = "move";
          // Firefox/WebKit refuse to start a drag with an empty payload.
          e.dataTransfer.setData("text/plain", dir);
          onDragStart();
        }}
        onDragEnd={onDragEnd}
        aria-label={`Reorder ${repo.name}`}
        title="Drag to reorder"
        className="shrink-0 cursor-grab text-muted-foreground active:cursor-grabbing"
      >
        <GripVertical className="size-4" />
      </span>
      <Icon
        aria-hidden
        className={cn("size-4 shrink-0", !hasRepoColor(meta) && "text-muted-foreground")}
        style={accent.iconStyle}
      />
      <div className="flex min-w-0 flex-1 flex-col">
        <span className="truncate text-sm">{repo.name}</span>
        <span className="truncate font-mono text-xs text-muted-foreground">{dir}</span>
      </div>
      {saved && <span className="shrink-0 text-xs text-muted-foreground">Saved.</span>}

      <Popover>
        <PopoverTrigger asChild>
          <Button variant="outline" size="sm" className="h-7 px-2 text-xs">
            Icon
          </Button>
        </PopoverTrigger>
        <PopoverContent className="w-56 p-2" align="end">
          <div className="grid grid-cols-8 gap-1">
            {Object.entries(REPO_ICONS).map(([name, Choice]) => (
              <button
                key={name}
                type="button"
                title={name}
                aria-label={name}
                aria-pressed={meta?.icon === name}
                onClick={() => setIcon(name)}
                className={cn(
                  "flex size-6 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground",
                  meta?.icon === name && "bg-accent text-foreground",
                )}
              >
                <Choice className="size-3.5" style={accent.iconStyle} />
              </button>
            ))}
          </div>
        </PopoverContent>
      </Popover>

      <Popover
        onOpenChange={(open) => {
          // A rejected hex is only reported inside this popover, so a stale
          // error would greet you on reopen with nothing explaining it.
          if (!open) {
            setHexError(null);
            setHex(latest.current?.color ?? "");
          }
        }}
      >
        <PopoverTrigger asChild>
          <Button variant="outline" size="sm" className="h-7 px-2 text-xs">
            Color
          </Button>
        </PopoverTrigger>
        <PopoverContent className="w-56 p-2" align="end">
          <div className="grid grid-cols-5 gap-1.5">
            {REPO_PALETTE.map((swatch) => (
              <button
                key={swatch}
                type="button"
                title={swatch}
                aria-label={swatch}
                aria-pressed={meta?.color === swatch}
                onClick={() => setColor(swatch, "palette")}
                style={{ backgroundColor: swatch }}
                className={cn(
                  "size-6 rounded-md border border-border",
                  meta?.color === swatch && "ring-2 ring-ring ring-offset-1 ring-offset-background",
                )}
              />
            ))}
          </div>
          <div className="mt-2 flex items-center gap-1.5">
            <Input
              value={hex}
              onChange={(e) => editHex(e.target.value)}
              onBlur={() => commitHex("hex")}
              onKeyDown={(e) => {
                if (e.key === "Enter") commitHex("hex");
              }}
              placeholder="#3b82f6"
              spellCheck={false}
              aria-label="Custom color"
              aria-invalid={hexError !== null}
              className="h-7 flex-1 font-mono text-xs"
            />
          </div>
          {hexError && <p className="mt-1 text-xs text-red-500">{hexError}</p>}
        </PopoverContent>
      </Popover>

      <label className="flex shrink-0 items-center gap-1.5 text-xs text-muted-foreground">
        <Switch
          checked={(meta?.style ?? "accent") === "tint"}
          onCheckedChange={setStyle}
          aria-label={`Tint the ${repo.name} row background`}
        />
        Tint
      </label>
      <Button variant="ghost" size="sm" className="h-7 px-2 text-xs" onClick={reset}>
        Reset
      </Button>
      <Button
        variant="ghost"
        size="sm"
        className="h-7 px-2 text-xs text-muted-foreground hover:text-foreground"
        onClick={onUntrack}
      >
        Untrack
      </Button>
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
            {s.when && <span className="text-muted-foreground/70"> — {s.when}</span>}
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

/** About tab: static facts, filtered so the whole screen stays consistent. */
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
  const visible = empty ? rows : rows.filter((r) => matchesFilter(query, r.label, r.keywords));
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
          Shared with the TypeScript CLI. The General, Journal, and Collectors tabs read and write
          it directly; unknown keys the CLI owns are preserved on save. Theme and Agentboard scan
          roots persist separately.
        </p>
      )}
    </>
  );
}

/** The tab + prefilled filter this screen was deep-linked to (see
 * `useWorkspace().openSettingsTab`), consumed once from `settingsTargetStore`.
 * An unknown tab falls back to General. */
function resolveTarget(target: SettingsTarget | null): {
  tab: string;
  filter: string;
} {
  const fallback = { tab: "general", filter: "" };
  if (!target) return fallback;
  const known = TABS.some((t) => t.id === target.tab);
  return { tab: known ? target.tab : "general", filter: target.filter ?? "" };
}

/**
 * Footer readout for the autosave. With no Save button there's nothing for the
 * user to retry, so a failure has to stay on screen and say plainly that the
 * change didn't land — the idle/saved states can be quiet, this one can't.
 */
function SaveStatus({ state }: { state: SaveState }) {
  if (state === "error") {
    return (
      <span className="text-xs text-destructive">
        Couldn&rsquo;t save — your last change wasn&rsquo;t written to disk.
      </span>
    );
  }
  const label =
    state === "saving" ? "Saving…" : state === "saved" ? "Saved." : "Changes save automatically.";
  return <span className="text-xs text-muted-foreground">{label}</span>;
}

export function SettingsScreen() {
  const { theme, setTheme, colorTheme, setColorTheme } = useTheme();
  const { settings, saveState, update, flush } = useUserSettings();
  const { snapshot } = useStoreSnapshot();
  const now = useNow();
  const version = useAppVersion();
  // Consume the store exactly once (on mount) so the tab/filter initializers
  // below don't race each other for the same one-shot target.
  const [initialTarget] = useState(() => settingsTargetStore.consume());
  const initialResolved = resolveTarget(initialTarget);
  const [tab, setTab] = useState(initialResolved.tab);
  const [query, setQuery] = useState(initialResolved.filter);
  const filterRef = useRef<HTMLInputElement>(null);

  // A deep link fired while this screen is already mounted (just hidden)
  // doesn't remount it, so watch the store and re-apply a fresh target live.
  const pendingTarget = useSyncExternalStore(
    settingsTargetStore.subscribe,
    settingsTargetStore.get,
    settingsTargetStore.get,
  );
  useEffect(() => {
    if (!pendingTarget) return;
    const resolved = resolveTarget(settingsTargetStore.consume());
    setTab(resolved.tab);
    setQuery(resolved.filter);
  }, [pendingTarget]);

  const run = (key: string) => snapshot.runs.find((r) => r.collector === key);

  // Escape clears the filter (a second, empty-box Escape has no further
  // action here — closing this screen is the close-tab shortcut's job, not
  // one owned by the filter input).
  const onFilterKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key !== "Escape") return;
    if (isEmptyQuery(query)) return;
    e.preventDefault();
    setQuery("");
  };

  const collectorsPrelude = (
    <div className="rounded-md border border-dashed p-3 text-xs text-muted-foreground">
      Changes save as you make them — the scheduler re-reads its cadence live.
    </div>
  );

  return (
    <div className="flex h-full min-h-0 flex-col">
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
            <TabsTrigger key={t.id} value={t.id} className="justify-start gap-2 px-2 py-1.5">
              <t.icon className="size-4" />
              {t.label}
            </TabsTrigger>
          ))}
        </TabsList>

        <div className="min-h-0 flex-1 overflow-y-auto">
          <TabsContent value="general" className="flex flex-col gap-5 p-4">
            <TabHeading title="General" note="Editor used to open repos." />
            {settings ? (
              <FilteredContent query={query} sections={generalSections(settings, update, flush)} />
            ) : (
              <SettingsLoading />
            )}
          </TabsContent>

          <TabsContent value="appearance" className="flex flex-col gap-5 p-4">
            <TabHeading title="Appearance" note="Theme applies immediately across the app." />
            <FilteredContent
              query={query}
              sections={appearanceSections(theme, setTheme, colorTheme, setColorTheme)}
            />
          </TabsContent>

          <TabsContent value="agentboard" className="flex flex-col gap-5 p-4">
            <TabHeading title="Agentboard" note="Repo discovery and needs-you notifications." />
            <FilteredContent query={query} sections={agentboardSections(settings, update, flush)} />
          </TabsContent>

          <TabsContent value="journal" className="flex flex-col gap-5 p-4">
            <TabHeading
              title="Journal"
              note="Where notes live and how their file paths are templated."
            />
            {settings ? (
              <FilteredContent query={query} sections={journalSections(settings, update, flush)} />
            ) : (
              <SettingsLoading />
            )}
          </TabsContent>

          <TabsContent value="collectors" className="flex flex-col gap-5 p-4">
            <TabHeading
              title="Collectors"
              note="Background jobs that fill the data hub. Each has an enable flag and cadence."
              action={<RefreshNowButton />}
            />
            {settings ? (
              <FilteredContent
                query={query}
                sections={collectorsSections(settings, update, run, now, flush)}
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
        <SaveStatus state={saveState} />
      </footer>
    </div>
  );
}
