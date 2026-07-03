import { useState } from "react";
import { FileText } from "lucide-react";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { notes } from "@/lib/mock-data";

export function JournalNotesScreen() {
  const [query, setQuery] = useState("");

  const visible = notes.filter((n) =>
    (n.title + n.file + n.tags.join(" ")).toLowerCase().includes(query.toLowerCase()),
  );

  return (
    <div className="flex flex-col gap-4">
      <div>
        <h2 className="font-heading text-lg font-semibold">Notes</h2>
        <p className="text-sm text-muted-foreground">Everything under notes/ in the journal.</p>
      </div>

      <Input
        placeholder="Filter notes…"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        className="max-w-sm"
      />

      <div className="divide-y rounded-lg border">
        {visible.map((note) => (
          <button
            key={note.file}
            className="flex w-full items-center gap-3 px-3 py-2.5 text-left text-sm hover:bg-muted/50"
            onClick={() => toast.info("Opening notes isn't wired to the CLI yet")}
          >
            <FileText className="size-4 shrink-0 text-muted-foreground" />
            <span className="flex-1 truncate">{note.title}</span>
            {note.tags.map((tag) => (
              <Badge key={tag} variant="secondary">
                {tag}
              </Badge>
            ))}
            <span className="shrink-0 font-mono text-xs text-muted-foreground">{note.date}</span>
          </button>
        ))}
        {visible.length === 0 && (
          <p className="px-3 py-8 text-center text-sm text-muted-foreground">
            No notes match “{query}”.
          </p>
        )}
      </div>
    </div>
  );
}
