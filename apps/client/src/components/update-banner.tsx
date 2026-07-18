import { useState } from "react";
import { Sparkles, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { openExternalUrl } from "@/lib/open-url";
import { useUpdateCheck } from "@/lib/update";

/**
 * Full-width strip under the day bar announcing a newer GitHub release (see
 * `useUpdateCheck`). Dismiss is session-only — no persisted "skip this
 * version" state, so it reappears on relaunch until the app is actually
 * updated. Mirrors `DmBanner`'s layout/styling.
 */
export function UpdateBanner() {
  const update = useUpdateCheck();
  const [dismissed, setDismissed] = useState(false);

  if (!update || dismissed) return null;

  return (
    <div className="flex shrink-0 items-center gap-2.5 border-b border-l-2 border-l-sky-500 bg-sky-500/5 px-3 py-1.5 text-xs">
      <Sparkles className="size-4 shrink-0 text-sky-500" />
      <span className="text-foreground">
        Towles Tool <span className="font-medium">{update.latestVersion}</span> is available —
        you're on {update.currentVersion}
      </span>

      <div className="flex-1" />

      <Button
        variant="outline"
        size="sm"
        className="h-6 border-sky-500/40 px-2 text-xs text-sky-600 hover:text-sky-600 dark:text-sky-400 dark:hover:text-sky-400"
        onClick={() => void openExternalUrl(update.releaseUrl)}
      >
        View release
      </Button>
      <Button
        variant="ghost"
        size="sm"
        className="h-6 px-2 text-xs text-muted-foreground"
        onClick={() => setDismissed(true)}
      >
        <X className="size-3.5" />
      </Button>
    </div>
  );
}
