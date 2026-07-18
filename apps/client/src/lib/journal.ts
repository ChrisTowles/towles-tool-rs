import { invoke } from "@/lib/tauri";

/**
 * Client-side bridge to the journal screens (the Rust `tt-journal` crate, surfaced
 * via `crates-tauri/tt-app/src/journal.rs`). Unlike the store snapshot, these are
 * plain request/response commands — no live event stream.
 */

export type TodayNote = {
  relativePath: string;
  content: string;
};

export type JournalEntry = {
  relativePath: string;
  ty: string | null;
  date: string | null;
  sizeLabel: string;
};

export type SearchMatch = {
  relativePath: string;
  lineNumber: number;
  context: string[];
};

/** Today's daily note, creating it from the template if it doesn't exist. */
export const journalGetToday = () => invoke<TodayNote>("journal_get_today");

/** Journal entries, newest first, optionally filtered by type. */
export const journalList = (opts: { ty?: string; limit?: number; sort?: string } = {}) =>
  invoke<JournalEntry[]>("journal_list", opts);

/** Create a note or meeting entry from its template. Resolves its relative path. */
export const journalCreate = (ty: "note" | "meeting", title: string) =>
  invoke<string>("journal_create", { ty, title });

/** Full-text search across journal entries. */
export const journalSearch = (opts: { query: string; ty?: string }) =>
  invoke<SearchMatch[]>("journal_search", opts);

/** Open an entry in the user's preferred editor. */
export const journalOpen = (relativePath: string) => invoke<null>("journal_open", { relativePath });

/** The marker string a `journal_save` uses to detect an out-of-band change on disk. */
export const JOURNAL_STALE_ERROR = "file changed on disk since it was loaded";

/**
 * Overwrite a journal entry's full content. `expectedOriginal` is the content last
 * loaded; the command rejects (with {@link JOURNAL_STALE_ERROR}) if the file changed on
 * disk since then, so callers can show a distinct "reload" state instead of clobbering.
 * The rejection stays in the `Result`, so callers can tell stale from success.
 */
export const journalSave = (relativePath: string, expectedOriginal: string, content: string) =>
  invoke<null>("journal_save", { relativePath, expectedOriginal, content });
