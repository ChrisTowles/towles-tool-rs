import { useEffect, useState } from "react";
import { Stethoscope } from "lucide-react";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { invokeCmd, isTauri } from "@/lib/tauri";
import {
  collectorHealth,
  type CollectorHealth,
  type CollectorState,
} from "@/lib/collector-health";
import { fmtAge, useStoreSnapshot } from "@/lib/data";
import { useNow } from "@/lib/now";
import { cn } from "@/lib/utils";
import { useAppVersion } from "@/lib/version";
import { useWorkspace } from "@/lib/workspace";

/** Mirror of the `app_resource_usage` command's payload. */
type ResourceUsage = { cpuPercent: number; memoryBytes: number };

const USAGE_POLL_MS = 5000;

function formatMemory(bytes: number): string {
  const mb = bytes / (1024 * 1024);
  return mb >= 1024 ? `${(mb / 1024).toFixed(1)} GB` : `${Math.round(mb)} MB`;
}

/**
 * Passive CPU/RAM readout for the app's own process (#78). Polls the Rust
 * sampler on an interval; renders nothing in browser dev or until the first
 * sample lands.
 */
function useResourceUsage(): ResourceUsage | null {
  const [usage, setUsage] = useState<ResourceUsage | null>(null);
  useEffect(() => {
    if (!isTauri()) return;
    let cancelled = false;
    const tick = async () => {
      const u = await invokeCmd<ResourceUsage>("app_resource_usage");
      if (!cancelled && u) setUsage(u);
    };
    tick();
    const id = window.setInterval(tick, USAGE_POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, []);
  return usage;
}

/** Dot color per health state — subtle fills paired with dark: variants. */
const STATE_DOT: Record<CollectorState, string> = {
  fresh: "bg-green-500/70 dark:bg-green-400/70",
  stale: "bg-amber-500/80 dark:bg-amber-400/80",
  failing: "bg-red-500 dark:bg-red-400",
  "never-ran": "bg-muted-foreground/30 dark:bg-muted-foreground/30",
};

const STATE_WORD: Record<CollectorState, string> = {
  fresh: "up to date",
  stale: "stale",
  failing: "failing",
  "never-ran": "never ran",
};

/** One muted dot per collector with a health tooltip (name, age, ok/fail). */
function CollectorHealthDot({ health, now }: { health: CollectorHealth; now: number }) {
  const { label, state, run } = health;
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span
          className={cn("size-1.5 rounded-full", STATE_DOT[state])}
          aria-label={`${label}: ${STATE_WORD[state]}`}
        />
      </TooltipTrigger>
      <TooltipContent className="flex flex-col gap-0.5">
        <span className="font-medium">
          {label} · {STATE_WORD[state]}
        </span>
        {run ? (
          <span className="text-muted-foreground">
            {run.ok ? "ran" : "failed"} {fmtAge(run.ranAt, now)}
            {run.message ? ` · ${run.message}` : ""}
          </span>
        ) : (
          <span className="text-muted-foreground">no run recorded yet</span>
        )}
      </TooltipContent>
    </Tooltip>
  );
}

/**
 * Always-on collector health: a compact cluster of dots so a focused user sees
 * `gh` auth expiring (a red dot) before PRs quietly go missing. Classification
 * lives in the pure {@link collectorHealth}; this only paints it.
 */
function CollectorHealthCluster() {
  const { snapshot } = useStoreSnapshot();
  const now = useNow();
  const health = collectorHealth(snapshot.runs, now);
  return (
    <div className="flex items-center gap-1" title="Collector health">
      {health.map((h) => (
        <CollectorHealthDot key={h.key} health={h} now={now} />
      ))}
    </div>
  );
}

export function StatusBar() {
  const { openTab } = useWorkspace();
  const usage = useResourceUsage();
  const version = useAppVersion();

  return (
    <footer className="flex h-7 shrink-0 items-center justify-between border-t px-3 text-xs text-muted-foreground">
      <button
        className="flex items-center gap-1.5 hover:text-foreground"
        onClick={() => openTab("doctor")}
      >
        <Stethoscope className="size-3.5" />
        Doctor
      </button>
      <div className="flex items-center gap-3">
        <CollectorHealthCluster />
        {usage && (
          <span
            className="tabular-nums"
            title="towles-tool process CPU / memory"
          >
            {usage.cpuPercent.toFixed(0)}% CPU ·{" "}
            {formatMemory(usage.memoryBytes)}
          </span>
        )}
        <span
          className={
            isTauri()
              ? undefined
              : "font-medium text-amber-600 dark:text-amber-500"
          }
        >
          {isTauri() ? "Tauri shell" : "browser"}
        </span>
        <span>{version}</span>
      </div>
    </footer>
  );
}
