import { Stethoscope } from "lucide-react";
import { useWorkspace } from "@/lib/workspace";

const isTauri = "__TAURI_INTERNALS__" in window;

export function StatusBar() {
  const { openTab } = useWorkspace();

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
        <span
          className={
            isTauri
              ? undefined
              : "font-medium text-amber-600 dark:text-amber-500"
          }
        >
          {isTauri ? "Tauri shell" : "browser"}
        </span>
        <span>ttr v0.1.0</span>
      </div>
    </footer>
  );
}
