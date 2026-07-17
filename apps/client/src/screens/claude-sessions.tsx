import { useEffect, useMemo, useRef, useState } from "react";
import * as echarts from "echarts";
import {
  ArrowDown,
  ArrowUp,
  ArrowUpDown,
  BarChart3,
  Boxes,
  List,
  Loader2,
  RefreshCw,
  Search,
  TerminalSquare,
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
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { abOpenSessionForCwd, requestOpenSession } from "@/lib/agentboard";
import {
  claudeSessionsSearch,
  claudeSessionsSummary,
  claudeSessionsTreemapHtml,
  type ClaudeSession,
  type ClaudeSessionsSummary,
  type LedgerDay,
} from "@/lib/claude-sessions";
import { cn } from "@/lib/utils";
import { useWorkspace } from "@/lib/workspace";

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
    for (const p of day.projects) totals.set(p.project, (totals.get(p.project) ?? 0) + p.totalTokens);
  const ranked = [...totals.entries()].sort((a, b) => b[1] - a[1]).map(([p]) => p);
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

/** Stacked day×repo bars. Series identity rides the grayscale chart tokens
 * (hue stays reserved for status, per the app's design language); the legend
 * and tooltip carry the names. */
function DayStackChart({ days }: { days: LedgerDay[] }) {
  const ref = useEChart(
    (chart) => {
      const { repos, rows } = stackSeries(days);
      const foreground = cssVar("--foreground");
      const muted = cssVar("--muted-foreground");
      const border = cssVar("--border");
      const card = cssVar("--card");
      // Darkest-first so the biggest repo is the most legible on both themes;
      // "Other" (last) lands on the faintest step.
      const shades = ["--chart-2", "--chart-3", "--chart-4", "--chart-5", "--chart-1"].map(cssVar);

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
          itemStyle: { color: shades[i % shades.length] },
        })),
      });
    },
    [days],
  );

  if (days.length === 0)
    return <p className="text-sm text-muted-foreground">No sessions in this range.</p>;
  return <div ref={ref} style={{ height: 240 }} />;
}

/** A horizontal ranked bar chart: one series, identity on the axis label. */
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
            data: bars.map((b) => b.totalTokens),
            barMaxWidth: 22,
            itemStyle: { color: cssVar("--chart-2"), borderRadius: [0, 4, 4, 0] },
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
        className={cn(
          "inline-flex items-center gap-1",
          align === "right" && "flex-row-reverse",
        )}
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

function SessionTable({
  sessions,
  searching,
}: {
  sessions: ClaudeSession[];
  searching: boolean;
}) {
  const [sort, setSort] = useState<{ key: SessionSortKey; dir: SortDir } | null>(null);
  const [openingId, setOpeningId] = useState<string | null>(null);
  const { openTab } = useWorkspace();

  const medianBillable = useMemo(() => {
    const sorted = sessions.map((s) => s.inputTokens + s.outputTokens).sort((a, b) => a - b);
    return sorted.length ? sorted[Math.floor(sorted.length / 2)] : 0;
  }, [sessions]);

  const rows = useMemo(() => {
    if (!sort) return sessions;
    const { key, dir } = sort;
    return [...sessions].sort((a, b) => {
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

  // Resolve `s.cwd` to an Agentboard folder (registering the repo first if
  // it isn't on the rail yet — the backend handles that), then hand off
  // selecting + resuming it to Agentboard itself (see `lib/agentboard.ts`'s
  // pending-open-session bridge for why this can't just call into Agentboard
  // directly: it may not be mounted yet).
  async function openInAgentboard(s: ClaudeSession) {
    if (!s.cwd) {
      toast.error("No working directory recorded for this session");
      return;
    }
    setOpeningId(s.sessionId);
    try {
      const opened = await abOpenSessionForCwd(s.cwd);
      openTab("agentboard");
      requestOpenSession({
        folderDir: opened.folderDir,
        sessionId: opened.sessionId,
        resumeId: s.sessionId,
        label: s.title ?? s.sessionId.slice(0, 8),
      });
    } catch (e) {
      toast.error(String(e));
    } finally {
      setOpeningId(null);
    }
  }

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
            <SortableTh sortKey="title" active={sort?.key === "title"} dir={sort?.dir ?? "asc"} onSort={toggleSort}>
              Session
            </SortableTh>
            <SortableTh sortKey="project" active={sort?.key === "project"} dir={sort?.dir ?? "asc"} onSort={toggleSort}>
              Repo
            </SortableTh>
            <SortableTh sortKey="date" active={sort?.key === "date"} dir={sort?.dir ?? "desc"} onSort={toggleSort}>
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
              <tr key={s.sessionId} className="border-b border-border/60 hover:bg-accent/50">
                <td className="max-w-[340px] py-1.5 pr-3">
                  <div className="truncate text-foreground">
                    {s.title ?? <span className="font-mono text-xs">{s.sessionId.slice(0, 8)}</span>}
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
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <Button
                        variant="ghost"
                        size="icon-xs"
                        disabled={!s.cwd || opening}
                        onClick={() => void openInAgentboard(s)}
                        className="text-violet-500 hover:text-violet-400 disabled:text-muted-foreground/40"
                      >
                        {opening ? <Loader2 className="animate-spin" /> : <TerminalSquare />}
                      </Button>
                    </TooltipTrigger>
                    <TooltipContent>
                      {s.cwd
                        ? "Open in Agentboard (resumes this session)"
                        : "No working directory recorded for this session"}
                    </TooltipContent>
                  </Tooltip>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

/** The embedded treemap/bar-chart report. Fetches only while this tab is the
 * active one (a `days` change on another tab is picked up on the next
 * activation); the report's rich categorical palette is its own self-contained
 * visual system, deliberately distinct from the app's grayscale charts. The
 * generated document is fixed-size (1200×800), so the iframe scrolls
 * internally rather than reflowing. */
function TreemapTab({
  days,
  nonce,
  active,
}: {
  days: string;
  nonce: number;
  active: boolean;
}) {
  const [html, setHtml] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const fetchedKey = useRef<string | null>(null);

  useEffect(() => {
    const key = `${days}:${nonce}`;
    if (!active || fetchedKey.current === key) return;
    fetchedKey.current = key;
    setLoading(true);
    void claudeSessionsTreemapHtml(Number(days)).then((h) => {
      setHtml(h);
      setLoading(false);
    });
  }, [active, days, nonce]);

  if (loading)
    return <p className="p-4 text-sm text-muted-foreground">Building treemap…</p>;
  if (!html)
    return (
      <p className="p-4 text-sm text-muted-foreground">
        No treemap to show — no sessions in this range.
      </p>
    );
  return (
    <iframe
      srcDoc={html}
      sandbox="allow-scripts"
      className="h-full w-full border-0"
      title="Claude Code usage treemap"
    />
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
    setSummary(await claudeSessionsSummary(Number(d)));
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
      void claudeSessionsSearch(Number(days), q).then((r) => setResults(r ?? []));
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
              <TabsTrigger value="treemap" className="justify-start gap-2 px-2 py-1.5">
                <Boxes className="size-4" />
                Treemap
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

              <TabsContent value="treemap" className="h-full">
                <TreemapTab days={days} nonce={refreshNonce} active={tab === "treemap"} />
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
