import { useEffect, useState } from "react";
import { isTauri } from "@/lib/tauri";

/**
 * The single source for the app's displayed version. In the shipped Tauri
 * shell this comes from `app.getVersion()` (which reads
 * `crates-tauri/tt-app/tauri.conf.json`); in plain-Vite browser dev there is
 * no host, so we fall back to {@link DEV_VERSION}.
 */
const DEV_VERSION = "0.1.0";

/** The `ttr vX.Y.Z` label rendered in the status bar and the About tab. */
export function useAppVersion(): string {
  const [version, setVersion] = useState(DEV_VERSION);
  useEffect(() => {
    if (!isTauri()) return;
    let cancelled = false;
    void import("@tauri-apps/api/app").then(async ({ getVersion }) => {
      const v = await getVersion();
      if (!cancelled) setVersion(v);
    });
    return () => {
      cancelled = true;
    };
  }, []);
  return `ttr v${version}`;
}
