import { CircleAlert, Radio } from "lucide-react";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Empty, Panel } from "@/components/store-bits";
import { cn } from "@/lib/utils";
import { fmtAge, useStoreSnapshot, type McpCall } from "@/lib/data";
import { useNow } from "@/lib/now";

/**
 * MCP server — the live incoming-call log for the towles-tool MCP server
 * (`ttr mcp serve`). Each row is one handled JSON-RPC request the dispatcher
 * recorded into the store (method/tool, caller, duration, ok/error, age),
 * newest first. Read-only; the whole point is *seeing* who is calling the
 * server and how it answered. The dispatcher retains the newest few hundred
 * calls and the snapshot carries the newest 100.
 */
export function McpScreen() {
  const { snapshot, live } = useStoreSnapshot();
  const now = useNow();

  const calls = snapshot.mcpCalls;
  const errors = calls.filter((c) => !c.ok).length;
  // The caller identity is stamped from each session's `initialize`; the newest
  // call that carries one is the freshest "who is connected".
  const client = calls.find((c) => c.client)?.client;

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex shrink-0 flex-wrap items-center gap-x-4 gap-y-1 border-b px-5 py-3">
        <h2 className="flex items-center gap-2 font-heading text-lg font-semibold">
          <Radio className="size-5 text-muted-foreground" />
          MCP server
        </h2>
        <span className="font-mono text-xs text-muted-foreground">
          {calls.length} calls · {errors} failed
        </span>
        {client && (
          <span className="ml-auto font-mono text-xs text-muted-foreground">{client}</span>
        )}
      </div>

      {!live && (
        <div className="flex shrink-0 items-center gap-2 border-b bg-amber-500/10 px-5 py-1.5 text-xs text-amber-700 dark:text-amber-400">
          <CircleAlert className="size-3.5 shrink-0" />
          Not connected to the store — open this window in the Towles Tool app to see live calls.
        </div>
      )}

      <ScrollArea className="min-h-0 flex-1">
        <div className="flex flex-col gap-4 p-4">
          <Panel
            title="Incoming calls"
            note={`${calls.length}`}
            icon={<Radio className="size-4 text-muted-foreground" />}
          >
            {calls.length === 0 ? (
              <Empty>
                {live
                  ? "No MCP calls yet. Point a client at `ttr mcp serve` and they'll show up here."
                  : "Not connected yet."}
              </Empty>
            ) : (
              calls.map((call) => <CallRow key={call.id} call={call} now={now} />)
            )}
          </Panel>
        </div>
      </ScrollArea>
    </div>
  );
}

/**
 * One call-log row: a green/red status dot, the method (and tool for a
 * `tools/call`), the compacted args, then a right-aligned meta cluster of
 * duration and relative age. An error row surfaces its message beneath and gets
 * a red left edge so a failure reads at a glance.
 */
function CallRow({ call, now }: { call: McpCall; now: number }) {
  return (
    <div
      className={cn(
        "flex flex-col gap-0.5 border-l-2 border-transparent px-3 py-2",
        !call.ok && "border-l-red-500 bg-red-500/5",
      )}
    >
      <div className="flex items-center gap-2.5">
        <span
          className={cn("size-2 shrink-0 rounded-full", call.ok ? "bg-green-500" : "bg-red-500")}
        />
        <span className="font-mono text-xs text-foreground">
          {call.tool ?? call.method}
        </span>
        {call.tool && (
          <span className="font-mono text-[11px] text-muted-foreground/60">{call.method}</span>
        )}
        <div className="ml-auto flex shrink-0 items-center gap-3 font-mono text-[11px] text-muted-foreground">
          {call.durationMs !== undefined && <span>{call.durationMs}ms</span>}
          <span>{fmtAge(call.ts, now)}</span>
        </div>
      </div>
      {call.args && call.args !== "{}" && (
        <span className="truncate pl-[18px] font-mono text-[11px] text-muted-foreground/70">
          {call.args}
        </span>
      )}
      {!call.ok && call.error && (
        <span className="pl-[18px] font-mono text-[11px] text-red-600 dark:text-red-400">
          {call.error}
        </span>
      )}
    </div>
  );
}
