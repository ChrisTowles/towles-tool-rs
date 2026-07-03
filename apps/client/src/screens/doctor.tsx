import { CircleCheck, CircleX, RefreshCw } from "lucide-react";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { doctorReport, type DoctorTool } from "@/lib/mock-data";

function ToolRow({ tool }: { tool: DoctorTool }) {
  return (
    <div className="flex items-center gap-3 px-3 py-2 text-sm">
      {tool.ok ? (
        <CircleCheck className="size-4 shrink-0 text-green-600 dark:text-green-500" />
      ) : (
        <CircleX className="size-4 shrink-0 text-destructive" />
      )}
      <span className="w-32 font-mono">{tool.name}</span>
      <span className="flex-1 font-mono text-muted-foreground">
        {tool.version ?? tool.note ?? "—"}
      </span>
      <Badge variant={tool.ok ? "secondary" : "destructive"}>{tool.ok ? "ok" : "fail"}</Badge>
    </div>
  );
}

export function DoctorScreen() {
  const failing = doctorReport.tools.filter((t) => !t.ok);

  const rerun = () =>
    toast.promise(new Promise((resolve) => setTimeout(resolve, 900)), {
      loading: "Running checks…",
      success: "Checks finished (mock data — not wired to ttr doctor yet)",
      error: "Checks failed",
    });

  return (
    <div className="flex flex-col gap-4">
      <div className="flex items-center justify-between">
        <div>
          <h2 className="font-heading text-lg font-semibold">Doctor</h2>
          <p className="text-sm text-muted-foreground">
            {failing.length === 0
              ? "All checks passing."
              : `${failing.length} of ${doctorReport.tools.length} checks failing.`}
          </p>
        </div>
        <Button variant="outline" onClick={rerun}>
          <RefreshCw /> Run checks
        </Button>
      </div>

      <section className="flex flex-col gap-2">
        <h3 className="text-xs font-medium text-muted-foreground">Tools</h3>
        <div className="divide-y rounded-lg border">
          {doctorReport.tools.map((tool) => (
            <ToolRow key={tool.name} tool={tool} />
          ))}
        </div>
      </section>

      <section className="flex flex-col gap-2">
        <h3 className="text-xs font-medium text-muted-foreground">GitHub</h3>
        <div className="rounded-lg border">
          <div className="flex items-center gap-3 px-3 py-2 text-sm">
            {doctorReport.ghAuth ? (
              <CircleCheck className="size-4 shrink-0 text-green-600 dark:text-green-500" />
            ) : (
              <CircleX className="size-4 shrink-0 text-destructive" />
            )}
            <span className="flex-1">gh auth status</span>
            <Badge variant="secondary">{doctorReport.ghAuth ? "authenticated" : "signed out"}</Badge>
          </div>
        </div>
      </section>

      <section className="flex flex-col gap-2">
        <h3 className="text-xs font-medium text-muted-foreground">Claude Code plugins</h3>
        <div className="divide-y rounded-lg border">
          {doctorReport.plugins.map((plugin) => (
            <div key={plugin.name} className="flex items-center gap-3 px-3 py-2 text-sm">
              {plugin.ok ? (
                <CircleCheck className="size-4 shrink-0 text-green-600 dark:text-green-500" />
              ) : (
                <CircleX className="size-4 shrink-0 text-destructive" />
              )}
              <span className="flex-1 font-mono">{plugin.name}</span>
              <Badge variant="secondary">{plugin.ok ? "ok" : "fail"}</Badge>
            </div>
          ))}
        </div>
      </section>
    </div>
  );
}
