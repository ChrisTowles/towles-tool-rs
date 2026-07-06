import { JournalEntryList } from "@/components/journal-entry-list";

export function JournalNotesScreen() {
  return (
    <JournalEntryList
      ty="note"
      title="Notes"
      placeholder="New note title…"
      emptyLabel="No notes yet."
    />
  );
}
