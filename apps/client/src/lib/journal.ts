import { invokeToast } from "@/lib/tauri";

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
