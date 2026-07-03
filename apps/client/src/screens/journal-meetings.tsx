import { Users } from "lucide-react";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { meetings } from "@/lib/mock-data";

export function JournalMeetingsScreen() {
  return (
    <div className="flex flex-col gap-4">
      <div>
        <h2 className="font-heading text-lg font-semibold">Meetings</h2>
        <p className="text-sm text-muted-foreground">Meeting notes, newest first.</p>
      </div>

      <div className="divide-y rounded-lg border">
        {meetings.map((meeting) => (
          <button
            key={meeting.file}
            className="flex w-full items-center gap-3 px-3 py-2.5 text-left text-sm hover:bg-muted/50"
            onClick={() => toast.info("Opening meeting notes isn't wired to the CLI yet")}
          >
            <Users className="size-4 shrink-0 text-muted-foreground" />
            <div className="flex-1 truncate">
              <div className="truncate">{meeting.title}</div>
              <div className="truncate text-xs text-muted-foreground">
                {meeting.attendees.join(", ")}
              </div>
            </div>
            {meeting.tags.map((tag) => (
              <Badge key={tag} variant="secondary">
                {tag}
              </Badge>
            ))}
            <span className="shrink-0 font-mono text-xs text-muted-foreground">{meeting.date}</span>
          </button>
        ))}
      </div>
    </div>
  );
}
