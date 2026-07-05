import { FlaskConical } from "lucide-react";
import { isTauri } from "@/lib/data";

/**
 * A loud, always-visible strip shown when the app is running in the browser
 * without the Tauri backend — i.e. every screen is rendering fake `MOCK_*`
 * data, not the real store/agentboard. Renders nothing in the real app
 * (`npm run dev` / a packaged build), so seeing it means "not connected".
 */
export function MockBanner() {
  if (isTauri()) return null;
  return (
    <div className="flex h-6 shrink-0 items-center justify-center gap-2 bg-amber-500 text-center text-[11px] font-semibold tracking-wide text-black">
      <FlaskConical className="size-3.5" />
      MOCK DATA — not connected to the Tauri backend. Run{" "}
      <span className="font-mono">npm run dev</span> for the real app.
    </div>
  );
}
