// A reusable editor for a list of user-editable prompt templates, each a card
// with an enable switch, a label, a read-only id badge, a big prompt textarea,
// and a Remove button — plus a heading, help text, and an Add button. Extracted
// from the calendar-sources editor so the same shape drives both the calendar
// collector's per-source prompts and the new-task form's prompt improvers.
//
// Controlled: the parent owns the list and its persistence (this repo's
// autosave `useUserSettings`). This component renders and reports edits; the
// parent assigns ids and emits the `uiAction` telemetry on add/remove, since
// only it knows the store lane / settings key the ids belong to.
import type { ReactNode } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";

/** The minimum shape every prompt-template item shares. Callers may carry more
 * fields on their own item type — this list only reads and patches these. */
export type PromptTemplateItem = {
  id: string;
  label: string;
  enabled: boolean;
  prompt: string;
};

export function PromptTemplateList<T extends PromptTemplateItem>({
  items,
  onChange,
  onCommit,
  onAdd,
  onRemove,
  heading,
  description,
  addLabel,
  emptyText,
  labelPlaceholder = "Label",
  promptPlaceholder,
  promptRows = 5,
  idTitle = "Settings key",
  enableVerb = (item) => `Enable ${item.label || item.id}`,
  rowWarning,
}: {
  items: T[];
  /** Report an edited list. `defer` debounces writes behind typed fields. */
  onChange: (items: T[], opts?: { defer?: boolean }) => void;
  /** Commit the debounced write behind the typed label/prompt fields (on blur). */
  onCommit?: () => void;
  /** Append a new item. The parent assigns the id and emits telemetry. */
  onAdd: () => void;
  /** Remove the item at `index`. The parent emits telemetry. */
  onRemove: (index: number) => void;
  heading: string;
  description: ReactNode;
  addLabel: string;
  /** Shown in place of the list when there are no items. */
  emptyText: string;
  labelPlaceholder?: string;
  promptPlaceholder: string;
  promptRows?: number;
  /** Tooltip on the read-only id badge. */
  idTitle?: string;
  /** aria-label for a row's enable switch. */
  enableVerb?: (item: T) => string;
  /** Optional per-row warning (e.g. enabled but empty), shown under the prompt. */
  rowWarning?: (item: T) => string | null;
}) {
  const patch = (index: number, next: Partial<T>, opts?: { defer?: boolean }) =>
    onChange(
      items.map((it, i) => (i === index ? { ...it, ...next } : it)),
      opts,
    );

  return (
    <div className="flex flex-col gap-3">
      <div>
        <div className="text-sm font-medium">{heading}</div>
        <div className="text-sm text-muted-foreground">{description}</div>
      </div>

      {items.length === 0 ? (
        <div className="rounded-md border border-dashed p-3 text-sm text-muted-foreground">
          {emptyText}
        </div>
      ) : null}

      {items.map((item, index) => {
        const warning = rowWarning?.(item) ?? null;
        return (
          <div key={item.id} className="flex flex-col gap-2 rounded-md border p-3">
            <div className="flex items-center gap-2">
              <Switch
                checked={item.enabled}
                onCheckedChange={(v) => patch(index, { enabled: v } as Partial<T>)}
                aria-label={enableVerb(item)}
              />
              <Input
                value={item.label}
                onChange={(e) =>
                  patch(index, { label: e.target.value } as Partial<T>, { defer: true })
                }
                onBlur={onCommit}
                placeholder={labelPlaceholder}
                aria-label="Label"
                className="h-8 max-w-56"
              />
              <span className="font-mono text-xs text-muted-foreground" title={idTitle}>
                {item.id}
              </span>
              <Button
                variant="ghost"
                size="sm"
                className="ml-auto text-muted-foreground"
                onClick={() => onRemove(index)}
              >
                Remove
              </Button>
            </div>
            <Textarea
              value={item.prompt}
              onChange={(e) =>
                patch(index, { prompt: e.target.value } as Partial<T>, { defer: true })
              }
              onBlur={onCommit}
              placeholder={promptPlaceholder}
              spellCheck={false}
              rows={promptRows}
              className="font-mono text-xs"
              aria-label={`Prompt for ${item.label || item.id}`}
            />
            {warning ? <div className="text-xs text-destructive">{warning}</div> : null}
          </div>
        );
      })}

      <div>
        <Button variant="outline" size="sm" onClick={onAdd}>
          {addLabel}
        </Button>
      </div>
    </div>
  );
}
