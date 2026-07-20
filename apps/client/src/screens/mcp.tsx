import { memo, useCallback, useEffect, useMemo, useState } from "react";
import {
  BookOpen,
  CircleAlert,
  Copy,
  LayoutDashboard,
  Play,
  Plug,
  Radio,
  RefreshCw,
  Search,
} from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";
import { Textarea } from "@/components/ui/textarea";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Card, Empty, StatTile } from "@/components/store-bits";
import { cn } from "@/lib/utils";
import { fmtAge, useStoreSnapshot, type McpCall } from "@/lib/data";
import { useNow } from "@/lib/now";
import { errorMessage, NotInTauri } from "@/lib/errors";
import { invoke } from "@/lib/tauri";
import { uiAction } from "@/lib/ui-action";
import {
  McpStatusSchema,
  McpTestResultSchema,
  McpToolDocsSchema,
  type McpStatus,
  type McpTestResult,
  type McpToolDoc,
} from "@/lib/schemas/mcp";

/**
 * MCP server — a four-area console for the towles-tool MCP server, mirroring
 * the Claude Sessions layout (header · stat strip · vertical tabs): **Overview**
 * (is it being used, by whom, how often it fails), **Calls** (the live incoming
 * JSON-RPC log, searchable, with a per-call drill-down dialog), **Tools** (the
 * exposed contract, grouped by family), and **Setup** (how a client points at
 * it). Read-only; the whole point is *seeing* who is calling the server and how
 * it answered. The dispatcher retains the newest few hundred calls and the
 * snapshot carries the newest 100.
 */

/**
 * Loopback port the server listens on when settings don't override it. The
 * towles-tool-app plugin ships a static `.mcp.json` pointing here, so this
 * default has to stay stable — it is only the fallback for browser dev and for
 * the moment before {@link useMcpStatus} resolves.
 */
const DEFAULT_MCP_PORT = 8787;

const endpointFor = (port: number) => `http://127.0.0.1:${port}/mcp`;

/**
 * The real bind outcome, not an inference from call recency.
 *
 * These differ exactly where it matters: a healthy server nobody has called yet
 * reads as idle from the call log alone, and an instance that *lost* the bind
 * race (another slot got the port first) is serving nothing at all while still
 * showing that slot's calls. Only the backend knows which.
 */
function useMcpStatus() {
  const [status, setStatus] = useState<McpStatus | null>(null);
  useEffect(() => {
    void invoke<McpStatus>("mcp_status", {}, { schema: McpStatusSchema }).then((r) =>
      r.match({
        ok: setStatus,
        // `NotInTauri` is browser dev, not a failure — fall back to the
        // call-recency wording silently. Anything else is the backend actually
        // failing to answer, and this screen exists to report the bind outcome:
        // swallowing it would leave the header saying "Active" for a server that
        // may be serving nothing, which is precisely the inference this hook was
        // added to replace.
        err: (e) => {
          setStatus(null);
          if (!NotInTauri.is(e)) toast.error(`Could not read MCP status: ${errorMessage(e)}`);
        },
      }),
    );
  }, []);
  return status;
}

/** A call is "recent" — and the server therefore visibly in use — within this
 * window. Only drives the Overview/stat wording, never a data decision. */
const ACTIVE_WINDOW_MS = 5 * 60_000;

/**
 * What to call the server's state. Prefers {@link useMcpStatus}'s real bind
 * outcome for the reason spelled out there, and only falls back to call-recency
 * when there is no backend to ask (browser dev).
 */
function serverLabel(status: McpStatus | null, active: boolean): string {
  if (status) return status.serving ? "Serving" : "Not serving";
  return active ? "Active" : "Idle";
}

export function McpScreen() {
  const { snapshot, live } = useStoreSnapshot();
  const now = useNow();
  const { tools, reload } = useMcpToolDocs();
  const [tab, setTab] = useState("overview");
  const [selected, setSelected] = useState<McpCall | null>(null);

  const status = useMcpStatus();
  const port = status?.port ?? DEFAULT_MCP_PORT;
  const endpoint = endpointFor(port);

  const calls = snapshot.mcpCalls;
  const failed = useMemo(() => calls.filter((c) => !c.ok).length, [calls]);
  const clients = useMemo(() => clientNames(calls).length, [calls]);
  const newest = calls[0]?.ts;
  const active = newest !== undefined && now - newest < ACTIVE_WINDOW_MS;

  function switchTab(next: string) {
    setTab(next);
    uiAction("mcp.tab", "mcp", next);
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <header className="flex items-center justify-between gap-2 border-b border-border bg-card px-4 py-3">
        <h2 className="flex items-center gap-2 font-heading text-lg font-semibold">
          <Radio className="size-5 text-muted-foreground" />
          MCP server
        </h2>
        <div className="flex items-center gap-2">
          <span className="font-mono text-xs text-muted-foreground">{endpoint}</span>
          <Button
            variant="outline"
            size="sm"
            onClick={() => {
              uiAction("mcp.tools.refresh", "mcp");
              reload();
            }}
          >
            <RefreshCw className="size-3.5" />
            Refresh
          </Button>
        </div>
      </header>

      {!live && (
        <div className="flex shrink-0 items-center gap-2 border-b border-border bg-amber-500/10 px-4 py-1.5 text-xs text-amber-700 dark:text-amber-400">
          <CircleAlert className="size-3.5 shrink-0" />
          Not connected to the store — open this window in the Towles Tool app to see live calls.
        </div>
      )}

      <div className="grid shrink-0 grid-cols-2 gap-3 border-b border-border p-4 lg:grid-cols-4">
        <StatTile
          label="Server"
          value={serverLabel(status, active)}
          detail={
            status && !status.serving ? "another instance holds the port" : `127.0.0.1:${port}`
          }
        />
        <StatTile
          label="Calls"
          value={String(calls.length)}
          detail={newest === undefined ? "none yet" : `last ${fmtAge(newest, now)}`}
        />
        <StatTile
          label="Failed"
          value={String(failed)}
          detail={
            calls.length > 0
              ? `${Math.round((failed / calls.length) * 100)}% error rate`
              : undefined
          }
        />
        <StatTile label="Clients" value={String(clients)} detail="seen in this log" />
      </div>

      <Tabs
        orientation="vertical"
        value={tab}
        onValueChange={switchTab}
        className="min-h-0 flex-1 gap-0"
      >
        <TabsList
          variant="line"
          className="h-full w-44 shrink-0 items-stretch gap-1 rounded-none border-r border-border bg-card p-2"
        >
          <TabsTrigger value="overview" className="justify-start gap-2 px-2 py-1.5">
            <LayoutDashboard className="size-4" />
            Overview
          </TabsTrigger>
          <TabsTrigger value="calls" className="justify-start gap-2 px-2 py-1.5">
            <Radio className="size-4" />
            Calls
          </TabsTrigger>
          <TabsTrigger value="tools" className="justify-start gap-2 px-2 py-1.5">
            <BookOpen className="size-4" />
            Tools
          </TabsTrigger>
          <TabsTrigger value="setup" className="justify-start gap-2 px-2 py-1.5">
            <Plug className="size-4" />
            Setup
          </TabsTrigger>
        </TabsList>

        <div className="min-h-0 flex-1 overflow-y-auto">
          <TabsContent value="overview" className="p-4">
            <OverviewTab
              calls={calls}
              tools={tools}
              now={now}
              onOpenSetup={() => switchTab("setup")}
            />
          </TabsContent>

          <TabsContent value="calls" className="p-4">
            <CallsTab calls={calls} now={now} live={live} onSelect={setSelected} />
          </TabsContent>

          <TabsContent value="tools" className="p-4">
            <ToolsTab tools={tools} />
          </TabsContent>

          <TabsContent value="setup" className="p-4">
            <SetupTab endpoint={endpoint} port={port} />
          </TabsContent>
        </div>
      </Tabs>

      <CallDialog call={selected} now={now} onClose={() => setSelected(null)} />
    </div>
  );
}

/**
 * Fetches the MCP tool list from the same contract the server answers
 * `tools/list` with (`tt_mcp::tool_definitions`, via the `mcp_tool_docs`
 * command) — this is what keeps the Tools tab from ever drifting out of sync
 * with what the server actually exposes. `null` while loading or outside the
 * app; `reload` re-fetches after the tool surface changes.
 */
function useMcpToolDocs(): { tools: McpToolDoc[] | null; reload: () => void } {
  const [tools, setTools] = useState<McpToolDoc[] | null>(null);
  const [nonce, setNonce] = useState(0);

  useEffect(() => {
    void invoke<McpToolDoc[]>("mcp_tool_docs", {}, { schema: McpToolDocsSchema }).then((docs) =>
      setTools(docs.unwrapOr(null)),
    );
  }, [nonce]);

  return { tools, reload: () => setNonce((n) => n + 1) };
}

/**
 * One settled telemetry event per search, never one per keystroke — continuous
 * input is explicitly excluded from the event log (see the root CLAUDE.md).
 */
function useSearchTelemetry(query: string, action: string) {
  useEffect(() => {
    if (!query.trim()) return;
    const t = setTimeout(() => uiAction(action, "mcp"), 400);
    return () => clearTimeout(t);
  }, [query, action]);
}

/** Distinct clients that identified themselves on `initialize`. */
function clientNames(calls: McpCall[]): string[] {
  return [...new Set(calls.map((c) => c.client).filter((c): c is string => !!c))];
}

// ── Overview ────────────────────────────────────────────────────────────────

/** Per-tool call counts, busiest first — the "what is actually being used"
 * answer the tool list alone can't give. */
function toolUsage(calls: McpCall[]): { name: string; total: number; failed: number }[] {
  const by = new Map<string, { name: string; total: number; failed: number }>();
  for (const c of calls) {
    const name = c.tool ?? c.method;
    const row = by.get(name) ?? { name, total: 0, failed: 0 };
    row.total += 1;
    if (!c.ok) row.failed += 1;
    by.set(name, row);
  }
  return [...by.values()].toSorted((a, b) => b.total - a.total);
}

function OverviewTab({
  calls,
  tools,
  now,
  onOpenSetup,
}: {
  calls: McpCall[];
  tools: McpToolDoc[] | null;
  now: number;
  onOpenSetup: () => void;
}) {
  const usage = useMemo(() => toolUsage(calls), [calls]);
  const max = Math.max(1, ...usage.map((u) => u.total));
  const clients = useMemo(() => clientNames(calls), [calls]);
  const recent = calls.slice(0, 6);

  if (calls.length === 0) {
    return (
      <Card title="No calls yet">
        <Empty inline>
          Nothing has called this server. The towles-tool-app plugin registers it automatically —
          check the Setup tab if a client isn&apos;t connecting.
        </Empty>
        <Button variant="outline" size="sm" className="mt-3" onClick={onOpenSetup}>
          <Plug className="size-3.5" />
          Setup
        </Button>
      </Card>
    );
  }

  return (
    <div className="flex flex-col gap-4">
      <div className="grid gap-4 md:grid-cols-2">
        <Card title="Most-used tools" note={tools ? `${tools.length} exposed` : undefined}>
          <div className="flex flex-col gap-1.5">
            {usage.slice(0, 8).map((u) => (
              <div key={u.name} className="flex items-center gap-2 text-sm">
                <span className="w-40 truncate font-mono text-xs text-foreground" title={u.name}>
                  {u.name}
                </span>
                <div className="h-2 flex-1 overflow-hidden rounded-full bg-muted">
                  <div
                    className={cn(
                      "h-full rounded-full",
                      u.failed > 0 ? "bg-amber-500" : "bg-violet-500",
                    )}
                    style={{ width: `${Math.max(2, (u.total / max) * 100)}%` }}
                  />
                </div>
                <span className="w-14 shrink-0 text-right font-mono text-xs text-muted-foreground">
                  {u.total}
                  {u.failed > 0 && <span className="text-red-500"> ·{u.failed}</span>}
                </span>
              </div>
            ))}
          </div>
        </Card>

        <Card title="Callers" note={`${clients.length}`}>
          {clients.length === 0 ? (
            <Empty inline>No client identified itself on initialize.</Empty>
          ) : (
            <div className="flex flex-col gap-1.5">
              {clients.map((c) => (
                <span key={c} className="font-mono text-xs text-foreground">
                  {c}
                </span>
              ))}
            </div>
          )}
        </Card>
      </div>

      <Card title="Recent activity" note={`newest ${fmtAge(calls[0].ts, now)}`}>
        <div className="-mx-1 flex flex-col">
          {recent.map((call) => (
            <div key={call.id} className="flex items-center gap-2.5 px-1 py-1">
              <span
                className={cn(
                  "size-2 shrink-0 rounded-full",
                  call.ok ? "bg-green-500" : "bg-red-500",
                )}
              />
              <span className="font-mono text-xs text-foreground">{call.tool ?? call.method}</span>
              <span className="ml-auto font-mono text-[11px] text-muted-foreground">
                {fmtAge(call.ts, now)}
              </span>
            </div>
          ))}
        </div>
      </Card>
    </div>
  );
}

// ── Calls ───────────────────────────────────────────────────────────────────

type CallFilter = "all" | "errors";

/** Everything about a call the search box looks at, lowercased once. Built per
 * call when the log changes, not per keystroke and not per clock tick — the
 * screen re-renders every second, and this is the only per-row string work. */
function searchHaystack(call: McpCall): string {
  return [call.method, call.tool, call.args, call.client, call.error]
    .filter(Boolean)
    .join(" ")
    .toLowerCase();
}

function CallsTab({
  calls,
  now,
  live,
  onSelect,
}: {
  calls: McpCall[];
  now: number;
  live: boolean;
  onSelect: (call: McpCall) => void;
}) {
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<CallFilter>("all");

  useSearchTelemetry(query, "mcp.calls.search");

  const indexed = useMemo(
    () => calls.map((call) => ({ call, hay: searchHaystack(call) })),
    [calls],
  );

  const q = query.trim().toLowerCase();
  const shown = useMemo(
    () =>
      indexed
        .filter(
          ({ call, hay }) => (filter === "errors" ? !call.ok : true) && (!q || hay.includes(q)),
        )
        .map(({ call }) => call),
    [indexed, filter, q],
  );

  // The age string is computed here, not inside the row, so a tick that doesn't
  // change a row's coarse age ("2m") produces identical props and `CallRow`'s
  // `memo` skips it entirely.
  const rows = useMemo(
    () => shown.map((call) => ({ call, age: fmtAge(call.ts, now) })),
    [shown, now],
  );

  const open = useCallback(
    (call: McpCall) => {
      uiAction("mcp.call.open", "mcp", call.tool ?? call.method);
      onSelect(call);
    },
    [onSelect],
  );

  return (
    <Card title="Incoming calls" note={`${shown.length} of ${calls.length}`}>
      <div className="mb-3 flex items-center gap-2">
        <div className="relative w-72">
          <Search className="absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
          <Input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search tool, args, client…"
            className="h-8 pl-8 text-sm"
          />
        </div>
        <Select
          value={filter}
          onValueChange={(v) => {
            setFilter(v as CallFilter);
            uiAction("mcp.calls.filter", "mcp", v);
          }}
        >
          <SelectTrigger className="h-8 w-32">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">All calls</SelectItem>
            <SelectItem value="errors">Errors only</SelectItem>
          </SelectContent>
        </Select>
      </div>

      {shown.length === 0 ? (
        <Empty inline>
          {calls.length === 0
            ? live
              ? "No MCP calls yet. See the Setup tab to point a client here."
              : "Not connected yet."
            : "No calls match."}
        </Empty>
      ) : (
        <div className="-mx-1.5 flex flex-col">
          {rows.map(({ call, age }) => (
            <CallRow key={call.id} call={call} age={age} onSelect={open} />
          ))}
        </div>
      )}
    </Card>
  );
}

/**
 * One call-log row: a green/red status dot, the method (and tool for a
 * `tools/call`), the compacted args, then a right-aligned meta cluster of
 * duration and relative age. An error row surfaces its message beneath and gets
 * a red left edge so a failure reads at a glance. The whole row opens the
 * drill-down — it holds no interactive children, so a real `<button>` is safe
 * here and gives the largest possible target.
 *
 * `memo`'d, and taking `age` pre-rendered rather than `now`, because the screen
 * re-renders on a one-second clock and the log holds a hundred of these.
 */
const CallRow = memo(function CallRow({
  call,
  age,
  onSelect,
}: {
  call: McpCall;
  age: string;
  onSelect: (call: McpCall) => void;
}) {
  return (
    <button
      type="button"
      onClick={() => onSelect(call)}
      className={cn(
        "flex w-full flex-col gap-0.5 rounded-md border-l-2 border-transparent px-3 py-2 text-left hover:bg-accent/50",
        !call.ok && "border-l-red-500 bg-red-500/5",
      )}
    >
      <div className="flex w-full items-center gap-2.5">
        <span
          className={cn("size-2 shrink-0 rounded-full", call.ok ? "bg-green-500" : "bg-red-500")}
        />
        <span className="font-mono text-xs text-foreground">{call.tool ?? call.method}</span>
        {call.tool && (
          <span className="font-mono text-[11px] text-muted-foreground/60">{call.method}</span>
        )}
        <div className="ml-auto flex shrink-0 items-center gap-3 font-mono text-[11px] text-muted-foreground">
          {call.durationMs !== undefined && <span>{call.durationMs}ms</span>}
          <span>{age}</span>
        </div>
      </div>
      {call.args && call.args !== "{}" && (
        <span className="w-full truncate pl-[18px] font-mono text-[11px] text-muted-foreground/70">
          {call.args}
        </span>
      )}
      {!call.ok && call.error && (
        <span className="w-full truncate pl-[18px] font-mono text-[11px] text-red-600 dark:text-red-400">
          {call.error}
        </span>
      )}
    </button>
  );
});

/** Full detail for one logged call — the args and error message in full, which
 * the row can only truncate. */
function CallDialog({
  call,
  now,
  onClose,
}: {
  call: McpCall | null;
  now: number;
  onClose: () => void;
}) {
  return (
    <Dialog open={!!call} onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle className="pr-6 font-mono text-base">
            {call?.tool ?? call?.method}
          </DialogTitle>
          <DialogDescription>
            {call?.method}
            {call?.client ? ` · ${call.client}` : ""}
            {call?.durationMs !== undefined ? ` · ${call.durationMs}ms` : ""}
            {call ? ` · ${fmtAge(call.ts, now)}` : ""}
          </DialogDescription>
        </DialogHeader>

        {call && (
          <div className="flex flex-col gap-4">
            <section>
              <h4 className="mb-2 text-[10.5px] font-medium uppercase tracking-wider text-muted-foreground">
                Result
              </h4>
              <div className="flex items-center gap-2 text-sm">
                <span
                  className={cn(
                    "size-2 shrink-0 rounded-full",
                    call.ok ? "bg-green-500" : "bg-red-500",
                  )}
                />
                <span className="text-foreground">{call.ok ? "Succeeded" : "Failed"}</span>
              </div>
              {!call.ok && call.error && (
                <pre className="mt-2 overflow-x-auto rounded-md border border-border bg-muted/40 p-2.5 font-mono text-xs whitespace-pre-wrap text-red-600 dark:text-red-400">
                  {call.error}
                </pre>
              )}
            </section>

            <section>
              <h4 className="mb-2 text-[10.5px] font-medium uppercase tracking-wider text-muted-foreground">
                Arguments
              </h4>
              {call.args && call.args !== "{}" ? (
                <pre className="overflow-x-auto rounded-md border border-border bg-muted/40 p-2.5 font-mono text-xs whitespace-pre-wrap text-foreground">
                  {call.args}
                </pre>
              ) : (
                <Empty inline>No arguments.</Empty>
              )}
            </section>
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}

// ── Tools ───────────────────────────────────────────────────────────────────

/** Tools are named `<family>_<verb>` (`task_list`, `calendar_next`), so the
 * prefix groups them without hardcoding any tool name — a family added on the
 * Rust side shows up here on its own. */
function toolFamily(name: string): string {
  const i = name.indexOf("_");
  return i === -1 ? name : name.slice(0, i);
}

function groupTools(tools: McpToolDoc[]): { family: string; tools: McpToolDoc[] }[] {
  const groups: { family: string; tools: McpToolDoc[] }[] = [];
  for (const tool of tools) {
    const family = toolFamily(tool.name);
    const group = groups.find((g) => g.family === family);
    if (group) group.tools.push(tool);
    else groups.push({ family, tools: [tool] });
  }
  return groups;
}

/**
 * Auto-generated tool documentation, straight from the MCP contract — never
 * hand-maintained, so it can't fall out of sync as tools are added or changed.
 */
function ToolsTab({ tools }: { tools: McpToolDoc[] | null }) {
  const [query, setQuery] = useState("");
  const [testing, setTesting] = useState<McpToolDoc | null>(null);

  useSearchTelemetry(query, "mcp.tools.search");

  const q = query.trim().toLowerCase();
  const groups = useMemo(() => {
    if (tools === null) return [];
    const shown = q
      ? tools.filter((t) => `${t.name} ${t.description}`.toLowerCase().includes(q))
      : tools;
    return groupTools(shown);
  }, [tools, q]);

  if (tools === null) {
    return (
      <Card title="Tools">
        <Empty inline>Not available outside the app.</Empty>
      </Card>
    );
  }

  return (
    <div className="flex flex-col gap-4">
      <div className="relative w-72">
        <Search className="absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
        <Input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search tools…"
          className="h-8 pl-8 text-sm"
        />
      </div>

      {groups.length === 0 ? (
        <Card title="Tools">
          <Empty inline>No tools match.</Empty>
        </Card>
      ) : (
        groups.map((group) => (
          <Card key={group.family} title={group.family} note={`${group.tools.length}`}>
            <div className="-mx-1.5 flex flex-col">
              {group.tools.map((tool) => (
                <ToolRow
                  key={tool.name}
                  tool={tool}
                  actions={
                    <Button
                      variant="outline"
                      size="sm"
                      className="h-7 text-xs"
                      onClick={() => {
                        uiAction("mcp.tool.test_open", "mcp", tool.name);
                        setTesting(tool);
                      }}
                    >
                      <Play className="size-3" />
                      Test
                    </Button>
                  }
                />
              ))}
            </div>
          </Card>
        ))
      )}

      <ToolTesterDialog tool={testing} onClose={() => setTesting(null)} />
    </div>
  );
}

/**
 * Whether a tool writes — read off the MCP contract, never off its wording.
 *
 * The server emits the spec's own `annotations.readOnlyHint: false` on the
 * tools that mutate and ships no `annotations` block at all otherwise, so this
 * is a declaration by the tool's author rather than a guess at what the
 * description's prose implies. That distinction is load-bearing now: with the
 * capability gate gone, the warning this drives is the only signal a human gets
 * before a write, and a description reworded on the Rust side must not be able
 * to silently turn it off.
 *
 * Strictly `=== false` — an absent hint means "no claim made", which is not the
 * same as "declared read-only", and neither is a reason to warn.
 */
function isMutating(tool: McpToolDoc): boolean {
  return tool.annotations?.readOnlyHint === false;
}

/**
 * Fire a real request at the MCP endpoint and show exactly what came back.
 *
 * The request is issued **from Rust**, not by `fetch` here, and that is not an
 * implementation convenience: the webview is a browser context, so its `fetch`
 * would carry an `Origin` header and the server would refuse it — correctly.
 * The app cannot call its own endpoint from the frontend, which is the defense
 * working. `mcp_test_call` sends the request the way a real MCP client does.
 *
 * The "as a browser would" toggle deliberately re-sends *with* an `Origin`
 * header so the refusal is something you can watch happen rather than take on
 * faith. With no capability gate left, that check is the whole guard on writes,
 * so making it visible is worth a button.
 */
function ToolTesterDialog({ tool, onClose }: { tool: McpToolDoc | null; onClose: () => void }) {
  const [args, setArgs] = useState("{}");
  const [asBrowser, setAsBrowser] = useState(false);
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<McpTestResult | null>(null);

  // Reset per tool, and seed the editor with a skeleton of its required args so
  // the common case is "fill in the values", not "recall the schema".
  // `running` resets too: without it, switching tools mid-call leaves the new
  // tool's Run button permanently disabled reading "Running…".
  useEffect(() => {
    if (!tool) return;
    setArgs(skeletonArgs(tool));
    setResult(null);
    setAsBrowser(false);
    setRunning(false);
  }, [tool]);

  if (!tool) return null;

  async function run() {
    if (!tool) return;
    let parsed: unknown;
    try {
      parsed = JSON.parse(args);
    } catch (e) {
      toast.error(`Arguments must be valid JSON: ${errorMessage(e)}`);
      return;
    }
    // A slow call whose tool has since been switched away must not paint its
    // result into the new tool's pane — the panes are identical, so a
    // `calendar_set` status would read as `task_list`'s.
    const issuedFor = tool.name;
    setRunning(true);
    // Both facts, always: which tool ran, and whether it was the browser
    // simulation. Encoding one *or* the other lost the tool name for exactly
    // the runs where the security boundary was being exercised.
    uiAction("mcp.tool.test_run", "mcp", asBrowser ? `${tool.name} as-browser` : tool.name);
    const body = JSON.stringify({
      jsonrpc: "2.0",
      id: 1,
      method: "tools/call",
      params: { name: tool.name, arguments: parsed },
    });
    const res = await invoke<McpTestResult>(
      "mcp_test_call",
      { body, simulateBrowserOrigin: asBrowser },
      { schema: McpTestResultSchema },
    );
    // `tool` is the current prop by the time this resolves; if it changed, this
    // reply belongs to a pane nobody is looking at.
    if (issuedFor !== tool?.name) return;
    setRunning(false);
    res.match({
      ok: setResult,
      err: (e) => toast.error(errorMessage(e)),
    });
  }

  const mutating = isMutating(tool);

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="max-w-2xl">
        <DialogHeader>
          <DialogTitle className="font-mono text-sm">{tool.name}</DialogTitle>
          <DialogDescription>{tool.description}</DialogDescription>
        </DialogHeader>

        {mutating && (
          <div className="flex items-start gap-2 rounded-md border border-amber-500/40 bg-amber-500/10 px-3 py-2 text-xs text-amber-700 dark:text-amber-400">
            <CircleAlert className="mt-0.5 size-3.5 shrink-0" />
            <span>This tool writes real data. Running it changes your board or calendar.</span>
          </div>
        )}

        <div className="flex flex-col gap-2">
          <label htmlFor="mcp-test-args" className="text-xs font-medium text-muted-foreground">
            Arguments (JSON)
          </label>
          <Textarea
            id="mcp-test-args"
            value={args}
            onChange={(e) => setArgs(e.target.value)}
            spellCheck={false}
            rows={6}
            className="font-mono text-xs"
          />
        </div>

        <label
          htmlFor="mcp-test-as-browser"
          className="flex items-center gap-2 text-xs text-muted-foreground"
        >
          <Checkbox
            id="mcp-test-as-browser"
            checked={asBrowser}
            onCheckedChange={(v) => setAsBrowser(v === true)}
          />
          {/* One flex child, not three: bare text nodes beside the inline
              <span> would each become a flex item and lay out as columns. */}
          <span>
            Send as a browser would (adds an <span className="font-mono">Origin</span> header) — the
            server must refuse this
          </span>
        </label>

        <div className="flex items-center gap-2">
          <Button size="sm" onClick={() => void run()} disabled={running}>
            <Play className="size-3.5" />
            {running ? "Running…" : "Run tool"}
          </Button>
          {result && (
            <span className="text-xs text-muted-foreground">
              HTTP {result.status} · {result.durationMs}ms
            </span>
          )}
        </div>

        {result && (
          <div className="flex flex-col gap-1.5">
            <div
              className={cn(
                "flex items-center gap-2 rounded-md px-3 py-2 text-xs",
                result.status === 200
                  ? "bg-emerald-500/10 text-emerald-700 dark:text-emerald-400"
                  : "bg-red-500/10 text-red-700 dark:text-red-400",
              )}
            >
              {result.status === 200
                ? "Accepted — the request reached the dispatcher."
                : result.sentOrigin
                  ? "Refused, as it should be: an Origin header means a web page sent it."
                  : "Refused before reaching the dispatcher."}
            </div>
            <pre className="max-h-64 overflow-auto rounded-md border border-border bg-muted/40 p-2 font-mono text-[11px] whitespace-pre-wrap">
              {prettyJson(result.body)}
            </pre>
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}

/** A starting `arguments` object: every required property, with a typed
 * placeholder. Optional properties are left out — they're discoverable in the
 * row above, and a pre-filled optional is a value you didn't mean to send. */
function skeletonArgs(tool: McpToolDoc): string {
  const required = tool.inputSchema.required ?? [];
  if (required.length === 0) return "{}";
  const seed: Record<string, unknown> = {};
  for (const name of required) {
    const type = tool.inputSchema.properties[name]?.type;
    seed[name] = type === "integer" || type === "number" ? 0 : type === "array" ? [] : "";
  }
  return JSON.stringify(seed, null, 2);
}

/** Pretty-print a JSON body, falling back to the raw text for the plain-text
 * refusal bodies the transport returns. */
function prettyJson(body: string): string {
  try {
    return JSON.stringify(JSON.parse(body), null, 2);
  } catch {
    return body;
  }
}

/**
 * One tool's docs: name, description, and its parameters (required ones
 * unmarked, optional ones suffixed `?`) derived straight from its JSON Schema.
 *
 * The `actions` slot is a sibling of the identity cluster, not a child of it,
 * so a per-tool control (e.g. a "test this tool" button) drops in without
 * nesting interactive elements — see apps/client/CLAUDE.md's clickable-rows
 * rule.
 */
function ToolRow({ tool, actions }: { tool: McpToolDoc; actions?: React.ReactNode }) {
  const params = Object.entries(tool.inputSchema.properties);
  return (
    <div className="flex items-start gap-2 rounded-md px-3 py-2.5 hover:bg-accent/50">
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <span className="font-mono text-xs text-foreground">{tool.name}</span>
        <p className="text-xs text-muted-foreground">{tool.description}</p>
        {params.length > 0 && (
          <div className="flex flex-wrap gap-x-3 gap-y-0.5 pt-0.5">
            {params.map(([name, schema]) => {
              const required = tool.inputSchema.required.includes(name);
              return (
                <span
                  key={name}
                  className="font-mono text-[11px] text-muted-foreground/80"
                  title={schema.description}
                >
                  {name}
                  {required ? "" : "?"}
                  {schema.type && <span className="text-muted-foreground/50">:{schema.type}</span>}
                </span>
              );
            })}
          </div>
        )}
      </div>
      {actions && <div className="flex shrink-0 items-center gap-1">{actions}</div>}
    </div>
  );
}

// ── Setup ───────────────────────────────────────────────────────────────────

const pluginMcpJson = (endpoint: string) => `{
  "mcpServers": {
    "towles-tool": { "type": "http", "url": "${endpoint}" }
  }
}`;

/**
 * How a client reaches the server. It listens on loopback HTTP and the
 * towles-tool-app plugin ships the `.mcp.json` that points at it, so the
 * common case is "enable the plugin" — the manual commands are the fallback.
 * No token: the listener is bound to 127.0.0.1.
 */
function SetupTab({ endpoint, port }: { endpoint: string; port: number }) {
  return (
    <div className="flex flex-col gap-4">
      <Card title="Endpoint">
        <p className="mb-3 text-xs text-muted-foreground">
          The app serves MCP over loopback HTTP. Nothing to start by hand and no token — the
          listener is bound to 127.0.0.1.
        </p>
        <CopyBlock value={endpoint} />
        <p className="mt-2 text-[11px] text-muted-foreground">
          Port defaults to {port}; set <span className="font-mono">mcp.port</span> in the settings
          file to change it.
        </p>
      </Card>

      <Card title="Connect a client">
        <SetupStep
          title="Claude Code (recommended)"
          detail="Enable the towles-tool-app plugin — it ships the .mcp.json below."
          command="claude plugin marketplace add ChrisTowles/towles-tool-rs && claude plugin enable towles-tool-app@towles-tool"
        />
        <SetupStep
          title="Claude Code, manual"
          detail="Registers the HTTP server at user scope, without the plugin."
          command={`claude mcp add --scope user --transport http towles-tool ${endpoint}`}
        />
        <SetupStep
          title="Any MCP client"
          detail="What the plugin ships; merge it into your client's config."
          command={pluginMcpJson(endpoint)}
          block
        />
        <p className="px-3 py-2 text-xs text-muted-foreground">
          Verify with <span className="font-mono">claude mcp list</span> — calls appear on the Calls
          tab as they arrive.
        </p>
      </Card>
    </div>
  );
}

/** One setup option: name + one-line context, then the copyable command. */
function SetupStep({
  title,
  detail,
  command,
  block = false,
}: {
  title: string;
  detail: string;
  command: string;
  block?: boolean;
}) {
  return (
    <div className="flex flex-col gap-1.5 px-3 py-2.5">
      <div className="flex items-baseline gap-2">
        <span className="text-sm text-foreground">{title}</span>
        <span className="text-[11px] text-muted-foreground">{detail}</span>
      </div>
      <CopyBlock value={command} block={block} />
    </div>
  );
}

/** A copyable command/URL. */
function CopyBlock({ value, block = false }: { value: string; block?: boolean }) {
  async function copy() {
    uiAction("mcp.setup.copy", "mcp");
    try {
      await navigator.clipboard.writeText(value);
      toast.success("Copied");
    } catch (e) {
      toast.error(errorMessage(e));
    }
  }

  return (
    <div className="flex items-start gap-1 rounded-md border border-border bg-background px-3 py-1.5">
      <pre
        className={cn(
          "min-w-0 flex-1 self-center overflow-x-auto font-mono text-xs text-foreground",
          !block && "whitespace-nowrap",
        )}
      >
        {value}
      </pre>
      <Button
        variant="ghost"
        size="icon"
        className="size-6 shrink-0 text-muted-foreground"
        onClick={copy}
        title="Copy"
      >
        <Copy className="size-3.5" />
      </Button>
    </div>
  );
}
