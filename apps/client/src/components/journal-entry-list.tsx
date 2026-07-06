import { useEffect, useState } from "react";
import { ExternalLink, Plus, Search } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  journalCreate,
  journalList,
  journalOpen,
  journalSearch,
  type JournalEntry,
  type SearchMatch,
} from "@/lib/journal";

const SEARCH_DEBOUNCE_MS = 250;

type SearchGroup = { relativePath: string; matches: SearchMatch[] };

function groupByFile(matches: SearchMatch[]): SearchGroup[] {
  const groups: SearchGroup[] = [];
  const byPath = new Map<string, SearchGroup>();
  for (const m of matches) {
    let group = byPath.get(m.relativePath);
    if (!group) {
      group = { relativePath: m.relativePath, matches: [] };
      byPath.set(m.relativePath, group);
      groups.push(group);
    }
    group.matches.push(m);
  }
  return groups;
}

/**
 * Shared list + search + create UI for the Notes and Meetings screens: a
 * `journal_list`-filtered list of entries, a live `journal_search` (debounced) that
 * replaces the list with matching lines grouped by file once a query is typed, and a
 * title box that creates an entry via `journal_create` and opens it in the editor.
 */
export function JournalEntryList({
  ty,
  title,
  placeholder,
  emptyLabel,
}: {
  ty: "note" | "meeting";
  title: string;
  placeholder: string;
  emptyLabel: string;
}) {
  const [entries, setEntries] = useState<JournalEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [draft, setDraft] = useState("");
  const [query, setQuery] = useState("");
  const [searchGroups, setSearchGroups] = useState<SearchGroup[] | null>(null);
  const [searching, setSearching] = useState(false);

  async function refresh() {
    setLoading(true);
    setEntries((await journalList({ ty })) ?? []);
    setLoading(false);
  }

  useEffect(() => {
    void refresh();
  }, [ty]);

  useEffect(() => {
    const trimmed = query.trim();
    if (!trimmed) {
      setSearchGroups(null);
      setSearching(false);
      return;
    }
    setSearching(true);
    let cancelled = false;
    const handle = setTimeout(() => {
      void journalSearch({ query: trimmed, ty }).then((matches) => {
        if (cancelled) return;
        setSearchGroups(groupByFile(matches ?? []));
        setSearching(false);
      });
    }, SEARCH_DEBOUNCE_MS);
    return () => {
      cancelled = true;
      clearTimeout(handle);
    };
  }, [query, ty]);

  async function create() {
    const entryTitle = draft.trim();
    if (!entryTitle) return;
    setDraft("");
    const path = await journalCreate(ty, entryTitle);
    if (path) {
      void refresh();
      void journalOpen(path);
    }
  }

  return (
    <div className="flex flex-col gap-4">
      <h2 className="font-heading text-lg font-semibold">{title}</h2>

      <div className="flex gap-2">
        <Input
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void create();
          }}
          placeholder={placeholder}
        />
        <Button size="sm" onClick={() => void create()}>
          <Plus className="size-3.5" />
          New
        </Button>
      </div>

      <div className="relative">
        <Search className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground" />
        <Input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={`Search ${title.toLowerCase()}…`}
          className="pl-8"
        />
      </div>

      <div className="rounded-lg border border-border">
        {searchGroups !== null ? (
          searching ? (
            <p className="p-3.5 text-sm text-muted-foreground">Searching…</p>
          ) : searchGroups.length === 0 ? (
            <p className="p-3.5 text-sm text-muted-foreground">No matches for "{query.trim()}".</p>
          ) : (
            searchGroups.map((g) => (
              <div
                key={g.relativePath}
                className="border-b border-border px-3.5 py-2 last:border-b-0"
              >
                <div className="flex items-center justify-between gap-2">
                  <p className="truncate text-sm text-foreground">{g.relativePath}</p>
                  <Button variant="ghost" size="sm" onClick={() => void journalOpen(g.relativePath)}>
                    <ExternalLink className="size-3.5" />
                  </Button>
                </div>
                {g.matches.map((m, i) => (
                  <pre
                    key={i}
                    className="mt-1 overflow-x-auto rounded-md bg-muted p-2 font-mono text-[11px] leading-relaxed text-muted-foreground"
                  >
                    {m.context.join("\n")}
                  </pre>
                ))}
              </div>
            ))
          )
        ) : loading ? (
          <p className="p-3.5 text-sm text-muted-foreground">Loading…</p>
        ) : entries.length === 0 ? (
          <p className="p-3.5 text-sm text-muted-foreground">{emptyLabel}</p>
        ) : (
          entries.map((e) => (
            <div
              key={e.relativePath}
              className="flex items-center justify-between gap-2 border-b border-border px-3.5 py-2 last:border-b-0 hover:bg-accent/50"
            >
              <div className="min-w-0">
                <p className="truncate text-sm text-foreground">{e.relativePath}</p>
                <p className="font-mono text-[11px] text-muted-foreground">
                  {e.date ?? "-"} · {e.sizeLabel}
                </p>
              </div>
              <Button variant="ghost" size="sm" onClick={() => void journalOpen(e.relativePath)}>
                <ExternalLink className="size-3.5" />
              </Button>
            </div>
          ))
        )}
      </div>
    </div>
  );
}
