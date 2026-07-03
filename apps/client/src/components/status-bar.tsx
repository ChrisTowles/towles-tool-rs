import { CircleCheck, TriangleAlert } from "lucide-react";
import { doctorReport } from "@/lib/mock-data";
import { useWorkspace } from "@/lib/workspace";

const isTauri = "__TAURI_INTERNALS__" in window;

export function StatusBar() {
  const { openTab } = useWorkspace();

  const failing = doctorReport.tools.filter((t) => !t.ok).length;
  const total = doctorReport.tools.length;

  return (
    <footer className="flex h-7 shrink-0 items-center justify-between border-t px-3 text-xs text-muted-foreground">
      <button
        className="flex items-center gap-1.5 hover:text-foreground"
        onClick={() => openTab("doctor")}
      >
        {failing === 0 ? (
          <>
            <CircleCheck className="size-3.5 text-green-600 dark:text-green-500" />
            doctor: all {total} checks passing
          </>
        ) : (
          <>
            <TriangleAlert className="size-3.5 text-amber-600 dark:text-amber-500" />
            doctor: {total - failing}/{total} checks passing
          </>
        )}
      </button>
      <div className="flex items-center gap-3">
        <span>{isTauri ? "Tauri shell" : "browser"}</span>
        <span>ttr v0.1.0</span>
      </div>
    </footer>
  );
}
