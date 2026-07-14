import { useEffect, useState } from "react";
import { isTauri } from "@/lib/tauri";

/** Mirrors `tt_update::UpdateCheck` (crates/tt-update/src/lib.rs). */
export type UpdateCheck = {
  currentVersion: string;
  latestVersion: string;
  releaseUrl: string;
  updateAvailable: boolean;
};

/**
 * Subscribes to `update://available`, the event the app's startup check fires
 * only when a newer GitHub release exists (see
 * `crates-tauri/tt-app/src/update.rs`). Never fires for "checked, nothing
 * newer" or in plain-Vite browser dev, so the initial state is simply "no
 * update known yet" — there's no separate loading/error state to model.
 */
export function useUpdateCheck(): UpdateCheck | null {
  const [update, setUpdate] = useState<UpdateCheck | null>(null);

  useEffect(() => {
    if (!isTauri()) return;
    let unlisten: (() => void) | undefined;
    void (async () => {
      const { listen } = await import("@tauri-apps/api/event");
      unlisten = await listen<UpdateCheck>("update://available", (event) => {
        setUpdate(event.payload);
      });
    })();
    return () => unlisten?.();
  }, []);

  return update;
}
