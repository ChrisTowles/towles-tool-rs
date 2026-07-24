import { useState } from "react";
import { RefreshCw } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { CollectorFreshness } from "@/components/store-bits";
import { PromptTemplateList } from "@/components/prompt-template-list";
import { NotInTauri } from "@/lib/errors";
import { storeCollectNow, type CollectRun } from "@/lib/data";
import {
  nextCalendarSourceId,
  nextPromptImproverId,
  type CalendarSource,
  type PromptImprover,
  type UserSettings,
} from "@/lib/settings";
import { uiAction } from "@/lib/ui-action";
import {
  CadenceRow,
  clampHour,
  FieldRow,
  RevealInput,
  SettingRow,
  SlackUserPicker,
  ToggleRow,
  WeekdayChips,
  type FilterSection,
  type Flush,
  type Update,
} from "./common";

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
export function PromptImproversEditor({
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
export function RefreshNowButton() {
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

export function collectorsSections(
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
              description="How often to re-poll open + review-requested PRs."
              value={c.prs.refreshSeconds}
              unit="sec"
              onValue={(n) => setPrs({ refreshSeconds: n }, typed)}
              onCommit={() => void flush()}
            />
          ),
        },
        {
          label: "Merged PRs refresh every",
          keywords: ["cadence", "interval", "merged"],
          node: (
            <CadenceRow
              label="Merged PRs refresh every"
              description="How often to re-poll recently-merged PRs. Looser than the open-PR cadence since this only catches a just-merged branch before its worktree is removed."
              value={c.prs.mergedRefreshMinutes}
              unit="min"
              onValue={(n) => setPrs({ mergedRefreshMinutes: n }, typed)}
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
