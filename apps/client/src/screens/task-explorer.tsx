import { useEffect, useState } from "react";
import { Activity, RefreshCw } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Card, Empty, StatTile } from "@/components/store-bits";
import { formatMemory } from "@/components/status-bar";
import { errorMessage, NotInTauri } from "@/lib/errors";
import { taskExplorerSnapshot, type ProcessGroup, type ProcessRow } from "@/lib/task-explorer";
import { cn } from "@/lib/utils";
import { useWorkspace } from "@/lib/workspace";
import { uiAction } from "@/lib/ui-action";

/**
 * Task Explorer — a live process view of what this app itself is running:
 * the `tt-app` process, then one group per embedded terminal (its shell and
 * everything that shell has spawned, per `task_explorer.rs`'s session-id
 * sweep). Auto-polls while this screen is the active tab, the same
 * "inherently live" expectation as Activity Monitor/htop; stops polling the
 * moment the tab isn't active rather than running in the background forever.
 * `formatMemory` is shared with the status bar, whose CPU/RAM readout is
 * this same total (see `status-bar.tsx`), so the two numbers can't drift
 * apart in formatting even if the underlying values ever did.
 */

const POLL_MS = 2500;

export function TaskExplorerScreen() {
  const { activeTab } = useWorkspace();
  const [groups, setGroups] = useState<ProcessGroup[] | null>(null);
  const [loading, setLoading] = useState(false);

  async function load() {
    setLoading(true);
    const r = await taskExplorerSnapshot();
    r.match({
      ok: setGroups,
      err: (e) => {
        if (!NotInTauri.is(e)) toast.error(`Could not read process snapshot: ${errorMessage(e)}`);
      },
    });
    setLoading(false);
  }

  useEffect(() => {
    if (activeTab !== "task-explorer") return;
    void load();
    const id = window.setInterval(() => void load(), POLL_MS);
    return () => window.clearInterval(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeTab]);

  function manualRefresh() {
    uiAction("task_explorer.refresh", "task-explorer");
    void load();
  }

  const processCount = groups?.reduce((n, g) => n + g.rows.length, 0) ?? 0;
  const totalCpu = groups?.reduce((n, g) => n + g.totalCpuPercent, 0) ?? 0;
  const totalMemory = groups?.reduce((n, g) => n + g.totalMemoryBytes, 0) ?? 0;
  const terminalCount = groups?.filter((g) => g.termId !== null).length ?? 0;

  return (
    <div className="flex h-full min-h-0 flex-col">
      <header className="flex items-center justify-between gap-2 border-b border-border bg-card px-4 py-3">
        <h2 className="flex items-center gap-2 font-heading text-lg font-semibold">
          <Activity className="size-5 text-muted-foreground" />
          Task Explorer
        </h2>
        <Button variant="outline" size="sm" onClick={manualRefresh} disabled={loading}>
          <RefreshCw className={cn("size-3.5", loading && "animate-spin")} />
          Refresh
        </Button>
      </header>

      <div className="grid shrink-0 grid-cols-2 gap-3 border-b border-border p-4 lg:grid-cols-4">
        <StatTile label="Processes" value={String(processCount)} />
        <StatTile label="Terminals" value={String(terminalCount)} />
        <StatTile label="CPU" value={`${totalCpu.toFixed(0)}%`} detail="all groups, all cores" />
        <StatTile label="Memory" value={formatMemory(totalMemory)} detail="all groups" />
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto p-4">
        {groups === null ? (
          <Empty>Loading…</Empty>
        ) : groups.length === 0 ? (
          <Empty>No processes found.</Empty>
        ) : (
          <div className="flex flex-col gap-4">
            {groups.map((g) => (
              <ProcessGroupCard key={g.termId ?? "app"} group={g} />
            ))}
            <TotalRow groups={groups} />
          </div>
        )}
      </div>
    </div>
  );
}

/** Grand total across every group, pinned below the per-group cards — the
 * same sum the status bar shows, spelled out per-group here so it's clear
 * what's being added together. */
function TotalRow({ groups }: { groups: ProcessGroup[] }) {
  const processCount = groups.reduce((n, g) => n + g.rows.length, 0);
  const cpuPercent = groups.reduce((n, g) => n + g.totalCpuPercent, 0);
  const memoryBytes = groups.reduce((n, g) => n + g.totalMemoryBytes, 0);
  return (
    <div className="flex items-center gap-2.5 rounded-lg border border-border bg-card px-3.5 py-2.5 text-sm font-medium">
      <span className="flex-1">
        Total · {groups.length} group{groups.length === 1 ? "" : "s"} · {processCount} process
        {processCount === 1 ? "" : "es"}
      </span>
      <span className="font-mono text-xs tabular-nums text-foreground">
        {cpuPercent.toFixed(0)}% CPU
      </span>
      <span className="font-mono text-xs tabular-nums text-foreground">
        {formatMemory(memoryBytes)}
      </span>
    </div>
  );
}

function ProcessGroupCard({ group }: { group: ProcessGroup }) {
  return (
    <Card
      title={group.label}
      note={`${group.rows.length} process${group.rows.length === 1 ? "" : "es"} · ${group.totalCpuPercent.toFixed(0)}% CPU · ${formatMemory(group.totalMemoryBytes)}`}
    >
      <div className="-mx-1.5 flex flex-col">
        <div className="flex items-center gap-2.5 px-1.5 py-1 font-mono text-[10.5px] uppercase tracking-wider text-muted-foreground">
          <span className="w-16 shrink-0">PID</span>
          <span className="flex-1">Name</span>
          <span className="w-24 shrink-0 text-right">Status</span>
          <span className="w-16 shrink-0 text-right">CPU</span>
          <span className="w-20 shrink-0 text-right">Memory</span>
        </div>
        {group.rows.map((row) => (
          <ProcessRowLine key={row.pid} row={row} />
        ))}
      </div>
    </Card>
  );
}

function ProcessRowLine({ row }: { row: ProcessRow }) {
  return (
    <div className="flex items-center gap-2.5 rounded-md px-1.5 py-1.5 text-sm hover:bg-accent/40">
      <span className="w-16 shrink-0 font-mono text-xs text-muted-foreground">{row.pid}</span>
      <span className="flex-1 truncate font-mono text-xs text-foreground" title={row.name}>
        {row.name}
      </span>
      <span className="w-24 shrink-0 text-right font-mono text-[11px] text-muted-foreground">
        {row.status}
      </span>
      <span className="w-16 shrink-0 text-right font-mono text-xs tabular-nums text-foreground">
        {row.cpuPercent.toFixed(0)}%
      </span>
      <span className="w-20 shrink-0 text-right font-mono text-xs tabular-nums text-foreground">
        {formatMemory(row.memoryBytes)}
      </span>
    </div>
  );
}
