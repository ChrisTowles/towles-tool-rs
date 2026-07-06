import { useEffect, useState } from "react";
import { ExternalLink, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { journalLog } from "@/lib/data";
import { journalGetToday, journalOpen, type TodayNote } from "@/lib/journal";

/** Today — today's daily note, read from and appended to via the real journal files. */
export function JournalTodayScreen() {
  const [note, setNote] = useState<TodayNote | null>(null);
  const [loading, setLoading] = useState(true);
  const [draft, setDraft] = useState("");

  async function refresh() {
    setLoading(true);
    setNote(await journalGetToday());
    setLoading(false);
  }

  useEffect(() => {
    void refresh();
  }, []);

  async function submitLog() {
    const line = draft.trim();
    if (!line) return;
    setDraft("");
    if (await journalLog(line)) void refresh();
  }

  return (
    <div className="flex flex-col gap-4">
      <div className="flex items-center justify-between gap-2">
        <div>
          <h2 className="font-heading text-lg font-semibold">Today</h2>
          {note && <p className="font-mono text-xs text-muted-foreground">{note.relativePath}</p>}
        </div>
        <div className="flex gap-2">
          <Button variant="outline" size="sm" onClick={() => void refresh()}>
            <RefreshCw className="size-3.5" />
            Refresh
          </Button>
          <Button
            variant="outline"
            size="sm"
            disabled={!note}
            onClick={() => note && void journalOpen(note.relativePath)}
          >
            <ExternalLink className="size-3.5" />
            Open in editor
          </Button>
        </div>
      </div>

      <Input
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") void submitLog();
        }}
        placeholder="Log to today's note… (⌘J works anywhere)"
      />

      <div className="rounded-lg border border-border bg-card p-3.5">
        {loading && !note ? (
          <p className="text-sm text-muted-foreground">Loading…</p>
        ) : note ? (
          <pre className="overflow-x-auto whitespace-pre-wrap font-mono text-xs leading-relaxed text-foreground">
            {note.content}
          </pre>
        ) : (
          <p className="text-sm text-muted-foreground">Not available outside the app.</p>
        )}
      </div>
    </div>
  );
}
