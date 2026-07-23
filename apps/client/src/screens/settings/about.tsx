import { Fragment } from "react";
import { isEmptyQuery, matchesFilter } from "@/lib/settings-filter";
import { NoMatches } from "./common";

/** Real, known location of the settings file (shared with the TypeScript CLI). */
export const SETTINGS_PATH = "~/.config/towles-tool/towles-tool.settings.json";

/** About tab: static facts, filtered so the whole screen stays consistent. */
export function AboutInfo({ query, version }: { query: string; version: string }) {
  const empty = isEmptyQuery(query);
  const rows: { label: string; keywords: string[]; node: React.ReactNode }[] = [
    {
      label: "Version",
      keywords: ["about", "app"],
      node: (
        <div className="flex justify-between">
          <span className="text-muted-foreground">Version</span>
          <span className="font-mono">{version}</span>
        </div>
      ),
    },
    {
      label: "Identifier",
      keywords: ["about", "bundle", "id"],
      node: (
        <div className="flex justify-between">
          <span className="text-muted-foreground">Identifier</span>
          <span className="font-mono">dev.towles.tool</span>
        </div>
      ),
    },
    {
      label: "Settings file",
      keywords: ["about", "path", "config", "json"],
      node: (
        <div className="flex flex-col gap-1">
          <span className="text-muted-foreground">Settings file</span>
          <span className="font-mono text-xs break-all">{SETTINGS_PATH}</span>
        </div>
      ),
    },
  ];
  const visible = empty ? rows : rows.filter((r) => matchesFilter(query, r.label, r.keywords));
  if (visible.length === 0) return <NoMatches query={query} />;
  return (
    <>
      <div className="flex flex-col gap-2 text-sm">
        {visible.map((r) => (
          <Fragment key={r.label}>{r.node}</Fragment>
        ))}
      </div>
      {empty && (
        <p className="text-sm text-muted-foreground">
          Shared with the TypeScript CLI. The General, Journal, and Collectors tabs read and write
          it directly; unknown keys the CLI owns are preserved on save. Theme and Agentboard scan
          roots persist separately.
        </p>
      )}
    </>
  );
}
