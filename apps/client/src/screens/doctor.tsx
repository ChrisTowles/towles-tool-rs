import { useState } from "react";
import {
  CircleAlert,
  CircleCheck,
  CircleX,
  KeyRound,
  Puzzle,
  RefreshCw,
  Stethoscope,
  TerminalSquare,
  Wrench,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { DoctorReportSchema } from "@/lib/schemas/doctor";
import { invokeCmd } from "@/lib/tauri";
import { useAsyncRefresh } from "@/lib/use-async-refresh";
import { Empty, Panel } from "@/components/store-bits";

/**
 * Doctor — the same environment checks as `tt doctor` (shared `tt-doctor`
 * crate): tool versions, gh auth, required Claude plugins, and the
 * agentboard/data-hub state. The probes spawn ~10 subprocesses, so runs are
 * on-demand (mount + Refresh), never on a timer.
 */

type CheckResult = { name: string; version: string | null; ok: boolean; warning?: string };
type NameOk = { name: string; ok: boolean };
type PluginCheck = { name: string; ok: boolean; installHint?: string };
type AgentBoardCheck = {
  name: string;
  value: string;
  ok: boolean;
  warning?: string;
  hint?: string;
};
type DoctorReport = {
  result: {
    timestamp: string;
    tools: CheckResult[];
    ghAuth: boolean;
    plugins: NameOk[];
    agentboard: NameOk[];
  };
  plugins: PluginCheck[];
  agentboard: AgentBoardCheck[];
};

export function DoctorScreen() {
  const [report, setReport] = useState<DoctorReport | null>(null);
  const [running, setRunning] = useState(true);

  const refresh = useAsyncRefresh(async () => {
    setRunning(true);
    setReport(await invokeCmd<DoctorReport>("doctor_run", {}, DoctorReportSchema));
    setRunning(false);
  }, []);

  const allOk =
    report !== null &&
    report.result.tools.every((c) => c.ok) &&
    report.result.ghAuth &&
    report.plugins.every((c) => c.ok) &&
    report.agentboard.every((c) => c.ok || c.warning);

  return (
    <div className="flex flex-col gap-4">
      <div className="flex items-center justify-between gap-2">
        <h2 className="flex items-center gap-2 font-heading text-lg font-semibold">
          <Stethoscope className="size-5 text-muted-foreground" />
          Doctor
        </h2>
        <div className="flex items-center gap-3">
          {report && !running && (
            <span
              className={
                allOk
                  ? "text-xs text-green-600 dark:text-green-500"
                  : "text-xs text-amber-600 dark:text-amber-500"
              }
            >
              {allOk ? "All checks passed" : "Some checks need attention"}
            </span>
          )}
          <Button variant="outline" size="sm" onClick={() => void refresh()} disabled={running}>
            <RefreshCw className={running ? "size-3.5 animate-spin" : "size-3.5"} />
            {running ? "Checking…" : "Re-run checks"}
          </Button>
        </div>
      </div>

      {running && !report ? (
        <p className="text-sm text-muted-foreground">Probing tools…</p>
      ) : report === null ? (
        <p className="text-sm text-muted-foreground">Not available outside the app.</p>
      ) : (
        <div className="grid gap-4 md:grid-cols-2">
          <Panel
            title="Tools"
            note={`${report.result.tools.filter((c) => c.ok).length}/${report.result.tools.length}`}
            icon={<Wrench className="size-4" />}
          >
            {report.result.tools.map((c) => (
              <CheckRow
                key={c.name}
                ok={c.ok}
                warned={Boolean(c.warning)}
                name={c.name}
                value={c.version ?? "not found"}
                detail={c.warning}
              />
            ))}
          </Panel>

          <div className="flex flex-col gap-4">
            <Panel title="GitHub" icon={<KeyRound className="size-4" />}>
              <CheckRow
                ok={report.result.ghAuth}
                warned={false}
                name="gh auth"
                value={report.result.ghAuth ? "authenticated" : "not authenticated"}
                detail={report.result.ghAuth ? undefined : "Run: gh auth login"}
              />
            </Panel>

            <Panel title="Claude plugins" icon={<Puzzle className="size-4" />}>
              {report.plugins.length === 0 ? (
                <Empty>No required plugins.</Empty>
              ) : (
                report.plugins.map((p) => (
                  <CheckRow
                    key={p.name}
                    ok={p.ok}
                    warned={false}
                    name={p.name}
                    value={p.ok ? "installed" : "not installed"}
                    detail={p.installHint}
                  />
                ))
              )}
            </Panel>

            <Panel title="Agentboard" icon={<TerminalSquare className="size-4" />}>
              {report.agentboard.map((a) => (
                <CheckRow
                  key={a.name}
                  ok={a.ok}
                  warned={Boolean(a.warning)}
                  name={a.name}
                  value={a.value}
                  detail={a.hint ?? a.warning}
                />
              ))}
            </Panel>
          </div>
        </div>
      )}
    </div>
  );
}

function CheckRow({
  ok,
  warned,
  name,
  value,
  detail,
}: {
  ok: boolean;
  warned: boolean;
  name: string;
  value: string;
  detail?: string;
}) {
  return (
    <div className="flex items-start gap-3 px-3 py-2 text-sm">
      {ok && !warned ? (
        <CircleCheck className="mt-0.5 size-4 shrink-0 text-green-600 dark:text-green-500" />
      ) : ok || warned ? (
        <CircleAlert className="mt-0.5 size-4 shrink-0 text-amber-600 dark:text-amber-500" />
      ) : (
        <CircleX className="mt-0.5 size-4 shrink-0 text-destructive" />
      )}
      <div className="min-w-0 flex-1">
        <div className="flex items-baseline justify-between gap-3">
          <span className="font-medium">{name}</span>
          <span className="truncate font-mono text-xs text-muted-foreground">{value}</span>
        </div>
        {detail && <div className="mt-0.5 text-xs text-muted-foreground">{detail}</div>}
      </div>
    </div>
  );
}
