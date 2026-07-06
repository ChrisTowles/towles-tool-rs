import { useState } from "react";
import { HardDriveDownload, Loader2, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useShpoolStatus } from "@/lib/shpool";

/**
 * Onboarding notice for session persistence. When the shpool daemon isn't
 * installed, agentboard shells die with the app; this slim bar offers a
 * one-click `cargo install shpool` (streamed in the background) to fix that.
 * Hidden once shpool is present, outside Tauri, or dismissed for the session.
 */
export function PersistenceBanner() {
  const { status, installing, installProgress, install } = useShpoolStatus();
  const [dismissed, setDismissed] = useState(false);

  // Nothing to show: not in the app, still probing, already set up, or the
  // user waved it away this session.
  if (!status || status.installed || dismissed) return null;

  return (
    <div className="flex items-center gap-3 border-b border-sky-500/20 bg-sky-500/5 px-3 py-1.5 text-sm dark:bg-sky-400/5">
      <HardDriveDownload className="size-4 shrink-0 text-sky-600 dark:text-sky-400" />
      <div className="min-w-0 flex-1">
        <span className="text-foreground">Shells won't survive an app restart.</span>{" "}
        {installing ? (
          <span className="text-muted-foreground">
            {installProgress ? (
              <span className="font-mono text-xs">{installProgress}</span>
            ) : (
              "Installing shpool…"
            )}
          </span>
        ) : status.cargoAvailable ? (
          <span className="text-muted-foreground">
            Install shpool to keep them running in the background.
          </span>
        ) : (
          <span className="text-muted-foreground">
            Install Rust (rustup.rs) to enable one-click persistence setup.
          </span>
        )}
      </div>

      {status.cargoAvailable &&
        (installing ? (
          <span className="flex shrink-0 items-center gap-1.5 text-xs text-muted-foreground">
            <Loader2 className="size-3.5 animate-spin" /> compiling…
          </span>
        ) : (
          <Button
            size="sm"
            variant="outline"
            className="h-7 shrink-0 border-sky-500/40 text-sky-700 hover:bg-sky-500/10 dark:text-sky-300"
            onClick={install}
          >
            Set up persistence
          </Button>
        ))}

      {!installing && (
        <button
          type="button"
          title="dismiss for now"
          onClick={() => setDismissed(true)}
          className="shrink-0 rounded-sm p-0.5 text-muted-foreground/60 hover:text-foreground"
        >
          <X className="size-3.5" />
        </button>
      )}
    </div>
  );
}
