import { useState } from "react";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { tokenUsage } from "@/lib/mock-data";

const formatTokens = (n: number) =>
  n >= 1_000_000 ? `${(n / 1_000_000).toFixed(1)}M` : `${Math.round(n / 1000)}k`;

export function GraphScreen() {
  const [days, setDays] = useState("14");

  const totalInput = tokenUsage.reduce((sum, s) => sum + s.inputTokens, 0);
  const totalOutput = tokenUsage.reduce((sum, s) => sum + s.outputTokens, 0);
  const totalSessions = tokenUsage.reduce((sum, s) => sum + s.sessions, 0);
  const max = Math.max(...tokenUsage.map((s) => s.inputTokens + s.outputTokens));

  return (
    <div className="flex flex-col gap-4">
      <div className="flex items-center justify-between">
        <div>
          <h2 className="font-heading text-lg font-semibold">Graph</h2>
          <p className="text-sm text-muted-foreground">
            Claude Code token usage by project, last {days} days.
          </p>
        </div>
        <Select value={days} onValueChange={setDays}>
          <SelectTrigger className="w-28">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="7">7 days</SelectItem>
            <SelectItem value="14">14 days</SelectItem>
            <SelectItem value="30">30 days</SelectItem>
          </SelectContent>
        </Select>
      </div>

      <div className="grid grid-cols-3 gap-4">
        <Card>
          <CardHeader>
            <CardDescription>Sessions</CardDescription>
            <CardTitle className="text-2xl tabular-nums">{totalSessions}</CardTitle>
          </CardHeader>
        </Card>
        <Card>
          <CardHeader>
            <CardDescription>Input tokens</CardDescription>
            <CardTitle className="text-2xl tabular-nums">{formatTokens(totalInput)}</CardTitle>
          </CardHeader>
        </Card>
        <Card>
          <CardHeader>
            <CardDescription>Output tokens</CardDescription>
            <CardTitle className="text-2xl tabular-nums">{formatTokens(totalOutput)}</CardTitle>
          </CardHeader>
        </Card>
      </div>

      <Card>
        <CardHeader>
          <CardTitle>By project</CardTitle>
          <CardDescription>Input + output tokens per project.</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          {tokenUsage.map((s) => {
            const total = s.inputTokens + s.outputTokens;
            return (
              <div key={s.project} className="flex items-center gap-3 text-sm">
                <span className="w-48 truncate font-mono text-xs">{s.project}</span>
                <div className="h-4 flex-1 overflow-hidden rounded-sm bg-muted">
                  <div
                    className="h-full rounded-sm bg-primary"
                    style={{ width: `${(total / max) * 100}%` }}
                  />
                </div>
                <span className="w-14 text-right font-mono text-xs tabular-nums text-muted-foreground">
                  {formatTokens(total)}
                </span>
              </div>
            );
          })}
        </CardContent>
      </Card>
    </div>
  );
}
