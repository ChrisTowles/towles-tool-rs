import type { ReactNode } from "react";
import { Files as FilesIcon, GitCompare, Globe } from "lucide-react";
import { cn } from "@/lib/utils";

/**
 * One header row, shared by every pane kind in the Agentboard tiling.
 *
 * The ordering rule is the whole point: **the lens leads, the subject
 * follows, the folder never appears.** A window is scoped to a single
 * checkout (`AgWindow.folderDir`), so every pane in it shows the same folder
 * — printing that name in each header spends the most prominent slot on the
 * one value guaranteed to be constant, leaving the panes to be told apart by
 * a 14px muted glyph. Repo / folder / branch live once in the working-context
 * band above, which is the rule `PaneHeader` already documented and the diff,
 * files, and preview headers each drifted from by copy-paste.
 *
 * Type is carried by shape and words, never by hue. Violet means "focused /
 * agent" and amber means "needs you" across the app; a per-kind color would
 * put a third meaning on color and weaken both. The lens chip is neutral
 * (`bg-muted`) for every kind except the Claude session, which keeps the
 * violet `✦` it already had.
 */
export function PaneChrome({
  lens,
  subject,
  subjectTitle,
  controls,
  actions,
}: {
  /** The lens chip (see [`PaneLens`]), plus any always-present marker the
   * kind owns — the session panes pass their status `Dot` here. */
  lens: ReactNode;
  /** What is unique to *this* pane: the diff's baseline, the open file, the
   * loaded URL, the session's label. Omitted when the kind has nothing to
   * say yet, which drops the rule with it rather than leaving it dangling. */
  subject?: ReactNode;
  /** Hover text for a subject the row is likely to truncate (a long path). */
  subjectTitle?: string;
  /** Kind-specific controls that sit inline with the subject rather than in
   * the trailing action cluster — the diff pane's baseline toggle. */
  controls?: ReactNode;
  /** Trailing icon buttons, right-aligned. */
  actions: ReactNode;
}) {
  return (
    <div className="flex shrink-0 items-center gap-2 border-b bg-card px-2 py-1">
      {lens}
      {subject != null && (
        <>
          <span className="h-3 w-px shrink-0 bg-border" aria-hidden="true" />
          {/* `flex-1` so the subject claims the row's leftover width instead
           * of only its content width. Without it a `min-w-0 truncate` span
           * sitting beside `shrink-0` controls is the first thing a narrow
           * pane collapses — which is exactly how the old diff header ended
           * up rendering its folder name at zero width. */}
          <span
            className="min-w-0 flex-1 truncate font-mono text-xs text-foreground"
            title={subjectTitle}
          >
            {subject}
          </span>
        </>
      )}
      {controls}
      <span className="ml-auto flex shrink-0 items-center gap-1.5">{actions}</span>
    </div>
  );
}

/** The lens kinds, in the order they read as a family. `agent` and `shell`
 * keep the `✦`/`❯` glyphs the rail already uses for sessions rather than
 * borrowing a lucide icon, so a session pane and its rail row still name
 * themselves the same way. */
export type LensKind = "agent" | "shell" | "diff" | "files" | "web";

const LENSES: Record<LensKind, { label: string; glyph?: string; icon?: typeof GitCompare }> = {
  agent: { label: "claude", glyph: "✦" },
  shell: { label: "shell", glyph: "❯" },
  diff: { label: "diff", icon: GitCompare },
  files: { label: "files", icon: FilesIcon },
  web: { label: "web", icon: Globe },
};

/** The leading chip that names a pane's kind — icon-or-glyph plus the word.
 * The word is what makes two panes of the same checkout distinguishable
 * without reading them, which an icon alone measurably failed to do. */
export function PaneLens({
  kind,
  label,
  title,
}: {
  kind: LensKind;
  /** Overrides the default word — the shell panes pass their real shell
   * (`zsh`, `bash`) when the backend reported one. */
  label?: string;
  title?: string;
}) {
  const lens = LENSES[kind];
  const Icon = lens.icon;
  return (
    <span
      title={title}
      className={cn(
        "flex shrink-0 items-center gap-1 rounded-md bg-muted px-1.5 py-px font-mono text-[10.5px]",
        kind === "agent" ? "text-violet-500" : "text-muted-foreground",
      )}
    >
      {lens.glyph ? (
        <span aria-hidden="true">{lens.glyph}</span>
      ) : (
        Icon && <Icon className="size-3" />
      )}
      {label ?? lens.label}
    </span>
  );
}
