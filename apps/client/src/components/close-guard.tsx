import { useEffect, useState } from "react";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";

/**
 * Ask-on-exit. When shells are running and the session daemon can keep them
 * alive, the Rust side intercepts the window close and emits
 * `app://close-requested` with the live count instead of closing; this dialog
 * resolves it via the `app_close` command — keep (quit, shells stay detached
 * and resume next launch) or kill everything. Cancel just stays open.
 */
export function CloseGuard() {
  const [liveCount, setLiveCount] = useState<number | null>(null);

  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void (async () => {
      const { listen } = await import("@tauri-apps/api/event");
      const un = await listen<{ live: number }>("app://close-requested", (e) =>
        setLiveCount(e.payload.live),
      );
      if (disposed) un();
      else unlisten = un;
    })();
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const resolve = (killSessions: boolean) => {
    setLiveCount(null);
    void import("@tauri-apps/api/core").then(({ invoke }) =>
      invoke("app_close", { killSessions }).catch(() => {}),
    );
  };

  return (
    <AlertDialog open={liveCount !== null} onOpenChange={(open) => !open && setLiveCount(null)}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>
            {liveCount === 1 ? "1 shell is" : `${liveCount ?? 0} shells are`} still running
          </AlertDialogTitle>
          <AlertDialogDescription>
            Keep them running in the background and they'll be here, detached, when you come
            back — agents keep working. Or kill everything on the way out.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Stay</AlertDialogCancel>
          <Button variant="destructive" onClick={() => resolve(true)}>
            Kill all &amp; quit
          </Button>
          <AlertDialogAction onClick={() => resolve(false)}>
            Keep running &amp; quit
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
