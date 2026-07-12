import { useEffect, useState } from "react";
import { Stethoscope } from "lucide-react";
import { invokeCmd, isTauri } from "@/lib/tauri";
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
