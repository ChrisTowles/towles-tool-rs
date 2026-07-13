import { CircleAlert, Copy, Plug, Radio } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Empty, Panel } from "@/components/store-bits";
import { cn } from "@/lib/utils";
import { fmtAge, useStoreSnapshot, type McpCall } from "@/lib/data";
import { useNow } from "@/lib/now";

/**
 * MCP server — the live incoming-call log for the towles-tool MCP server
 * (`tt mcp serve`). Each row is one handled JSON-RPC request the dispatcher
 * recorded into the store (method/tool, caller, duration, ok/error, age),
 * newest first. Read-only; the whole point is *seeing* who is calling the
 * server and how it answered. The dispatcher retains the newest few hundred
 * calls and the snapshot carries the newest 100. A setup panel carries the
 * copyable registration commands; it leads while no client has ever called.
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
          {calls.length === 0 && <SetupPanel />}
          <Panel
            title="Incoming calls"
            note={`${calls.length}`}
            icon={<Radio className="size-4 text-muted-foreground" />}
          >
            {calls.length === 0 ? (
              <Empty>
                {live
                  ? "No MCP calls yet. Register a client above and they'll show up here."
                  : "Not connected yet."}
              </Empty>
            ) : (
              calls.map((call) => <CallRow key={call.id} call={call} now={now} />)
            )}
          </Panel>
          {calls.length > 0 && <SetupPanel />}
        </div>
      </ScrollArea>
    </div>
  );
}

const CLAUDE_DESKTOP_JSON = `{
  "mcpServers": {
    "tt": { "command": "tt", "args": ["mcp", "serve"] }
  }
}`;

/**
 * How to point a client at the server. Leads while the call log is empty
 * (setup is the whole task then), drops below the log once traffic exists.
 * Every command assumes `tt` is on PATH; the server itself needs no port or
 * daemon — clients spawn `tt mcp serve` over stdio themselves.
 */
function SetupPanel() {
  return (
    <Panel title="Connect a client" icon={<Plug className="size-4 text-muted-foreground" />}>
      <SetupStep
        title="Claude Code (recommended)"
        detail="Registers the `tt` server at user scope and checks the rest of the Claude Code setup."
        command="tt install"
      />
      <SetupStep
        title="Claude Code, manual"
        detail="Just the MCP registration — what `tt install` runs for you."
        command="claude mcp add --scope user tt -- tt mcp serve"
      />
      <SetupStep
        title="Claude Desktop / any MCP client"
        detail="Stdio server config; for Claude Desktop merge it into claude_desktop_config.json."
        command={CLAUDE_DESKTOP_JSON}
        block
      />
      <p className="px-3 py-2 text-xs text-muted-foreground">
        The server exposes the local store (todos, issues, PRs, day brief, needs-you), live agent
        sessions, and <span className="font-mono">journal_append</span>. Verify with{" "}
        <span className="font-mono">claude mcp list</span> — calls appear above as they arrive.
      </p>
    </Panel>
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
  async function copy() {
    try {
      await navigator.clipboard.writeText(command);
      toast.success("Copied");
    } catch (e) {
      toast.error(String(e));
    }
  }

  return (
    <div className="flex flex-col gap-1.5 px-3 py-2.5">
      <div className="flex items-baseline gap-2">
        <span className="text-sm text-foreground">{title}</span>
        <span className="text-[11px] text-muted-foreground">{detail}</span>
      </div>
      <div className="flex items-start gap-1 rounded-md border bg-card px-3 py-1.5">
        <pre
          className={cn(
            "min-w-0 flex-1 self-center overflow-x-auto font-mono text-xs text-foreground",
            !block && "whitespace-nowrap",
          )}
        >
          {command}
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
