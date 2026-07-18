import { useEffect, useMemo, useRef, useState } from "react";
import * as echarts from "echarts";
import {
  ArrowDown,
  ArrowUp,
  ArrowUpDown,
  BarChart3,
  CircleCheck,
  DatabaseZap,
  Flame,
  Lightbulb,
  List,
  Loader2,
  MessagesSquare,
  RefreshCw,
  Repeat2,
  Search,
  TerminalSquare,
  type LucideIcon,
} from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { abOpenSessionForCwd, requestOpenSession } from "@/lib/agentboard";
import {
  claudeSessionsBreakdown,
  claudeSessionsInsights,
  claudeSessionsSearch,
  claudeSessionsSummary,
  type ClaudeSession,
  type ClaudeSessionInsight,
  type ClaudeSessionsSummary,
  type InsightKind,
  type LedgerDay,
  type SessionBreakdown,
} from "@/lib/claude-sessions";
import { NotInTauri, type IpcError } from "@/lib/errors";
import { cn } from "@/lib/utils";
import { useWorkspace } from "@/lib/workspace";

/** Surface a failed sessions read. Silent outside the Tauri shell, where every
 * command fails identically and the screen already says so. */
function reportSessionsError(error: IpcError) {
  if (!NotInTauri.is(error)) toast.error(error.message);
}

const DAY_OPTIONS = [
  { label: "Last 7 days", value: "7" },
  { label: "Last 30 days", value: "30" },
  { label: "Last 90 days", value: "90" },
  { label: "All time", value: "0" },
];

/** Repos stacked individually in the day chart; the rest fold into "Other". */
const MAX_STACKED_REPOS = 4;

/** A session whose in+out volume is ≥ this multiple of the visible median gets
 * the amber outlier treatment — "this one is not like the others". */
const OUTLIER_FACTOR = 5;

/** CVD-validated categorical palette (carried over from the retired HTML
 * report, `dataviz`-skill validated). This screen is a data-exploration
 * surface: hue encodes series identity here, a sanctioned exception to the
 * app's hue-is-for-status default. */
const PALETTE = [
  "#3987e5", // blue
  "#008300", // green
  "#d55181", // magenta
  "#c98500", // amber
  "#199e70", // teal
  "#d95926", // orange
  "#9085e9", // violet
  "#e66767", // red
];
/** Uncommon tools and the "Other" fold both land on neutral gray. */
const FALLBACK_COLOR = "#8a8a8a";

/** The 8 common tools get dedicated hues (same assignment the old report
 * used); anything else folds to gray so the legend stays honest. */
const TOOL_COLORS: Record<string, string> = {
  Glob: PALETTE[0],
  Read: PALETTE[1],
  Task: PALETTE[3],
  Grep: PALETTE[4],
  Edit: PALETTE[5],
  MultiEdit: PALETTE[5],
  Bash: PALETTE[6],
  Write: PALETTE[7],
};

function toolColor(name: string | null | undefined): string {
  if (!name) return FALLBACK_COLOR;
  return TOOL_COLORS[name] ?? (name.startsWith("mcp") ? PALETTE[2] : FALLBACK_COLOR);
}

const INSIGHT_META: Record<InsightKind, { label: string; icon: LucideIcon; color: string }> = {
  tokenOutlier: { label: "Token outlier", icon: Flame, color: PALETTE[3] },
  rereadLoop: { label: "Re-read loop", icon: Repeat2, color: PALETTE[7] },
  cacheChurn: { label: "Cache churn", icon: DatabaseZap, color: PALETTE[6] },
  marathon: { label: "Marathon session", icon: MessagesSquare, color: PALETTE[0] },
};

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

/** Shared echarts lifecycle: init, first-layout resize, theme re-render. */
function useEChart(render: (chart: echarts.ECharts) => void, deps: unknown[]) {
  const ref = useRef<HTMLDivElement>(null);
  const themeVersion = useDomThemeVersion();

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const chart = echarts.init(el);
    render(chart);
    // A tab switch mounts this container before layout has settled, so the
    // container can be 0×0 at echarts.init() time. A ResizeObserver catches
    // the first real layout pass, not just later window-level resizes.
    const onResize = () => chart.resize();
    window.addEventListener("resize", onResize);
    const observer = new ResizeObserver(onResize);
    observer.observe(el);
    return () => {
      window.removeEventListener("resize", onResize);
      observer.disconnect();
      chart.dispose();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, themeVersion]);

  return ref;
}

/** Rank repos by window total and fold everything past the top N into "Other". */
function stackSeries(days: LedgerDay[]): { repos: string[]; rows: Map<string, number[]> } {
  const totals = new Map<string, number>();
  for (const day of days)
    for (const p of day.projects)
      totals.set(p.project, (totals.get(p.project) ?? 0) + p.totalTokens);
  const ranked = [...totals.entries()].toSorted((a, b) => b[1] - a[1]).map(([p]) => p);
  const top = ranked.slice(0, MAX_STACKED_REPOS);
  const hasOther = ranked.length > top.length;
  const repos = hasOther ? [...top, "Other"] : top;

  const rows = new Map<string, number[]>(repos.map((r) => [r, days.map(() => 0)]));
  days.forEach((day, i) => {
    for (const p of day.projects) {
      const key = top.includes(p.project) ? p.project : "Other";
      const row = rows.get(key);
      if (row) row[i] += p.totalTokens;
    }
  });
  return { repos, rows };
}

/** Stacked day×repo bars. Repos are ranked by window total, so a repo's
 * palette hue matches its row in the "By repo" ranked chart; the "Other" fold
 * stays neutral gray. */
function DayStackChart({ days }: { days: LedgerDay[] }) {
  const ref = useEChart(
    (chart) => {
      const { repos, rows } = stackSeries(days);
      const foreground = cssVar("--foreground");
      const muted = cssVar("--muted-foreground");
      const border = cssVar("--border");
      const card = cssVar("--card");

      chart.setOption({
        grid: { left: 8, right: 8, top: 8, bottom: 44, containLabel: true },
        legend: {
          bottom: 0,
          textStyle: { color: muted, fontSize: 11 },
          itemWidth: 10,
          itemHeight: 10,
        },
        tooltip: {
          trigger: "axis",
          axisPointer: { type: "shadow" },
          backgroundColor: card,
          borderColor: border,
          textStyle: { color: foreground },
          valueFormatter: (v: unknown) => `${(v as number).toLocaleString()} tokens`,
        },
        xAxis: {
          type: "category",
          data: days.map((d) => d.date.slice(5)),
          axisLine: { lineStyle: { color: border } },
          axisTick: { show: false },
          axisLabel: { color: muted, fontSize: 10.5 },
        },
        yAxis: {
          type: "value",
          splitLine: { lineStyle: { color: border } },
          axisLabel: { color: muted, formatter: (v: number) => formatTokens(v) },
        },
        series: repos.map((repo, i) => ({
          name: repo,
          type: "bar",
          stack: "day",
          barMaxWidth: 26,
          data: rows.get(repo),
          itemStyle: {
            color: repo === "Other" ? FALLBACK_COLOR : PALETTE[i % PALETTE.length],
          },
        })),
      });
    },
    [days],
  );

  if (days.length === 0)
    return <p className="text-sm text-muted-foreground">No sessions in this range.</p>;
  return <div ref={ref} style={{ height: 240 }} />;
}

/** A horizontal ranked bar chart: identity on the axis label, one palette hue
 * per rank (rows are sorted descending, matching the day-stack's ordering so
 * a repo keeps its hue across both charts). */
function RankedBarChart({ bars }: { bars: { label: string; totalTokens: number }[] }) {
  const ref = useEChart(
    (chart) => {
      const foreground = cssVar("--foreground");
      const muted = cssVar("--muted-foreground");
      const border = cssVar("--border");
      const card = cssVar("--card");
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
          axisLabel: { show: false },
        },
        yAxis: {
          type: "category",
          data: bars.map((b) => b.label),
          inverse: true,
          axisLine: { lineStyle: { color: border } },
          axisTick: { show: false },
          axisLabel: { color: foreground, width: 150, overflow: "truncate" },
        },
        series: [
          {
            type: "bar",
            data: bars.map((b, i) => ({
              value: b.totalTokens,
              itemStyle: {
                color: PALETTE[i % PALETTE.length],
                borderRadius: [0, 4, 4, 0],
              },
            })),
            barMaxWidth: 22,
            label: {
              show: true,
              position: "right",
              color: muted,
              formatter: (p: { value: number }) => formatTokens(p.value),
            },
          },
        ],
      });
    },
    [bars],
  );

  if (bars.length === 0)
    return <p className="text-sm text-muted-foreground">No sessions in this range.</p>;
  return <div ref={ref} style={{ height: Math.max(120, bars.length * 36) }} />;
}

function StatTile({ label, value, detail }: { label: string; value: string; detail?: string }) {
  return (
    <div className="rounded-lg border border-border bg-card px-3.5 py-2.5">
      <div className="text-[10.5px] font-medium uppercase tracking-wider text-muted-foreground">
        {label}
      </div>
      <div className="mt-0.5 font-mono text-xl font-semibold text-foreground">{value}</div>
      {detail && <div className="text-[11px] text-muted-foreground">{detail}</div>}
    </div>
  );
}

type SessionSortKey = "title" | "project" | "date" | "billable" | "cacheRead" | "cacheWrite";
type SortDir = "asc" | "desc";

/** First-click direction per column: names/repos alphabetize ascending; dates
 * and token volumes lead with the most recent/largest — the more useful
 * default for a table you opened to spot outliers. */
const DEFAULT_SORT_DIR: Record<SessionSortKey, SortDir> = {
  title: "asc",
  project: "asc",
  date: "desc",
  billable: "desc",
  cacheRead: "desc",
  cacheWrite: "desc",
};

function sessionSortValue(s: ClaudeSession, key: SessionSortKey): string | number {
  switch (key) {
    case "title":
      return (s.title ?? s.sessionId).toLowerCase();
    case "project":
      return s.project.toLowerCase();
    case "date":
      return s.date;
    case "billable":
      return s.inputTokens + s.outputTokens;
    case "cacheRead":
      return s.cacheReadTokens;
    case "cacheWrite":
      return s.cacheCreationTokens;
  }
}

function SortableTh({
  sortKey,
  active,
  dir,
  align = "left",
  onSort,
  children,
}: {
  sortKey: SessionSortKey;
  active: boolean;
  dir: SortDir;
  align?: "left" | "right";
  onSort: (key: SessionSortKey) => void;
  children: React.ReactNode;
}) {
  return (
    <th
      className={cn(
        "cursor-pointer select-none py-1.5 pr-3 font-medium hover:text-foreground",
        align === "right" && "text-right",
      )}
      aria-sort={active ? (dir === "asc" ? "ascending" : "descending") : "none"}
      onClick={() => onSort(sortKey)}
    >
      <span
        className={cn("inline-flex items-center gap-1", align === "right" && "flex-row-reverse")}
      >
        {children}
        {active ? (
          dir === "asc" ? (
            <ArrowUp className="size-3" />
          ) : (
            <ArrowDown className="size-3" />
          )
        ) : (
          <ArrowUpDown className="size-3 opacity-30" />
        )}
      </span>
    </th>
  );
}

/** Resolve `s.cwd` to an Agentboard folder (registering the repo first if it
 * isn't on the rail yet — the backend handles that), then hand off selecting +
 * resuming it to Agentboard itself (see `lib/agentboard.ts`'s
 * pending-open-session bridge for why this can't just call into Agentboard
 * directly: it may not be mounted yet). Shared by the Sessions table and the
 * Insights cards. */
function useOpenInAgentboard() {
  const [openingId, setOpeningId] = useState<string | null>(null);
  const { openTab } = useWorkspace();

  async function open(s: ClaudeSession) {
    if (!s.cwd) {
      toast.error("No working directory recorded for this session");
      return;
    }
    setOpeningId(s.sessionId);
    const opened = await abOpenSessionForCwd(s.cwd);
    opened.match({
      ok: (o) => {
        openTab("agentboard");
        requestOpenSession({
          folderDir: o.folderDir,
          sessionId: o.sessionId,
          resumeId: s.sessionId,
          label: s.title ?? s.sessionId.slice(0, 8),
        });
      },
      err: (e) => toast.error(e.message),
    });
    setOpeningId(null);
  }

  return { openingId, open };
}

/** Icon button that resumes a session in Agentboard, with the disabled/tooltip
 * treatment shared between the table rows and insight cards. */
function OpenInAgentboardButton({
  session,
  opening,
  onOpen,
}: {
  session: ClaudeSession;
  opening: boolean;
  onOpen: (s: ClaudeSession) => void;
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="icon-xs"
          disabled={!session.cwd || opening}
          onClick={(e) => {
            e.stopPropagation();
            void onOpen(session);
          }}
          className="text-violet-500 hover:text-violet-400 disabled:text-muted-foreground/40"
        >
          {opening ? <Loader2 className="animate-spin" /> : <TerminalSquare />}
        </Button>
      </TooltipTrigger>
      <TooltipContent>
        {session.cwd
          ? "Open in Agentboard (resumes this session)"
          : "No working directory recorded for this session"}
      </TooltipContent>
    </Tooltip>
  );
}

/** One horizontal magnitude bar, scaled against the section's max. */
function TokenBar({ color, fraction }: { color: string; fraction: number }) {
  return (
    <div className="h-2 flex-1 overflow-hidden rounded-full bg-muted">
      <div
        className="h-full rounded-full"
        style={{ width: `${Math.max(2, fraction * 100)}%`, background: color }}
      />
    </div>
  );
}

/** Turn/tool drill-down for one session — fetched on open, the only
 * per-session re-parse in the screen. */
function BreakdownDialog({
  session,
  onClose,
}: {
  session: ClaudeSession | null;
  onClose: () => void;
}) {
  const [data, setData] = useState<SessionBreakdown | null>(null);
  const [loading, setLoading] = useState(false);
  const sessionId = session?.sessionId;

  useEffect(() => {
    if (!sessionId) return;
    setData(null);
    setLoading(true);
    void claudeSessionsBreakdown(sessionId).then((r) => {
      r.match({ ok: setData, err: reportSessionsError });
      setLoading(false);
    });
  }, [sessionId]);

  const topTools = data?.tools.slice(0, 10) ?? [];
  const maxTool = Math.max(1, ...topTools.map((t) => t.inputTokens + t.outputTokens));
  const topTurns = [...(data?.turns ?? [])]
    .toSorted((a, b) => b.inputTokens + b.outputTokens - (a.inputTokens + a.outputTokens))
    .slice(0, 10);
  const maxTurn = Math.max(1, ...topTurns.map((t) => t.inputTokens + t.outputTokens));

  return (
    <Dialog open={!!session} onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle className="pr-6">
            {session?.title ?? session?.sessionId.slice(0, 8)}
          </DialogTitle>
          <DialogDescription>
            {session?.project} · {session?.date} ·{" "}
            {formatTokens((session?.inputTokens ?? 0) + (session?.outputTokens ?? 0))} in+out
          </DialogDescription>
        </DialogHeader>

        {loading ? (
          <p className="text-sm text-muted-foreground">Parsing session…</p>
        ) : data ? (
          <div className="flex flex-col gap-5">
            <section>
              <h4 className="mb-2 text-[10.5px] font-medium uppercase tracking-wider text-muted-foreground">
                Where the tokens went
              </h4>
              {topTools.length === 0 ? (
                <p className="text-sm text-muted-foreground">No tool calls recorded.</p>
              ) : (
                <div className="flex flex-col gap-1.5">
                  {topTools.map((t) => (
                    <div key={t.name} className="flex items-center gap-2 text-sm">
                      <span
                        className="size-2.5 shrink-0 rounded-[3px]"
                        style={{ background: toolColor(t.name) }}
                      />
                      <span className="w-40 truncate text-foreground">{t.name}</span>
                      <span className="w-10 shrink-0 text-right font-mono text-xs text-muted-foreground">
                        {t.detail}
                      </span>
                      <TokenBar
                        color={toolColor(t.name)}
                        fraction={(t.inputTokens + t.outputTokens) / maxTool}
                      />
                      <span className="w-14 shrink-0 text-right font-mono text-xs text-muted-foreground">
                        {formatTokens(t.inputTokens + t.outputTokens)}
                      </span>
                    </div>
                  ))}
                </div>
              )}
            </section>

            <section>
              <h4 className="mb-2 text-[10.5px] font-medium uppercase tracking-wider text-muted-foreground">
                Heaviest steps
              </h4>
              {topTurns.length === 0 ? (
                <p className="text-sm text-muted-foreground">No token-bearing steps.</p>
              ) : (
                <div className="flex flex-col gap-1.5">
                  {topTurns.map((t, i) => (
                    <div key={i} className="flex items-center gap-2 text-sm">
                      <span
                        className="size-2.5 shrink-0 rounded-[3px]"
                        style={{ background: toolColor(t.toolName) }}
                      />
                      <span className="w-52 truncate text-foreground" title={t.name}>
                        {t.name}
                      </span>
                      <TokenBar
                        color={toolColor(t.toolName)}
                        fraction={(t.inputTokens + t.outputTokens) / maxTurn}
                      />
                      <span className="w-14 shrink-0 text-right font-mono text-xs text-muted-foreground">
                        {formatTokens(t.inputTokens + t.outputTokens)}
                      </span>
                    </div>
                  ))}
                </div>
              )}
              {data.turns.length > topTurns.length && (
                <p className="mt-1.5 text-[11px] text-muted-foreground">
                  Top {topTurns.length} of {data.turns.length} token-bearing steps.
                </p>
              )}
            </section>
          </div>
        ) : (
          <p className="text-sm text-muted-foreground">Could not load this session.</p>
        )}
      </DialogContent>
    </Dialog>
  );
}

function SessionTable({ sessions, searching }: { sessions: ClaudeSession[]; searching: boolean }) {
  const [sort, setSort] = useState<{ key: SessionSortKey; dir: SortDir } | null>(null);
  const [breakdownFor, setBreakdownFor] = useState<ClaudeSession | null>(null);
  const { openingId, open } = useOpenInAgentboard();

  const medianBillable = useMemo(() => {
    const sorted = sessions.map((s) => s.inputTokens + s.outputTokens).toSorted((a, b) => a - b);
    return sorted.length ? sorted[Math.floor(sorted.length / 2)] : 0;
  }, [sessions]);

  const rows = useMemo(() => {
    if (!sort) return sessions;
    const { key, dir } = sort;
    return [...sessions].toSorted((a, b) => {
      const av = sessionSortValue(a, key);
      const bv = sessionSortValue(b, key);
      const cmp = av < bv ? -1 : av > bv ? 1 : 0;
      return dir === "asc" ? cmp : -cmp;
    });
  }, [sessions, sort]);

  const toggleSort = (key: SessionSortKey) =>
    setSort((prev) =>
      prev?.key === key
        ? { key, dir: prev.dir === "asc" ? "desc" : "asc" }
        : { key, dir: DEFAULT_SORT_DIR[key] },
    );

  if (sessions.length === 0)
    return (
      <p className="px-1 py-3 text-sm text-muted-foreground">
        {searching ? "No sessions match." : "No sessions in this range."}
      </p>
    );

  return (
    <div className="overflow-x-auto">
      <table className="w-full text-sm">
        <thead>
          <tr className="border-b border-border text-left text-[10.5px] uppercase tracking-wider text-muted-foreground">
            <SortableTh
              sortKey="title"
              active={sort?.key === "title"}
              dir={sort?.dir ?? "asc"}
              onSort={toggleSort}
            >
              Session
            </SortableTh>
            <SortableTh
              sortKey="project"
              active={sort?.key === "project"}
              dir={sort?.dir ?? "asc"}
              onSort={toggleSort}
            >
              Repo
            </SortableTh>
            <SortableTh
              sortKey="date"
              active={sort?.key === "date"}
              dir={sort?.dir ?? "desc"}
              onSort={toggleSort}
            >
              Date
            </SortableTh>
            <SortableTh
              sortKey="billable"
              active={sort?.key === "billable"}
              dir={sort?.dir ?? "desc"}
              align="right"
              onSort={toggleSort}
            >
              In+Out
            </SortableTh>
            <SortableTh
              sortKey="cacheRead"
              active={sort?.key === "cacheRead"}
              dir={sort?.dir ?? "desc"}
              align="right"
              onSort={toggleSort}
            >
              Cache R
            </SortableTh>
            <SortableTh
              sortKey="cacheWrite"
              active={sort?.key === "cacheWrite"}
              dir={sort?.dir ?? "desc"}
              align="right"
              onSort={toggleSort}
            >
              Cache W
            </SortableTh>
            <th className="py-1.5 pl-3 font-medium" aria-label="Actions" />
          </tr>
        </thead>
        <tbody>
          {rows.map((s) => {
            const billable = s.inputTokens + s.outputTokens;
            const outlier = medianBillable > 0 && billable >= OUTLIER_FACTOR * medianBillable;
            const opening = openingId === s.sessionId;
            return (
              <tr
                key={s.sessionId}
                className="cursor-pointer border-b border-border/60 hover:bg-accent/50"
                onClick={() => setBreakdownFor(s)}
              >
                <td className="max-w-[340px] py-1.5 pr-3">
                  <div className="truncate text-foreground">
                    {s.title ?? (
                      <span className="font-mono text-xs">{s.sessionId.slice(0, 8)}</span>
                    )}
                  </div>
                  {s.snippet && (
                    <div className="truncate text-[11px] text-muted-foreground">{s.snippet}</div>
                  )}
                </td>
                <td className="py-1.5 pr-3 font-mono text-xs text-muted-foreground">{s.project}</td>
                <td className="py-1.5 pr-3 font-mono text-xs text-muted-foreground">{s.date}</td>
                <td
                  className={cn(
                    "py-1.5 pr-3 text-right font-mono text-xs",
                    outlier ? "font-semibold text-amber-500" : "text-foreground",
                  )}
                >
                  {formatTokens(billable)}
                </td>
                <td className="py-1.5 pr-3 text-right font-mono text-xs text-muted-foreground">
                  {formatTokens(s.cacheReadTokens)}
                </td>
                <td className="py-1.5 pr-3 text-right font-mono text-xs text-muted-foreground">
                  {formatTokens(s.cacheCreationTokens)}
                </td>
                <td className="py-1.5 pl-3 text-right">
                  <OpenInAgentboardButton session={s} opening={opening} onOpen={open} />
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
      <BreakdownDialog session={breakdownFor} onClose={() => setBreakdownFor(null)} />
    </div>
  );
}

/** Ranked waste findings for the window — answer-first: each card names one
 * session, one number, and why it matters, with the same resume-in-Agentboard
 * action as the table. Fetches only while this tab is the active one (a
 * `days` change on another tab is picked up on the next activation). */
function InsightsTab({ days, nonce, active }: { days: string; nonce: number; active: boolean }) {
  const [insights, setInsights] = useState<ClaudeSessionInsight[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [breakdownFor, setBreakdownFor] = useState<ClaudeSession | null>(null);
  const { openingId, open } = useOpenInAgentboard();
  const fetchedKey = useRef<string | null>(null);

  useEffect(() => {
    const key = `${days}:${nonce}`;
    if (!active || fetchedKey.current === key) return;
    fetchedKey.current = key;
    setLoading(true);
    void claudeSessionsInsights(Number(days)).then((r) => {
      r.match({
        ok: setInsights,
        err: (e) => {
          setInsights(null);
          reportSessionsError(e);
        },
      });
      setLoading(false);
    });
  }, [active, days, nonce]);

  if (loading && !insights)
    return <p className="p-4 text-sm text-muted-foreground">Scanning for patterns…</p>;
  if (!insights) return null;
  if (insights.length === 0)
    return (
      <div className="flex items-center gap-2.5 p-4 text-sm text-muted-foreground">
        <CircleCheck className="size-4 shrink-0" style={{ color: PALETTE[1] }} />
        No waste patterns in this window — sessions look healthy.
      </div>
    );

  return (
    <div className="flex flex-col gap-3 p-4">
      {insights.map((insight, i) => {
        const meta = INSIGHT_META[insight.kind];
        const s = insight.session;
        return (
          <div
            key={`${insight.kind}-${s.sessionId}-${i}`}
            role="button"
            tabIndex={0}
            onClick={() => setBreakdownFor(s)}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                setBreakdownFor(s);
              }
            }}
            className="flex cursor-pointer items-start gap-3 rounded-lg border border-border bg-card p-3.5 text-left hover:bg-accent/50"
          >
            <span
              className="mt-0.5 flex size-8 shrink-0 items-center justify-center rounded-md"
              style={{ background: `${meta.color}26`, color: meta.color }}
            >
              <meta.icon className="size-4" />
            </span>
            <span className="min-w-0 flex-1">
              <span className="flex items-baseline gap-2">
                <span className="text-sm font-medium text-foreground">{meta.label}</span>
                <span className="font-mono text-xs" style={{ color: meta.color }}>
                  {insight.metric}
                </span>
              </span>
              <span className="mt-0.5 block truncate text-sm text-foreground">
                {s.title ?? s.sessionId.slice(0, 8)}
                <span className="ml-2 font-mono text-xs text-muted-foreground">
                  {s.project} · {s.date}
                </span>
              </span>
              <span className="mt-0.5 block text-[12px] leading-snug text-muted-foreground">
                {insight.detail}
              </span>
            </span>
            <OpenInAgentboardButton session={s} opening={openingId === s.sessionId} onOpen={open} />
          </div>
        );
      })}
      <p className="text-[11px] text-muted-foreground">
        Click a finding for its turn/tool breakdown, or{" "}
        <TerminalSquare className="inline size-3 align-[-2px]" /> to resume the session in
        Agentboard.
      </p>
      <BreakdownDialog session={breakdownFor} onClose={() => setBreakdownFor(null)} />
    </div>
  );
}

/** Claude Sessions — where the tokens went (day × repo × model) and which
 * sessions are the outliers, with title + prompt-text search over the
 * scanned window. */
export function ClaudeSessionsScreen() {
  const [days, setDays] = useState("30");
  const [tab, setTab] = useState("overview");
  const [summary, setSummary] = useState<ClaudeSessionsSummary | null>(null);
  const [loading, setLoading] = useState(true);
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<ClaudeSession[] | null>(null);
  // Bumped by the Refresh button so the Treemap tab rebuilds its report too.
  const [refreshNonce, setRefreshNonce] = useState(0);

  async function refresh(d: string) {
    setLoading(true);
    (await claudeSessionsSummary(Number(d))).match({
      ok: setSummary,
      err: (e) => {
        setSummary(null);
        reportSessionsError(e);
      },
    });
    setLoading(false);
  }

  useEffect(() => {
    void refresh(days);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [days]);

  // Debounced search; empty query falls back to the ranked outlier list.
  useEffect(() => {
    const q = query.trim();
    if (!q) {
      setResults(null);
      return;
    }
    const t = setTimeout(() => {
      void claudeSessionsSearch(Number(days), q).then((r) =>
        r.match({
          ok: setResults,
          err: (e) => {
            setResults([]);
            reportSessionsError(e);
          },
        }),
      );
    }, 250);
    return () => clearTimeout(t);
  }, [query, days]);

  const totals = summary?.totals;
  const searching = query.trim().length > 0;
  const sessions = searching ? (results ?? []) : (summary?.topSessions ?? []);

  return (
    <div className="flex h-full min-h-0 flex-col">
      <header className="flex items-center justify-between gap-2 border-b border-border bg-card px-4 py-3">
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
          <Button
            variant="outline"
            size="sm"
            onClick={() => {
              setRefreshNonce((n) => n + 1);
              void refresh(days);
            }}
          >
            <RefreshCw className="size-3.5" />
            Refresh
          </Button>
        </div>
      </header>

      {loading && !summary ? (
        <p className="p-6 text-sm text-muted-foreground">Scanning sessions…</p>
      ) : summary && totals ? (
        <>
          <div className="grid grid-cols-2 gap-3 border-b border-border p-4 lg:grid-cols-4">
            <StatTile label="Sessions" value={String(totals.sessions)} />
            <StatTile
              label="In + Out"
              value={formatTokens(totals.inputTokens + totals.outputTokens)}
              detail={`${formatTokens(totals.inputTokens)} in · ${formatTokens(totals.outputTokens)} out`}
            />
            <StatTile label="Cache read" value={formatTokens(totals.cacheReadTokens)} />
            <StatTile label="Cache write" value={formatTokens(totals.cacheCreationTokens)} />
          </div>

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
              <TabsTrigger value="overview" className="justify-start gap-2 px-2 py-1.5">
                <BarChart3 className="size-4" />
                Overview
              </TabsTrigger>
              <TabsTrigger value="sessions" className="justify-start gap-2 px-2 py-1.5">
                <List className="size-4" />
                Sessions{searching ? " · search" : ""}
              </TabsTrigger>
              <TabsTrigger value="insights" className="justify-start gap-2 px-2 py-1.5">
                <Lightbulb className="size-4" />
                Insights
              </TabsTrigger>
            </TabsList>

            <div className="min-h-0 flex-1 overflow-y-auto">
              <TabsContent value="overview" className="flex flex-col gap-4 p-4">
                <div className="rounded-lg border border-border bg-card p-3.5">
                  <h3 className="mb-3 text-sm font-medium text-foreground">Tokens by day</h3>
                  <DayStackChart days={summary.days} />
                </div>

                <div className="grid gap-4 md:grid-cols-2">
                  <div className="rounded-lg border border-border bg-card p-3.5">
                    <h3 className="mb-3 text-sm font-medium text-foreground">By repo</h3>
                    <RankedBarChart
                      bars={summary.byProject.map((b) => ({ label: b.project, ...b }))}
                    />
                  </div>
                  <div className="rounded-lg border border-border bg-card p-3.5">
                    <h3 className="mb-3 text-sm font-medium text-foreground">By model</h3>
                    <RankedBarChart bars={summary.byModel.map((b) => ({ label: b.model, ...b }))} />
                  </div>
                </div>
              </TabsContent>

              <TabsContent value="sessions" className="p-4">
                <div className="rounded-lg border border-border bg-card p-3.5">
                  <div className="mb-3 flex items-center justify-between gap-3">
                    <h3 className="text-sm font-medium text-foreground">
                      {searching ? "Search results" : "Top sessions"}
                    </h3>
                    <div className="relative w-72">
                      <Search className="absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
                      <Input
                        value={query}
                        onChange={(e) => setQuery(e.target.value)}
                        placeholder="Search titles & prompts…"
                        className="h-8 pl-8 text-sm"
                      />
                    </div>
                  </div>
                  <SessionTable sessions={sessions} searching={searching} />
                  <p className="mt-2 text-[11px] text-muted-foreground">
                    {searching
                      ? "Matches session titles and what you typed, newest first."
                      : "Ranked by input+output tokens; amber marks outliers vs the median."}{" "}
                    Click <TerminalSquare className="inline size-3 align-[-2px]" /> to resume a
                    session in Agentboard — adds the repo to the rail first if it isn't there yet.
                  </p>
                </div>
              </TabsContent>

              <TabsContent value="insights">
                <InsightsTab days={days} nonce={refreshNonce} active={tab === "insights"} />
              </TabsContent>
            </div>
          </Tabs>
        </>
      ) : (
        <p className="p-6 text-sm text-muted-foreground">Not available outside the app.</p>
      )}
    </div>
  );
}
