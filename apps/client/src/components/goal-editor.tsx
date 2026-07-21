// The new-task form's goal field: a plain textarea with two GitHub-ish
// affordances layered on — `#` autocompletes the repo's open issues, and URLs
// and `#123` refs are highlighted.
//
// It stays a real <textarea> rather than a contenteditable/rich editor because
// the surrounding form depends on textarea behavior that's painful to
// reimplement: image paste (WebKitGTK delivers it with empty clipboardData —
// see `inline-new-task.tsx`), drag-drop, Cmd+Enter submit, Escape to cancel,
// and native undo. Highlighting is therefore an aria-hidden mirror div sitting
// exactly behind transparent text; the two must keep identical typography,
// padding and wrapping or the colours slide off the words, which is why the
// shared classes live in one constant below.
import { useLayoutEffect, useRef, useState, type ReactNode } from "react";

import { Textarea } from "@/components/ui/textarea";
import {
  highlightSegments,
  applyMention,
  insertMentionTrigger,
  matchIssues,
  mentionQueryAt,
} from "@/lib/goal-text";
import type { IssueItem } from "@/lib/data";
import { cn } from "@/lib/utils";

/** Typography/box metrics shared by the textarea and its highlight mirror.
 * Any change here must stay in both — that's the whole point of the constant.
 *
 * `md:text-xs` is not redundant: shadcn's Textarea base ends in `md:text-sm`,
 * and tailwind-merge only dedupes classes within the same modifier, so a bare
 * `text-xs` loses to it above 768px. The textarea would then render a size
 * larger than the mirror and the caret would drift further right with every
 * character typed. */
const SHARED_BOX = "px-2.5 py-2 text-xs leading-normal md:text-xs";

export function GoalEditor({
  value,
  onChange,
  onKeyDown,
  issues,
  issuesError,
  onNeedIssues,
  onPickIssue,
  hint,
  className,
  ...textareaProps
}: {
  value: string;
  onChange: (next: string) => void;
  /** The form's own key handling (Cmd+Enter submit, Escape cancel). Not called
   * while the mention popup is open and owns the key. */
  onKeyDown?: (e: React.KeyboardEvent<HTMLTextAreaElement>) => void;
  /** Open issues for autocomplete; `null` while loading. */
  issues: IssueItem[] | null;
  issuesError: string | null;
  /** Ask the parent to fetch issues — fired the first time a `#` is typed, so
   * `gh` is never shelled for a goal that has no issue reference. */
  onNeedIssues: () => void;
  /** A picked issue also gets attached to the task, matching the Pick-issue
   * popover; the reference text is inserted by this component. */
  onPickIssue: (issue: IssueItem) => void;
  /** Extra affordance text for the hint row, after the `#` chip. */
  hint?: ReactNode;
  className?: string;
} & Omit<React.ComponentProps<"textarea">, "value" | "onChange" | "onKeyDown">) {
  const ref = useRef<HTMLTextAreaElement>(null);
  const mirror = useRef<HTMLDivElement>(null);
  // The mention being typed, or null. Held as the token's start index plus its
  // query so a pick can replace exactly that span.
  const [mention, setMention] = useState<{ start: number; query: string } | null>(null);
  const [active, setActive] = useState(0);

  const matches = mention && issues ? matchIssues(issues, mention.query).slice(0, 8) : [];
  const open = mention !== null;

  // Keep the mirror's scroll pinned to the textarea's, or the highlights lag
  // behind the text once the field scrolls.
  useLayoutEffect(() => {
    const el = ref.current;
    const m = mirror.current;
    if (el && m) m.scrollTop = el.scrollTop;
  }, [value]);

  function syncMention(el: HTMLTextAreaElement) {
    const found = mentionQueryAt(el.value, el.selectionStart ?? 0);
    setMention(found);
    setActive(0);
    if (found && issues === null) onNeedIssues();
  }

  function pick(issue: IssueItem) {
    const el = ref.current;
    if (!el || !mention) return;
    const next = applyMention(value, mention.start, el.selectionStart ?? 0, issue.number);
    onChange(next.text);
    onPickIssue(issue);
    setMention(null);
    // Restore the caret after React re-renders with the new value; setting it
    // synchronously would be overwritten by the controlled re-render.
    requestAnimationFrame(() => {
      el.focus();
      el.setSelectionRange(next.caret, next.caret);
    });
  }

  /** The hint button's job: start a mention and open the list, so the feature
   * is discoverable by clicking as well as by knowing to type `#`. */
  function startMention() {
    const el = ref.current;
    if (!el) return;
    const caret = el.selectionStart ?? value.length;
    const next = insertMentionTrigger(value, caret);
    onChange(next.text);
    if (issues === null) onNeedIssues();
    // Same reason as pick(): the caret must be set after the controlled
    // re-render, and the mention state has to describe the text we just wrote.
    requestAnimationFrame(() => {
      el.focus();
      el.setSelectionRange(next.caret, next.caret);
      setMention(mentionQueryAt(next.text, next.caret));
      setActive(0);
    });
  }

  return (
    <div className="flex flex-col gap-1">
      <div className="relative">
        <div
          ref={mirror}
          aria-hidden
          className={cn(
            "pointer-events-none absolute inset-0 overflow-hidden rounded-lg border border-transparent whitespace-pre-wrap break-words",
            SHARED_BOX,
          )}
        >
          {/* Highlight styles may only change *paint*, never metrics: padding,
            margin, font-weight, letter-spacing or borders here would reflow the
            mirror out from under the textarea's caret. Colour, background and
            text-decoration are safe; a radius is too, with no border width. */}
          {highlightSegments(value).map((seg, i) => (
            <span
              key={i}
              className={
                seg.kind === "url"
                  ? "text-sky-600 underline decoration-sky-600/60 dark:text-sky-400 dark:decoration-sky-400/60"
                  : seg.kind === "ref"
                    ? "rounded bg-sky-500/20 text-sky-700 dark:text-sky-300"
                    : undefined
              }
            >
              {seg.text}
            </span>
          ))}
          {/* A trailing newline collapses without this, so the mirror ends one
            line short of the textarea while typing at the end. */}
          {value.endsWith("\n") ? " " : null}
        </div>
        <Textarea
          {...textareaProps}
          ref={ref}
          value={value}
          // Transparent text with a visible caret: the mirror underneath supplies
          // the glyphs, and `relative` keeps the field above it for input.
          className={cn(
            "relative bg-transparent text-transparent caret-foreground",
            SHARED_BOX,
            className,
          )}
          onChange={(e) => {
            onChange(e.target.value);
            syncMention(e.target);
          }}
          onScroll={(e) => {
            if (mirror.current) mirror.current.scrollTop = e.currentTarget.scrollTop;
          }}
          onClick={(e) => syncMention(e.currentTarget)}
          onBlur={() => setMention(null)}
          onKeyDown={(e) => {
            // While the popup owns the keyboard, the form must not see these —
            // Enter would submit the task and Escape would close the whole form.
            if (open && matches.length > 0) {
              if (e.key === "ArrowDown") {
                e.preventDefault();
                setActive((a) => (a + 1) % matches.length);
                return;
              }
              if (e.key === "ArrowUp") {
                e.preventDefault();
                setActive((a) => (a - 1 + matches.length) % matches.length);
                return;
              }
              if (e.key === "Enter" || e.key === "Tab") {
                e.preventDefault();
                pick(matches[active]);
                return;
              }
            }
            if (open && e.key === "Escape") {
              e.preventDefault();
              setMention(null);
              return;
            }
            onKeyDown?.(e);
          }}
          onKeyUp={(e) => syncMention(e.currentTarget)}
        />
        {open && (
          <div className="absolute top-full left-0 z-50 mt-1 w-full overflow-hidden rounded-md border border-border bg-popover shadow-md">
            {issuesError ? (
              <p className="p-2 text-[11px] text-red-500">{issuesError}</p>
            ) : issues === null ? (
              <p className="p-2 text-[11px] text-muted-foreground">Loading issues…</p>
            ) : matches.length === 0 ? (
              <p className="p-2 text-[11px] text-muted-foreground">No matching issues.</p>
            ) : (
              matches.map((issue, i) => (
                <button
                  key={issue.number}
                  type="button"
                  // The textarea's blur would close the popup before the click
                  // lands, so commit on mousedown instead.
                  onMouseDown={(e) => {
                    e.preventDefault();
                    pick(issue);
                  }}
                  onMouseEnter={() => setActive(i)}
                  className={cn(
                    "flex w-full items-baseline gap-2 px-2 py-1.5 text-left",
                    i === active && "bg-accent",
                  )}
                >
                  <span className="shrink-0 font-mono text-[10.5px] text-muted-foreground">
                    #{issue.number}
                  </span>
                  <span className="truncate text-xs">{issue.title}</span>
                </button>
              ))
            )}
          </div>
        )}
      </div>
      {/* Persistent, because the placeholder that used to carry this vanishes
          on the first keystroke — which is exactly when someone is composing a
          goal and might want to reference an issue. */}
      <p className="flex items-center gap-1.5 text-[10.5px] text-muted-foreground">
        <button
          type="button"
          aria-label="Link an issue"
          // mousedown, not click: a plain click blurs the textarea first, and
          // onBlur closes the mention we are trying to open.
          onMouseDown={(e) => {
            e.preventDefault();
            startMention();
          }}
          className="rounded bg-sky-500/20 px-1 font-mono text-sky-700 hover:bg-sky-500/35 dark:text-sky-300"
        >
          #
        </button>
        <span>to link an issue</span>
        {hint ? <span className="text-muted-foreground/70">· {hint}</span> : null}
      </p>
    </div>
  );
}
