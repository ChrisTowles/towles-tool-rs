import { JournalEntryList } from "@/components/journal-entry-list";

export function JournalMeetingsScreen() {
  return (
    <JournalEntryList
      ty="meeting"
      title="Meetings"
      placeholder="New meeting title…"
      emptyLabel="No meetings yet."
    />
  );
}
