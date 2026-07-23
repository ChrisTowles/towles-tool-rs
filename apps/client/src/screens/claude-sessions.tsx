import { useEffect, useMemo, useRef, useState } from "react";
import * as echarts from "echarts";
import {
  ArrowDown,
  ArrowUp,
  ArrowUpDown,
  BarChart3,
  Check,
  CircleCheck,
  Clock,
  Copy,
  DatabaseZap,
  Flame,
  Lightbulb,
  List,
  MessagesSquare,
  RefreshCw,
  Repeat2,
  Search,
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
import { Card, StatTile } from "@/components/store-bits";
import {
  claudeSessionsBreakdown,
  claudeSessionsCadence,
  claudeSessionsInsights,
  claudeSessionsSearch,
  claudeSessionsSummary,
  type CadenceSummary,
  type ClaudeSession,
  type ClaudeSessionInsight,
  type ClaudeSessionsSummary,
  type InsightKind,
  type LedgerDay,
  type SessionBreakdown,
} from "@/lib/claude-sessions";
import { NotInTauri, type IpcError } from "@/lib/errors";
import { cn } from "@/lib/utils";

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

/** Estimated dollar cost. Sub-cent figures round to `<$0.01` rather than `$0.00`
 * so a nonzero session never reads as free. */
function formatCost(n: number): string {
  if (n > 0 && n < 0.01) return "<$0.01";
  if (n >= 1_000) return `$${(n / 1_000).toFixed(1)}K`;
  if (n >= 100) return `$${n.toFixed(0)}`;
  return `$${n.toFixed(2)}`;
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

/** `0` → `12am`, `13` → `1pm`, … — compact hour-of-day axis labels. */
function formatHourLabel(hour: number): string {
  const period = hour < 12 ? "am" : "pm";
  const twelveHour = hour % 12 === 0 ? 12 : hour % 12;
  return `${twelveHour}${period}`;
}

/** `YYYY-MM-DD` for a `Date`, in local time (never UTC — `toISOString` would
 * shift the date near midnight). */
function localDateStr(d: Date): string {
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${y}-${m}-${day}`;
}

/** The grid's date span: the selected window (`days`, `"0"` = all time)
 * ending today. The backend already clips `byDay`/`byDayHour` to this same
 * window per-prompt (not just per-session mtime — see `build_cadence`'s doc
 * comment), so this only needs to mirror that window for the axis. */
function dateRange(daysSel: string, byDay: CadenceSummary["byDay"]): [string, string] {
  const today = new Date();
  const end = localDateStr(today);
  const n = Number(daysSel);
  if (n > 0) {
    const start = new Date(today);
    start.setDate(start.getDate() - (n - 1));
    return [localDateStr(start), end];
  }
  return [byDay.length ? byDay[0].date : end, end];
}

/** Every `YYYY-MM-DD` from `start` to `end`, inclusive, ascending — the row
 * labels for the day×hour grid, so a day with zero prompts still gets a row
 * of empty cells rather than silently disappearing. */
function enumerateDates(start: string, end: string): string[] {
  const dates: string[] = [];
  const last = new Date(`${end}T00:00:00`);
  // Reassigned (not mutated) each pass via `setDate` on a fresh `Date`, so a
  // DST transition's 23/25-hour day can't skip or repeat a calendar date the
  // way a fixed-millisecond increment would.
  let cur = new Date(`${start}T00:00:00`);
  while (cur <= last) {
    dates.push(localDateStr(cur));
    const next = new Date(cur);
    next.setDate(next.getDate() + 1);
    cur = next;
  }
  return dates;
}

/** `true` for a `YYYY-MM-DD` local date that falls on Saturday or Sunday. */
function isWeekend(date: string): boolean {
  const dow = new Date(`${date}T00:00:00`).getDay();
  return dow === 0 || dow === 6;
}

/** `true` for an 8am–5pm hour (`8..16`) on a weekday — the "work hours" cue. */
function isWorkHour(date: string, hour: number): boolean {
  return !isWeekend(date) && hour >= 8 && hour < 17;
}

/** Linear-interpolates two `#rrggbb` colors at `t` (0..1). */
function lerpColor(a: string, b: string, t: number): string {
  const pa = [1, 3, 5].map((i) => Number.parseInt(a.slice(i, i + 2), 16));
  const pb = [1, 3, 5].map((i) => Number.parseInt(b.slice(i, i + 2), 16));
  return `#${pa
    .map((ca, i) => Math.round(ca + (pb[i] - ca) * t).toString(16).padStart(2, "0"))
    .join("")}`;
}

/** Quantile (equal-*count*, not equal-value) bins over the nonzero counts, so
 * a heavily skewed distribution — a handful of marathon hours against many
 * 1-2-prompt hours — still spreads across the gradient instead of every cell
 * but the outliers landing in one bucket. Up to 9 bins (10 total ranges once
 * the zero piece is added), fewer only when the data has too little spread
 * to fill that many distinct boundaries. */
function buildCountPieces(counts: number[]): { min: number; max: number; color: string }[] {
  const nonzero = counts.filter((c) => c > 0);
  if (nonzero.length === 0) return [];
  const sorted = nonzero.toSorted((a, b) => a - b);
  const targetBins = 9;
  const boundaries = Array.from({ length: targetBins }, (_, i) => {
    const idx = Math.min(sorted.length - 1, Math.ceil(((i + 1) * sorted.length) / targetBins) - 1);
    return sorted[idx];
  });
  const uniqueBoundaries = [...new Set(boundaries)];
  let lo = 1;
  return uniqueBoundaries.map((hi, i) => {
    const t = uniqueBoundaries.length === 1 ? 1 : i / (uniqueBoundaries.length - 1);
    const piece = { min: lo, max: hi, color: lerpColor(PALETTE[0], "#0b2f70", t) };
    lo = hi + 1;
    return piece;
  });
}

/** Punch-card heat map: one column per day (oldest on the left, so the axis
 * reads left-to-right as time passing), one row per hour, one block per
 * cell. Every (day, hour) pair is included — even zero-prompt ones — and
 * rendered white via `visualMap.outOfRange` (`min: 1` puts zero cells
 * outside the mapped range), so an empty cell reads as "no prompts" rather
 * than disappearing into the page background. Fixed square cells via the
 * grid's own pixel `width`/`height` (not `left`+`right`, which would give
 * echarts a fixed box and it'd silently re-stretch the cells to fill it —
 * the same trap the calendar-coordinate version of this chart hit before). */
function DayHourHeatmap({
  byDayHour,
  range,
}: {
  byDayHour: CadenceSummary["byDayHour"];
  range: [string, string];
}) {
  const days = useMemo(() => enumerateDates(range[0], range[1]), [range]);
  const cellPx = 16;
  const gridWidth = days.length * cellPx;
  const gridHeight = 24 * cellPx;

  const ref = useEChart(
    (chart) => {
      const foreground = cssVar("--foreground");
      const muted = cssVar("--muted-foreground");
      const border = cssVar("--border");
      const card = cssVar("--card");

      const countByCell = new Map(byDayHour.map((c) => [`${c.date}|${c.hour}`, c.count]));
      // Cells are fully opaque, so a background layer behind them (tried
      // `markArea`, then `xAxis.splitArea` — neither rendered, and once this
      // was understood as an opacity problem rather than an API one, the fix
      // is obvious: verified live that both are simply painted over). A
      // per-cell border is the one cue that survives on top — amber for a
      // weekend day, teal for an 8am–5pm weekday hour (the two are mutually
      // exclusive, so no cell needs both).
      const data: { value: [number, number, number]; itemStyle?: { borderColor: string } }[] = [];
      days.forEach((date, dayIndex) => {
        const weekend = isWeekend(date);
        for (let hour = 0; hour < 24; hour++) {
          const borderColor = weekend
            ? PALETTE[3]
            : isWorkHour(date, hour)
              ? PALETTE[4]
              : undefined;
          data.push({
            value: [dayIndex, hour, countByCell.get(`${date}|${hour}`) ?? 0],
            itemStyle: borderColor ? { borderColor } : undefined,
          });
        }
      });

      chart.setOption({
        tooltip: {
          backgroundColor: card,
          borderColor: border,
          textStyle: { color: foreground },
          formatter: (p: { value: [number, number, number] }) =>
            `${days[p.value[0]]} · ${formatHourLabel(p.value[1])}<br/>${p.value[2]} prompt${
              p.value[2] === 1 ? "" : "s"
            }`,
        },
        visualMap: {
          // Piecewise, not continuous: a continuous visualMap with `min: 1`
          // and an explicit `outOfRange` still painted every zero cell the
          // same as the nonzero low end (verified live — `outOfRange` never
          // took effect regardless of `color` shape or `dimension`).
          // Piecewise pieces are colored independently with no such
          // fallback, so the zero piece reliably renders white.
          type: "piecewise",
          dimension: 2,
          show: false,
          pieces: [
            { min: 0, max: 0, color: "#ffffff" },
            ...buildCountPieces(byDayHour.map((c) => c.count)),
          ],
        },
        grid: { left: 44, top: 8, width: gridWidth, height: gridHeight },
        xAxis: {
          type: "category",
          data: days.map((d) => d.slice(5)),
          axisLine: { show: false },
          axisTick: { show: false },
          axisLabel: {
            color: (_value: string, index: number) =>
              isWeekend(days[index]) ? PALETTE[3] : muted,
            fontSize: 9.5,
            interval: Math.ceil(days.length / 15) - 1,
            rotate: 45,
          },
        },
        yAxis: {
          type: "category",
          inverse: true,
          data: Array.from({ length: 24 }, (_, h) => formatHourLabel(h)),
          axisLine: { show: false },
          axisTick: { show: false },
          axisLabel: { color: muted, fontSize: 10 },
        },
        series: [
          {
            type: "heatmap",
            data,
            itemStyle: { borderWidth: 1, borderColor: border },
          },
        ],
      });
    },
    [byDayHour, days],
  );

  if (byDayHour.length === 0)
    return <p className="text-sm text-muted-foreground">No prompts in this range.</p>;
  return (
    <div className="overflow-x-auto">
      <div ref={ref} style={{ height: gridHeight + 60, width: gridWidth + 60 }} />
    </div>
  );
}

/** When in the day, and how often, the human sends prompts — cadence, not
 * token/cost accounting. Same fetch-once-per-activation pattern as
 * [`InsightsTab`]. */
function CadenceTab({ days, nonce, active }: { days: string; nonce: number; active: boolean }) {
  const [cadence, setCadence] = useState<CadenceSummary | null>(null);
  const [loading, setLoading] = useState(false);
  const fetchedKey = useRef<string | null>(null);

  useEffect(() => {
    const key = `${days}:${nonce}`;
    if (!active || fetchedKey.current === key) return;
    fetchedKey.current = key;
    setLoading(true);
    void claudeSessionsCadence(Number(days)).then((r) => {
      r.match({
        ok: setCadence,
        err: (e) => {
          setCadence(null);
          reportSessionsError(e);
        },
      });
      setLoading(false);
    });
  }, [active, days, nonce]);

  if (loading && !cadence)
    return <p className="p-4 text-sm text-muted-foreground">Scanning prompts…</p>;
  if (!cadence) return null;

  const activeDays = cadence.byDay.length;
  const avgPerDay = activeDays > 0 ? cadence.totalPrompts / activeDays : 0;
  const range = dateRange(days, cadence.byDay);

  return (
    <div className="flex flex-col gap-4 p-4">
      <div className="grid grid-cols-2 gap-3 md:grid-cols-3">
        <StatTile label="Prompts" value={String(cadence.totalPrompts)} />
        <StatTile label="Active days" value={String(activeDays)} />
        <StatTile label="Avg / active day" value={avgPerDay.toFixed(1)} />
      </div>
      <Card title="Prompts by day & hour">
        <DayHourHeatmap byDayHour={cadence.byDayHour} range={range} />
      </Card>
      <p className="text-[11px] text-muted-foreground">
        Dates and hours are local time. Counts are human-typed prompts only — tool results and
        injected envelopes are excluded, and token/cost volume plays no part here.{" "}
        <span style={{ color: PALETTE[3] }}>Amber</span> outlines a weekend day;{" "}
        <span style={{ color: PALETTE[4] }}>teal</span> outlines 8am–5pm on a weekday.
      </p>
    </div>
  );
}

type SessionSortKey =
  | "title"
  | "project"
  | "date"
  | "billable"
  | "cacheRead"
  | "cacheWrite"
  | "cost";
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
  cost: "desc",
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
    case "cost":
      return s.costUsd;
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

/** Copies `text` to the clipboard, briefly swapping the icon to a checkmark
 * as the only feedback — no toast, so copying several rows in a row doesn't
 * spam notifications. */
function useClipboardCopy() {
  const [copiedKey, setCopiedKey] = useState<string | null>(null);

  function copy(key: string, text: string) {
    void navigator.clipboard.writeText(text).then(() => {
      setCopiedKey(key);
      setTimeout(() => setCopiedKey((k) => (k === key ? null : k)), 1200);
    });
  }

  return { copiedKey, copy };
}

/** Icon buttons to copy a session's ID and transcript file path — the two
 * things needed to point Claude at a specific session file. Shared by the
 * Sessions table, Insights cards, and the breakdown dialog header. */
function CopySessionButtons({ session }: { session: ClaudeSession }) {
  const { copiedKey, copy } = useClipboardCopy();

  return (
    <span className="inline-flex items-center gap-0.5">
      <Tooltip>
        <TooltipTrigger asChild>
          <Button
            variant="ghost"
            size="icon-xs"
            onClick={(e) => {
              e.stopPropagation();
              copy("id", session.sessionId);
            }}
          >
            {copiedKey === "id" ? <Check className="text-green-500" /> : <Copy />}
          </Button>
        </TooltipTrigger>
        <TooltipContent>Copy session ID</TooltipContent>
      </Tooltip>
      <Tooltip>
        <TooltipTrigger asChild>
          <Button
            variant="ghost"
            size="icon-xs"
            onClick={(e) => {
              e.stopPropagation();
              copy("path", session.path);
            }}
          >
            {copiedKey === "path" ? <Check className="text-green-500" /> : <Copy />}
          </Button>
        </TooltipTrigger>
        <TooltipContent>Copy session file path</TooltipContent>
      </Tooltip>
    </span>
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
          <DialogTitle className="flex items-center gap-2 pr-6">
            <span className="min-w-0 truncate">
              {session?.title ?? session?.sessionId.slice(0, 8)}
            </span>
            {session && <CopySessionButtons session={session} />}
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
            <SortableTh
              sortKey="cost"
              active={sort?.key === "cost"}
              dir={sort?.dir ?? "desc"}
              align="right"
              onSort={toggleSort}
            >
              Cost
            </SortableTh>
            <th className="py-1.5 pl-3 font-medium" aria-label="Actions" />
          </tr>
        </thead>
        <tbody>
          {rows.map((s) => {
            const billable = s.inputTokens + s.outputTokens;
            const outlier = medianBillable > 0 && billable >= OUTLIER_FACTOR * medianBillable;
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
                <td className="py-1.5 pr-3 text-right font-mono text-xs text-foreground">
                  {formatCost(s.costUsd)}
                </td>
                <td className="py-1.5 pl-3 text-right">
                  <CopySessionButtons session={s} />
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
 * session, one number, and why it matters. Fetches only while this tab is
 * the active one (a `days` change on another tab is picked up on the next
 * activation). */
function InsightsTab({ days, nonce, active }: { days: string; nonce: number; active: boolean }) {
  const [insights, setInsights] = useState<ClaudeSessionInsight[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [breakdownFor, setBreakdownFor] = useState<ClaudeSession | null>(null);
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
            <CopySessionButtons session={s} />
          </div>
        );
      })}
      <p className="text-[11px] text-muted-foreground">
        Click a finding for its turn/tool breakdown, or{" "}
        <Copy className="inline size-3 align-[-2px]" /> to copy the session ID or file path.
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
          <div className="grid grid-cols-2 gap-3 border-b border-border p-4 lg:grid-cols-5">
            <StatTile label="Sessions" value={String(totals.sessions)} />
            <StatTile
              label="In + Out"
              value={formatTokens(totals.inputTokens + totals.outputTokens)}
              detail={`${formatTokens(totals.inputTokens)} in · ${formatTokens(totals.outputTokens)} out`}
            />
            <StatTile label="Cache read" value={formatTokens(totals.cacheReadTokens)} />
            <StatTile label="Cache write" value={formatTokens(totals.cacheCreationTokens)} />
            <StatTile
              label="Est. cost"
              value={formatCost(totals.costUsd)}
              detail="approx · per-model rates"
            />
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
              <TabsTrigger value="cadence" className="justify-start gap-2 px-2 py-1.5">
                <Clock className="size-4" />
                Cadence
              </TabsTrigger>
            </TabsList>

            <div className="min-h-0 flex-1 overflow-y-auto">
              <TabsContent value="overview" className="flex flex-col gap-4 p-4">
                <Card title="Tokens by day">
                  <DayStackChart days={summary.days} />
                </Card>

                <div className="grid gap-4 md:grid-cols-2">
                  <Card title="By repo">
                    <RankedBarChart
                      bars={summary.byProject.map((b) => ({ label: b.project, ...b }))}
                    />
                  </Card>
                  <Card title="By model">
                    <RankedBarChart bars={summary.byModel.map((b) => ({ label: b.model, ...b }))} />
                  </Card>
                </div>
              </TabsContent>

              <TabsContent value="sessions" className="p-4">
                <Card
                  title={searching ? "Search results" : "Top sessions"}
                  action={
                    <div className="relative w-72 self-center">
                      <Search className="absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
                      <Input
                        value={query}
                        onChange={(e) => setQuery(e.target.value)}
                        placeholder="Search titles & prompts…"
                        className="h-8 pl-8 text-sm"
                      />
                    </div>
                  }
                >
                  <SessionTable sessions={sessions} searching={searching} />
                  <p className="mt-2 text-[11px] text-muted-foreground">
                    {searching
                      ? "Matches session titles and what you typed, newest first."
                      : "Ranked by input+output tokens; amber marks outliers vs the median."}{" "}
                    Click <Copy className="inline size-3 align-[-2px]" /> to copy the session ID or
                    file path.
                  </p>
                </Card>
              </TabsContent>

              <TabsContent value="insights">
                <InsightsTab days={days} nonce={refreshNonce} active={tab === "insights"} />
              </TabsContent>

              <TabsContent value="cadence">
                <CadenceTab days={days} nonce={refreshNonce} active={tab === "cadence"} />
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
