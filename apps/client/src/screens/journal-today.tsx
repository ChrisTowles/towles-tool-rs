import { Plus } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { dailyNote } from "@/lib/mock-data";

const today = new Date().toLocaleDateString(undefined, {
  weekday: "long",
  year: "numeric",
  month: "long",
  day: "numeric",
});

export function JournalTodayScreen() {
  return (
    <div className="flex flex-col gap-4">
      <div className="flex items-center justify-between">
        <div>
          <h2 className="font-heading text-lg font-semibold">Today</h2>
          <p className="text-sm text-muted-foreground">{today}</p>
        </div>
        <Button onClick={() => toast.info("Journal entries aren't wired to the CLI yet")}>
          <Plus /> New entry
        </Button>
      </div>

      <Card>
        <CardHeader>
          <CardTitle className="font-mono text-sm font-normal text-muted-foreground">
            {dailyNote.file}
          </CardTitle>
          <CardDescription>Daily note</CardDescription>
        </CardHeader>
        <CardContent>
          <ul className="flex flex-col gap-3">
            {dailyNote.entries.map((entry) => (
              <li key={entry.time} className="flex gap-3 text-sm">
                <span className="shrink-0 font-mono text-muted-foreground">{entry.time}</span>
                <span>{entry.text}</span>
              </li>
            ))}
          </ul>
        </CardContent>
      </Card>
    </div>
  );
}
