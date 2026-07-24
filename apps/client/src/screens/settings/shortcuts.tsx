import { Kbd, KbdGroup } from "@/components/ui/kbd";
import { isEmptyQuery, matchesFilter } from "@/lib/settings-filter";
import { SHORTCUTS, shortcutKeys, type ShortcutScope } from "@/lib/shortcuts";
import { NoMatches } from "./common";

export const SCOPE_LABELS: Record<ShortcutScope, string> = {
  global: "",
  agentboard: "Agentboard",
  board: "Board",
};

/** Shortcuts list, filtered by the same predicate (description + when + scope). */
export function ShortcutsList({ query }: { query: string }) {
  const empty = isEmptyQuery(query);
  const rows = Object.values(SHORTCUTS).filter((s) =>
    empty
      ? true
      : matchesFilter(query, s.description, [
          s.when ?? "",
          SCOPE_LABELS[s.scope],
          ...shortcutKeys(s.id),
        ]),
  );
  if (rows.length === 0) return <NoMatches query={query} />;
  return (
    <div className="flex flex-col">
      {rows.map((s, i) => (
        <div
          key={s.id}
          className={`flex items-center justify-between py-2 ${
            i > 0 ? "border-t border-border" : ""
          }`}
        >
          <span className="text-sm text-muted-foreground">
            {s.description}
            {s.when && <span className="text-muted-foreground/70"> — {s.when}</span>}
            {s.scope !== "global" && (
              <span className="ml-2 text-xs text-muted-foreground/70">
                ({SCOPE_LABELS[s.scope]})
              </span>
            )}
          </span>
          <KbdGroup>
            {shortcutKeys(s.id).map((cap) => (
              <Kbd key={cap}>{cap}</Kbd>
            ))}
          </KbdGroup>
        </div>
      ))}
    </div>
  );
}
