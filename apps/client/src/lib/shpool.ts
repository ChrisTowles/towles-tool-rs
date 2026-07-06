import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";
import { isTauri } from "@/lib/data";
import { invokeCmd, invokeOrThrow } from "@/lib/tauri";

/** Persistence capability, from the Rust `shpool_status` command. */
export type ShpoolStatus = {
  /** The shpool binary is present — shells survive an app restart. */
  installed: boolean;
  /** cargo is available to build shpool (the installer needs it). */
  cargoAvailable: boolean;
  /** An install is currently running. */
  installing: boolean;
};

/**
 * Track whether session persistence (shpool) is set up. Polls `shpool_status`
 * once on mount and re-checks whenever an install finishes. Returns `null`
 * outside Tauri (plain-Vite browser dev) so the banner never shows there.
 *
 * `installProgress` is the latest line of `cargo install` output while a
 * build is running (for a live "still working" hint), else null.
 */
export function useShpoolStatus(): {
  status: ShpoolStatus | null;
  installing: boolean;
  installProgress: string | null;
  install: () => void;
} {
  const [status, setStatus] = useState<ShpoolStatus | null>(null);
  const [installing, setInstalling] = useState(false);
  const [installProgress, setInstallProgress] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    const s = await invokeCmd<ShpoolStatus>("shpool_status");
    setStatus(s);
    if (s) setInstalling(s.installing);
  }, []);

  useEffect(() => {
    if (!isTauri()) return;
    void refresh();

    let disposed = false;
    const unlisteners: (() => void)[] = [];
    void (async () => {
      const { listen } = await import("@tauri-apps/api/event");
      const onLog = await listen<{ line: string }>("shpool://install-log", (e) =>
        setInstallProgress(e.payload.line),
      );
      const onDone = await listen<{ ok: boolean; error?: string }>(
        "shpool://install-done",
        (e) => {
          setInstalling(false);
          setInstallProgress(null);
          if (e.payload.ok) {
            toast.success("Session persistence enabled — shells now survive an app restart.");
          } else {
            toast.error(`shpool install failed: ${e.payload.error ?? "unknown error"}`);
          }
          void refresh();
        },
      );
      if (disposed) {
        onLog();
        onDone();
        return;
      }
      unlisteners.push(onLog, onDone);
    })();

    return () => {
      disposed = true;
      for (const un of unlisteners) un();
    };
  }, [refresh]);

  const install = useCallback(() => {
    setInstalling(true);
    setInstallProgress(null);
    void invokeOrThrow("shpool_install").catch((e: unknown) => {
      setInstalling(false);
      toast.error(`Couldn't start install: ${e instanceof Error ? e.message : String(e)}`);
    });
  }, []);

  return { status, installing, installProgress, install };
}
