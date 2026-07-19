import { Globe, Play, Server, SquareTerminal } from "lucide-react";
import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import type { FolderData, SessionActions } from "@/lib/agentboard";
import { errorMessage } from "@/lib/errors";
import {
  devServerUrl,
  launchAction,
  launchCommand,
  launchConfigs,
  type LaunchConfigStatus,
} from "@/lib/launch";
import { openExternalUrl } from "@/lib/open-url";
import { cn } from "@/lib/utils";

/** How often the open popover re-probes ports/panes — long enough to stay
 * cheap, short enough that a just-launched server's dot flips green while
 * you watch. */
const REFRESH_MS = 3000;

/** The dev-servers affordance for a checkout with a Claude Desktop
 * `.claude/launch.json` (callers gate on `folder.hasLaunchConfig`): a
 * folder-header icon button opening a popover of the file's configs. Each
 * row shows a running dot (port probe) and one action — launch into a fresh
 * pane, focus the pane we already launched it in, or an inert "external"
 * chip when something outside the app holds the port. */
export function DevServersButton({
  folder,
  actions,
}: {
  folder: FolderData;
  actions: SessionActions;
}) {
  const [open, setOpen] = useState(false);
  const [rows, setRows] = useState<LaunchConfigStatus[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Fetch on open, then keep probing while open. `rows` survives a close so
  // reopening paints the last-known list instantly under the fresh fetch.
  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    const refresh = async () => {
      (await launchConfigs(folder.dir)).match({
        ok: (r) => {
          if (cancelled) return;
          setRows(r);
          setError(null);
        },
        err: (e) => {
          if (!cancelled) setError(errorMessage(e));
        },
      });
    };
    void refresh();
    const timer = setInterval(() => void refresh(), REFRESH_MS);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [open, folder.dir]);

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          variant="outline"
          size="icon-xs"
          aria-label="Dev servers"
          title="Dev servers (.claude/launch.json)"
          className="hover:text-violet-500"
          onClick={(e) => e.stopPropagation()}
        >
          <Server className="size-3.5" />
        </Button>
      </PopoverTrigger>
      <PopoverContent align="end" className="w-96 p-2">
        <div className="flex items-baseline justify-between px-1 pb-1.5">
          <span className="text-[13px] font-medium">Dev servers</span>
          <span className="font-mono text-[10.5px] text-muted-foreground/60">
            .claude/launch.json
          </span>
        </div>
        {error && <p className="px-1 pb-1 text-[12px] text-red-500">{error}</p>}
        {!error && rows && rows.length === 0 && (
          <p className="px-1 pb-1 text-[12px] text-muted-foreground">
            no launchable configurations
          </p>
        )}
        {!error &&
          rows?.map((cfg) => (
            <ConfigRow
              key={cfg.name}
              cfg={cfg}
              onLaunch={() => actions.launchDevServer(folder.dir, cfg)}
              onFocus={(sessionId) => {
                setOpen(false);
                actions.focusSession(folder.dir, sessionId);
              }}
            />
          ))}
      </PopoverContent>
    </Popover>
  );
}

function ConfigRow({
  cfg,
  onLaunch,
  onFocus,
}: {
  cfg: LaunchConfigStatus;
  onLaunch: () => void;
  onFocus: (sessionId: string) => void;
}) {
  const command = launchCommand(cfg);
  const action = launchAction(cfg);
  return (
    <div className="flex items-center gap-2 rounded-md px-1 py-1 hover:bg-accent/50">
      <span
        title={
          cfg.port == null
            ? "no port in launch.json — can't probe"
            : cfg.portListening
              ? `listening on :${cfg.port}`
              : "not running"
        }
        className={cn(
          "size-2 shrink-0 rounded-full",
          cfg.port == null
            ? "bg-muted-foreground/20"
            : cfg.portListening
              ? "bg-green-500"
              : "bg-muted-foreground/40",
        )}
      />
      <div className="min-w-0 flex-1">
        <div className="flex items-baseline gap-1.5">
          <span className="truncate text-[13px]">{cfg.name}</span>
          {cfg.port != null && (
            <span className="font-mono text-[11px] text-muted-foreground">:{cfg.port}</span>
          )}
        </div>
        <div className="truncate font-mono text-[11px] text-muted-foreground/60" title={command}>
          {command}
        </div>
      </div>
      {cfg.portListening && cfg.port != null && (
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label={`Open localhost:${cfg.port} in the browser`}
          title={`Open ${devServerUrl(cfg.port)}`}
          onClick={() => cfg.port != null && void openExternalUrl(devServerUrl(cfg.port))}
        >
          <Globe className="size-3.5" />
        </Button>
      )}
      {action === "launch" && (
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label={`Start ${cfg.name}`}
          title={`Start in a new session: ${command}`}
          className="hover:text-violet-500"
          onClick={onLaunch}
        >
          <Play className="size-3.5" />
        </Button>
      )}
      {action === "focus" && (
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label={`Focus ${cfg.name}'s terminal`}
          title="Focus the terminal it's running in"
          className="hover:text-violet-500"
          onClick={() => cfg.sessionId && onFocus(cfg.sessionId)}
        >
          <SquareTerminal className="size-3.5" />
        </Button>
      )}
      {action === "external" && (
        <span
          title="Something outside the app is listening on this port — nothing to focus, and a second launch would collide"
          className="shrink-0 rounded-md border border-border/70 px-1.5 font-mono text-[10.5px] text-muted-foreground"
        >
          external
        </span>
      )}
    </div>
  );
}
