import { useState } from "react";
import {
  CalendarClock,
  CircleAlert,
  CircleDot,
  Coins,
  ExternalLink,
  GitPullRequest,
  RefreshCw,
  Settings2,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { storeCollectNow, useStoreSnapshot } from "@/lib/data";
import { useNow } from "@/lib/now";
import { openSettings } from "@/lib/open-settings";
import { useUserSettings, type UserSettings } from "@/lib/settings";
import { CollectorFreshness } from "@/components/store-bits";

/**
 * Config — the connectors control panel. The collectors are the app's only
 * paths that spend anything (claude tokens for calendar, GitHub API calls for
 * PRs/issues), so this screen puts their switches, cadences, and last-run
 * health in one place. Saves apply live: `settings_set` signals the scheduler
 * to reload its cadence without a relaunch. Journal paths and appearance live
 * in the Settings window (button below).
 */
export function ConfigScreen() {
  const { settings, loaded, saveState, update, save } = useUserSettings();
  const { snapshot } = useStoreSnapshot();
  const now = useNow();

  if (loaded && !settings) {
    return (
      <div className="flex flex-col gap-4">
        <h2 className="font-heading text-lg font-semibold">Config</h2>
        <p className="text-sm text-muted-foreground">Not available outside the app.</p>
      </div>
    );
  }
  if (!settings) {
    return (
      <div className="flex flex-col gap-4">
        <h2 className="font-heading text-lg font-semibold">Config</h2>
        <p className="text-sm text-muted-foreground">Loading…</p>
      </div>
    );
  }

  const run = (key: string) => snapshot.runs.find((r) => r.collector === key);
  const set = (fn: (draft: UserSettings) => void) =>
    update((prev) => {
      const draft = structuredClone(prev);
      fn(draft);
      return draft;
    });

  return (
    <div className="flex flex-col gap-4">
      <div className="flex items-center justify-between gap-2">
        <h2 className="font-heading text-lg font-semibold">Config</h2>
        <div className="flex items-center gap-3">
          <SaveStateNote state={saveState} />
          <Button size="sm" onClick={() => void save()} disabled={saveState === "saving"}>
            Save
          </Button>
        </div>
      </div>

      <section className="flex flex-col overflow-hidden rounded-lg border">
        <div className="flex items-center justify-between gap-2 border-b bg-muted/40 px-3 py-2">
          <span className="text-sm font-medium">Connectors</span>
          <div className="flex items-center gap-3">
            <span className="hidden text-xs text-muted-foreground sm:inline">
              saves apply live — the scheduler reloads its cadence
            </span>
            <RefreshNowButton />
          </div>
        </div>

        <CollectorCard
          icon={<CalendarClock className="size-4 text-muted-foreground" />}
          title="Calendar"
          subtitle={
            <span className="flex items-center gap-1.5 text-amber-600 dark:text-amber-500">
              <Coins className="size-3.5" />
              runs `claude -p` against your calendar MCP — every run costs tokens
            </span>
          }
          enabled={settings.collectors.calendar.enabled}
          onEnabled={(v) => set((d) => void (d.collectors.calendar.enabled = v))}
          freshness={<CollectorFreshness run={run("claude:calendar")} now={now} />}
        >
          <Field label="Provider">
            <Select
              value={settings.collectors.calendar.provider}
              onValueChange={(v) => set((d) => void (d.collectors.calendar.provider = v))}
            >
              <SelectTrigger className="w-32">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="google">Google</SelectItem>
                <SelectItem value="outlook">Outlook</SelectItem>
              </SelectContent>
            </Select>
          </Field>
          <Field label="Refresh (min)">
            <NumberInput
              value={settings.collectors.calendar.refreshMinutes}
              min={5}
              onChange={(n) => set((d) => void (d.collectors.calendar.refreshMinutes = n))}
            />
          </Field>
        </CollectorCard>

        <CollectorCard
          icon={<GitPullRequest className="size-4 text-muted-foreground" />}
          title="Pull requests"
          subtitle="`gh pr list` across your tracked repos"
          enabled={settings.collectors.prs.enabled}
          onEnabled={(v) => set((d) => void (d.collectors.prs.enabled = v))}
          freshness={<CollectorFreshness run={run("prs")} now={now} />}
        >
          <Field label="Refresh (sec)">
            <NumberInput
              value={settings.collectors.prs.refreshSeconds}
              min={30}
              onChange={(n) => set((d) => void (d.collectors.prs.refreshSeconds = n))}
            />
          </Field>
        </CollectorCard>

        <CollectorCard
          icon={<CircleDot className="size-4 text-muted-foreground" />}
          title="Issues"
          subtitle="`gh issue list --assignee @me` across your tracked repos"
          enabled={settings.collectors.issues.enabled}
          onEnabled={(v) => set((d) => void (d.collectors.issues.enabled = v))}
          freshness={<CollectorFreshness run={run("issues")} now={now} />}
        >
          <Field label="Refresh (min)">
            <NumberInput
              value={settings.collectors.issues.refreshMinutes}
              min={1}
              onChange={(n) => set((d) => void (d.collectors.issues.refreshMinutes = n))}
            />
          </Field>
        </CollectorCard>

        <div className="flex items-center gap-2 px-3 py-2 text-xs text-muted-foreground">
          <CircleAlert className="size-3.5 shrink-0" />
          Collection pauses while the window is minimized and resumes on the next tick after
          restore.
        </div>
      </section>

      <section className="flex flex-col overflow-hidden rounded-lg border">
        <div className="border-b bg-muted/40 px-3 py-2 text-sm font-medium">Editor</div>
        <div className="flex items-center gap-3 px-3 py-3">
          <Field label="Preferred editor">
            <Input
              className="w-44"
              value={settings.preferredEditor}
              onChange={(e) => set((d) => void (d.preferredEditor = e.target.value))}
            />
          </Field>
          <span className="text-xs text-muted-foreground">
            used by open-in-editor across the app (e.g. `code`, `cursor`)
          </span>
        </div>
      </section>

      <section className="flex items-center justify-between rounded-lg border px-3 py-3">
        <div className="flex flex-col gap-1">
          <span className="text-sm font-medium">Everything else</span>
          <span className="font-mono text-[11px] text-muted-foreground">
            ~/.config/towles-tool/towles-tool.settings.json — shared with the `tt` CLI; unknown
            keys are preserved on save
          </span>
        </div>
        <Button variant="outline" size="sm" onClick={() => void openSettings()}>
          <Settings2 className="size-3.5" />
          Open settings window
          <ExternalLink className="size-3" />
        </Button>
      </section>
    </div>
  );
}

/**
 * Force the issues/PRs/Slack collectors to run now instead of waiting for the
 * next scheduled tick (calendar stays on its cadence — it costs tokens). The
 * button disables and its icon spins while the run is in flight; the store
 * snapshot re-emits from Rust when it finishes, refreshing the freshness lines.
 */
function RefreshNowButton() {
  const [running, setRunning] = useState(false);
  const refresh = async () => {
    if (running) return;
    setRunning(true);
    try {
      await storeCollectNow();
    } finally {
      setRunning(false);
    }
  };
  return (
    <Button variant="outline" size="sm" disabled={running} onClick={() => void refresh()}>
      <RefreshCw className={running ? "size-3.5 animate-spin" : "size-3.5"} />
      {running ? "Refreshing…" : "Refresh now"}
    </Button>
  );
}

function CollectorCard({
  icon,
  title,
  subtitle,
  enabled,
  onEnabled,
  freshness,
  children,
}: {
  icon: React.ReactNode;
  title: string;
  subtitle: React.ReactNode;
  enabled: boolean;
  onEnabled: (v: boolean) => void;
  freshness: React.ReactNode;
  children?: React.ReactNode;
}) {
  return (
    <div className="flex flex-col gap-2 border-b px-3 py-3 last:border-b-0">
      <div className="flex items-center gap-3">
        {icon}
        <div className="flex min-w-0 flex-1 flex-col">
          <span className="text-sm font-medium">{title}</span>
          <span className="truncate text-xs text-muted-foreground">{subtitle}</span>
        </div>
        {freshness}
        <Switch checked={enabled} onCheckedChange={onEnabled} />
      </div>
      {enabled && children && (
        <div className="flex flex-wrap items-center gap-4 pl-7">{children}</div>
      )}
    </div>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="flex items-center gap-2">
      <span className="text-xs text-muted-foreground">{label}</span>
      {children}
    </label>
  );
}

/** Small numeric input that clamps to `min` and swallows non-numbers. */
function NumberInput({
  value,
  min,
  onChange,
}: {
  value: number;
  min: number;
  onChange: (n: number) => void;
}) {
  return (
    <Input
      type="number"
      className="w-24"
      value={value}
      min={min}
      onChange={(e) => {
        const n = Number(e.target.value);
        if (Number.isFinite(n)) {
          onChange(Math.max(min, Math.round(n)));
        }
      }}
    />
  );
}

function SaveStateNote({ state }: { state: "idle" | "saving" | "saved" | "error" }) {
  if (state === "saving")
    return <span className="text-xs text-muted-foreground">Saving…</span>;
  if (state === "saved")
    return <span className="text-xs text-green-600 dark:text-green-500">Saved</span>;
  if (state === "error")
    return <span className="text-xs text-destructive">Save failed</span>;
  return null;
}
