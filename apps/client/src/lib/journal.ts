import { invokeOrThrow, invokeToast } from "@/lib/tauri";

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

export const journalGetToday = () => invokeToast<TodayNote>("journal_get_today");

export const journalList = (opts: { ty?: string; limit?: number; sort?: string } = {}) =>
  invokeToast<JournalEntry[]>("journal_list", opts);

export const journalCreate = (ty: "note" | "meeting", title: string) =>
  invokeToast<string>("journal_create", { ty, title });

export const journalSearch = (opts: { query: string; ty?: string }) =>
  invokeToast<SearchMatch[]>("journal_search", opts);

export const journalOpen = (relativePath: string) =>
  invokeToast<null>("journal_open", { relativePath });

/** The marker string a `journal_save` uses to detect an out-of-band change on disk. */
export const JOURNAL_STALE_ERROR = "file changed on disk since it was loaded";

/**
 * Overwrite a journal entry's full content. `expectedOriginal` is the content last
 * loaded; the command rejects (with {@link JOURNAL_STALE_ERROR}) if the file changed on
 * disk since then, so callers can show a distinct "reload" state instead of clobbering.
 * Errors propagate (via {@link invokeOrThrow}) so callers can tell stale from success.
 */
export const journalSave = (relativePath: string, expectedOriginal: string, content: string) =>
  invokeOrThrow<null>("journal_save", { relativePath, expectedOriginal, content });
