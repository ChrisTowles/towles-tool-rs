import { useEffect, useRef, useState } from "react";
import * as echarts from "echarts";
import { GitFork, History, RefreshCw, Sparkles } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { IconBtn } from "@/components/agentboard-bits";
import { Panel, Empty } from "@/components/store-bits";
import { TerminalView } from "@/components/terminal-view";
import { fmtAge } from "@/lib/data";
import { forkSessionCommand, termWriteRetry } from "@/lib/agentboard";
import {
  claudeSessionsList,
  claudeSessionsSummary,
  type ClaudeSession,
  type SpendSummary,
} from "@/lib/claude-sessions";

/** An in-flight fork: a fresh PTY spawned at a past session's original cwd,
 * about to be handed `claude --resume <id> --fork-session` (see
 * `forkSessionCommand`). Closing the dialog kills the shell — `TerminalView`
 * unmounting is what tears down the PTY. */
type ForkTarget = { termId: string; cwd: string; command: string; label: string };

const DAY_OPTIONS = [
  { label: "Last 7 days", value: "7" },
  { label: "Last 30 days", value: "30" },
  { label: "Last 90 days", value: "90" },
  { label: "All time", value: "0" },
];

/** Cap the project ranking so one sprawling history doesn't dwarf the chart. */
const MAX_PROJECT_BARS = 12;

/** Cap the recent-sessions list so one sprawling history doesn't dwarf the panel. */
const MAX_SESSIONS_SHOWN = 20;

function cssVar(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return String(n);
}

/** Bumps whenever `<html>`'s class actually flips (light/dark), so chart color
 * reads never race the theme-provider's own effect that toggles the class. */
function useDomThemeVersion(): number {
  const [version, setVersion] = useState(0);
  useEffect(() => {
    const observer = new MutationObserver(() => setVersion((v) => v + 1));
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ["class"] });
    return () => observer.disconnect();
  }, []);
  return version;
}

/** A horizontal ranked bar chart: one series, category identity lives on the axis
 * label rather than per-bar hue (the app's chart tokens are grayscale by design —
 * hue is reserved for agent status elsewhere in the UI). */
function SpendBarChart({ bars }: { bars: { label: string; totalTokens: number }[] }) {
  const ref = useRef<HTMLDivElement>(null);
  const themeVersion = useDomThemeVersion();

  useEffect(() => {
    const el = ref.current;
    if (!el || bars.length === 0) return;

    const chart = echarts.init(el);
    const foreground = cssVar("--foreground");
    const muted = cssVar("--muted-foreground");
    const border = cssVar("--border");
    const card = cssVar("--card");
    const barColor = cssVar("--chart-2");

    chart.setOption({
      grid: { left: 8, right: 48, top: 8, bottom: 8, containLabel: true },
      tooltip: {
        trigger: "item",
        backgroundColor: card,
        borderColor: border,
        textStyle: { color: foreground },
        valueFormatter: (v: unknown) => `${(v as number).toLocaleString()} tokens`,
      },
      xAxis: {
        type: "value",
        axisLine: { show: false },
        axisTick: { show: false },
        splitLine: { lineStyle: { color: border } },
        // The bar-end labels already show the exact value; a numeric axis
        // below would just repeat it (and its ticks overlap in a narrow card).
        axisLabel: { show: false },
      },
      yAxis: {
        type: "category",
        data: bars.map((b) => b.label),
        inverse: true,
        axisLine: { lineStyle: { color: border } },
        axisTick: { show: false },
        // Fixed width + truncation: a single very long project/model name
        // must not blow out the label column and starve the bar area — the
        // tooltip shows the untruncated name on hover.
        axisLabel: { color: foreground, width: 150, overflow: "truncate" },
      },
      series: [
        {
          type: "bar",
          data: bars.map((b) => b.totalTokens),
          barMaxWidth: 22,
          itemStyle: { color: barColor, borderRadius: [0, 4, 4, 0] },
          label: {
            show: true,
            position: "right",
            color: muted,
            formatter: (p: { value: number }) => formatTokens(p.value),
          },
        },
      ],
    });

    // A tab switch mounts this container before layout has settled, so the
    // container can be 0×0 at echarts.init() time (it warns and renders
    // nothing). A ResizeObserver catches the first real layout pass, not just
    // later window-level resizes.
    const onResize = () => chart.resize();
    window.addEventListener("resize", onResize);
    const observer = new ResizeObserver(onResize);
    observer.observe(el);
    return () => {
      window.removeEventListener("resize", onResize);
      observer.disconnect();
      chart.dispose();
    };
  }, [bars, themeVersion]);

  if (bars.length === 0) {
    return <p className="text-sm text-muted-foreground">No sessions in this range.</p>;
  }

  return <div ref={ref} style={{ height: Math.max(120, bars.length * 36) }} />;
}

function SessionRow({
  session,
  now,
  onFork,
}: {
  session: ClaudeSession;
  now: number;
  /** Undefined when the session has no recorded cwd (older transcript) — the
   * row hides the fork actions rather than forking into an unknown folder. */
  onFork?: (compact: boolean) => void;
}) {
  return (
    <div className="flex items-center gap-3 px-3 py-2.5 text-sm">
      <div className="min-w-0 flex-1">
        <div className="truncate">{session.title ?? "Untitled session"}</div>
        <div className="truncate font-mono text-xs text-muted-foreground">
          {session.project} · {fmtAge(session.mtime, now)}
        </div>
      </div>
      <span className="shrink-0 font-mono text-xs text-muted-foreground">
        {formatTokens(session.tokens)}
      </span>
      {onFork && (
        <div className="flex shrink-0 items-center gap-1">
          <IconBtn title="Fork session here" onClick={() => onFork(false)}>
            <GitFork className="size-3.5" />
          </IconBtn>
          <IconBtn title="Fork + compact here" onClick={() => onFork(true)}>
            <Sparkles className="size-3.5" />
          </IconBtn>
        </div>
      )}
    </div>
  );
}

/** Claude Sessions — Claude Code session history across every repo: where tokens
 * have gone (by project and by model), and what you've actually been working on,
 * over a selectable window. */
export function ClaudeSessionsScreen() {
  const [days, setDays] = useState("7");
  const [summary, setSummary] = useState<SpendSummary | null>(null);
  const [sessions, setSessions] = useState<ClaudeSession[] | null>(null);
  const [loading, setLoading] = useState(true);
  const [now, setNow] = useState(() => Date.now());
  const [forkTarget, setForkTarget] = useState<ForkTarget | null>(null);

  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 30_000);
    return () => clearInterval(id);
  }, []);

  // The dialog below mounts a fresh TerminalView for `forkTarget.termId` in
  // the same render as this effect fires — `termWriteRetry` covers the beat
  // before `term_start` actually registers the PTY (same pattern as
  // Agentboard's `launchClaudeIn`).
  useEffect(() => {
    if (!forkTarget) return;
    void termWriteRetry(forkTarget.termId, forkTarget.command);
  }, [forkTarget]);

  function openFork(session: ClaudeSession, compact: boolean) {
    if (!session.cwd) return;
    setForkTarget({
      termId: crypto.randomUUID(),
      cwd: session.cwd,
      command: forkSessionCommand(session.sessionId, compact),
      label: session.title ?? "session",
    });
  }

  async function refresh(d: string) {
    setLoading(true);
    const [nextSummary, nextSessions] = await Promise.all([
      claudeSessionsSummary(Number(d)),
      claudeSessionsList(Number(d)),
    ]);
    setSummary(nextSummary);
    setSessions(nextSessions);
    setLoading(false);
  }

  useEffect(() => {
    void refresh(days);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [days]);

  const byProject = summary?.byProject.slice(0, MAX_PROJECT_BARS) ?? [];
  const truncatedProjects = (summary?.byProject.length ?? 0) - byProject.length;
  const byModel = summary?.byModel ?? [];
  const shownSessions = sessions?.slice(0, MAX_SESSIONS_SHOWN) ?? [];
  const truncatedSessions = (sessions?.length ?? 0) - shownSessions.length;

  return (
    <div className="flex flex-col gap-4">
      <div className="flex items-center justify-between gap-2">
        <h2 className="font-heading text-lg font-semibold">Claude Sessions</h2>
        <div className="flex items-center gap-2">
          <Select value={days} onValueChange={setDays}>
            <SelectTrigger className="w-36">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {DAY_OPTIONS.map((o) => (
                <SelectItem key={o.value} value={o.value}>
                  {o.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Button variant="outline" size="sm" onClick={() => void refresh(days)}>
            <RefreshCw className="size-3.5" />
            Refresh
          </Button>
        </div>
      </div>

      {loading && !summary ? (
        <p className="text-sm text-muted-foreground">Loading…</p>
      ) : summary ? (
        <div className="flex flex-col gap-4">
          <div className="grid gap-4 md:grid-cols-2">
            <div className="rounded-lg border border-border bg-card p-3.5">
              <h3 className="mb-3 text-sm font-medium text-foreground">Tokens by project</h3>
              <SpendBarChart bars={byProject.map((b) => ({ label: b.project, ...b }))} />
              {truncatedProjects > 0 && (
                <p className="mt-2 text-xs text-muted-foreground">
                  +{truncatedProjects} more project{truncatedProjects === 1 ? "" : "s"} not shown
                </p>
              )}
            </div>
            <div className="rounded-lg border border-border bg-card p-3.5">
              <h3 className="mb-3 text-sm font-medium text-foreground">Tokens by model</h3>
              <SpendBarChart bars={byModel.map((b) => ({ label: b.model, ...b }))} />
            </div>
          </div>

          <Panel
            title="Recent sessions"
            note={sessions ? `${sessions.length}` : undefined}
            icon={<History className="size-4 text-muted-foreground" />}
          >
            {shownSessions.length === 0 ? (
              <Empty>No sessions in this range.</Empty>
            ) : (
              shownSessions.map((s) => (
                <SessionRow
                  key={s.sessionId}
                  session={s}
                  now={now}
                  onFork={s.cwd ? (compact) => openFork(s, compact) : undefined}
                />
              ))
            )}
            {truncatedSessions > 0 && (
              <p className="px-3 py-2 text-xs text-muted-foreground">
                +{truncatedSessions} more session{truncatedSessions === 1 ? "" : "s"} not shown
              </p>
            )}
          </Panel>
        </div>
      ) : (
        <p className="text-sm text-muted-foreground">Not available outside the app.</p>
      )}

      <Dialog
        open={forkTarget != null}
        onOpenChange={(open) => {
          if (!open) setForkTarget(null);
        }}
      >
        <DialogContent className="sm:max-w-3xl">
          <DialogHeader>
            <DialogTitle>Fork — {forkTarget?.label}</DialogTitle>
          </DialogHeader>
          {forkTarget && (
            // data-term-host marks terminal territory for the shortcut guard
            // (see agentboard.tsx) — keys typed here belong to the shell.
            <div className="h-[420px] overflow-hidden rounded-md border" data-term-host>
              <TerminalView
                termId={forkTarget.termId}
                cwd={forkTarget.cwd}
                onExit={() => setForkTarget(null)}
              />
            </div>
          )}
        </DialogContent>
      </Dialog>
    </div>
  );
}
