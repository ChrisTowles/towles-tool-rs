import { toast } from "sonner";
import { isTauri } from "@/lib/data";

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

async function journalInvoke<T>(
  command: string,
  args: Record<string, unknown> = {},
): Promise<T | null> {
  if (!isTauri()) return null;
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    return await invoke<T>(command, args);
  } catch (e) {
    toast.error(String(e));
    return null;
  }
}

export const journalGetToday = () => journalInvoke<TodayNote>("journal_get_today");

export const journalList = (opts: { ty?: string; limit?: number; sort?: string } = {}) =>
  journalInvoke<JournalEntry[]>("journal_list", opts);

export const journalCreate = (ty: "note" | "meeting", title: string) =>
  journalInvoke<string>("journal_create", { ty, title });

export const journalSearch = (opts: { query: string; ty?: string }) =>
  journalInvoke<SearchMatch[]>("journal_search", opts);

export const journalOpen = (relativePath: string) =>
  journalInvoke<null>("journal_open", { relativePath });
