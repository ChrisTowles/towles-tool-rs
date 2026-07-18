// Inline new-slot flow: give a goal, pick the base branch, and a branch-named
// worktree slot is created under the repo's .claude/worktrees/ dir
// (`slot_create` → tt-slots ops, shared with `tt slot new`). The goal slugs the branch name
// (editable). Unlike the old modal this never blocks the rest of the rail —
// the form hands off to the caller on submit (which fires `slot_create`
// without awaiting it here) and a `PendingSlotRow` tracks the in-flight
// create until it resolves, so switching to other repos/sessions while a
// slot is being created just works.
import { AlertTriangle, Paperclip, RefreshCw, Sparkles, Undo2, X } from "lucide-react";
import { useEffect, useState } from "react";

import { Button } from "@/components/ui/button";
import {
  Command,
  CommandEmpty,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import {
  ClaudeEffort,
  ClaudeLaunchOptions,
  ClaudeModel,
  DEFAULT_CLAUDE_EFFORT,
  DEFAULT_CLAUDE_MODEL,
  PastedImage,
  fmtElapsed,
  imagesFromDataTransfer,
  isPasteableImage,
} from "@/lib/agentboard";
import { BaseBranchesSchema } from "@/lib/schemas/slots";
import { invokeOrThrow } from "@/lib/tauri";
import { cn } from "@/lib/utils";

const MODEL_OPTIONS: { value: ClaudeModel; label: string }[] = [
  { value: "sonnet", label: "Sonnet" },
  { value: "opus", label: "Opus" },
  { value: "fable", label: "Fable" },
];

const EFFORT_OPTIONS: { value: ClaudeEffort; label: string }[] = [
  { value: "low", label: "Low" },
  { value: "medium", label: "Medium" },
  { value: "high", label: "High" },
  { value: "xhigh", label: "XHigh" },
  { value: "max", label: "Max" },
];

export type NewSlotRepo = { name: string; dir: string; key: string };

/** Mirrors the Rust `SlotCreated` payload from `slot_create`. */
export type SlotCreated = {
  name: string;
  dir: string;
  branch: string;
  base: string;
  warnings: string[];
};

/** Mirrors the Rust `BranchCheck` payload from `slot_check_branch`. */
export type BranchCheck = {
  name: string | null;
  taken: boolean;
  error: string | null;
};

/** Mirrors the Rust `SlotSuggestion` payload from `slot_suggest`. */
export type SlotSuggestion = {
  branch: string;
  goal: string;
};

/** A `slot_create` call that's been fired and is running in the background —
 * tracked in the rail as a `PendingSlotRow` instead of a blocking spinner, so
 * the caller (agentboard.tsx) can keep several of these in flight across
 * different repos at once. Keyed by `${repoKey}::${branch}`, which is unique
 * enough since a branch collision is already rejected before submit. */
export type PendingSlot = {
  id: string;
  repoKey: string;
  repoDir: string;
  repoName: string;
  goal: string;
  branch: string;
  base: string;
  options: ClaudeLaunchOptions;
  /** Carried on the pending row, not just consumed at submit, so a retry
   * after a failed create re-attaches the same images — the form is long
   * gone by then and the user would otherwise have to re-paste. */
  images: PastedImage[];
  startedAt: number;
  status: "creating" | "error";
  error?: string;
};

/** How much of the goal `goalToBranch` slugs into the branch name — long
 * enough to stay recognizable, short enough that the branch name doesn't
 * become a second copy of the whole goal. */
export const BRANCH_SLUG_SOURCE_CHARS = 50;

/** Goal → branch name, mirroring tt-git's slug rules (lowercase, spaces and
 * non `[0-9a-z_-]` to `-`, collapse runs, strip trailing) under a `feat/`
 * prefix, from just the first `BRANCH_SLUG_SOURCE_CHARS` of the goal. The
 * branch field stays editable — this is just the default. */
export function goalToBranch(goal: string): string {
  let slug = goal
    .slice(0, BRANCH_SLUG_SOURCE_CHARS)
    .toLowerCase()
    .trim()
    .replaceAll(" ", "-");
  slug = slug.replace(/[^0-9a-z_-]/g, "-");
  slug = slug.replace(/-+/g, "-");
  slug = slug.replace(/-+$/, "");
  return slug ? `feat/${slug}` : "";
}

/** The inline goal/branch/base form, embedded directly in the rail under the
 * repo (or, for a solo repo, the merged repo+folder) header whose "+" opened
 * it. Submitting hands the collected input to `onSubmit` and closes — it does
 * not itself wait on `slot_create`, so the parent is free to run that call in
 * the background and represent it with a `PendingSlotRow` instead. */
export function InlineNewSlot({
  repo,
  onCancel,
  onSubmit,
}: {
  repo: NewSlotRepo;
  onCancel: () => void;
  onSubmit: (input: {
    goal: string;
    branch: string;
    base: string;
    options: ClaudeLaunchOptions;
    images: PastedImage[];
  }) => void;
}) {
  const [goal, setGoal] = useState("");
  const [images, setImages] = useState<PastedImage[]>([]);
  const [branchEdit, setBranchEdit] = useState<string | null>(null);
  const [base, setBase] = useState("");
  const [model, setModel] = useState<ClaudeModel>(DEFAULT_CLAUDE_MODEL);
  const [effort, setEffort] = useState<ClaudeEffort>(DEFAULT_CLAUDE_EFFORT);
  const [branches, setBranches] = useState<string[]>([]);
  const [baseOpen, setBaseOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [branchCheck, setBranchCheck] = useState<BranchCheck | null>(null);
  const [suggesting, setSuggesting] = useState(false);
  // What the goal/branch fields held right before the last accepted
  // suggestion overwrote them — lets "Undo" put them back exactly.
  const [preSuggest, setPreSuggest] = useState<{ goal: string; branchEdit: string | null } | null>(
    null,
  );

  const sortedBranches = [...branches].sort((a, b) => a.localeCompare(b));

  const branch = branchEdit ?? goalToBranch(goal);

  useEffect(() => {
    // Guarded like the branchCheck effect below: the caller unmounts this
    // form on cancel/submit rather than reusing one instance across opens,
    // so this mainly guards a fast close-then-reopen of the same repo's form
    // against a stale fetch's `.then` landing after a fresh one already has.
    let cancelled = false;
    invokeOrThrow<string[]>("slot_base_branches", { root: repo.dir }, BaseBranchesSchema)
      .then((list) => {
        if (cancelled) return;
        setBranches(list);
        setBase(list[0] ?? "main");
      })
      .catch((e) => !cancelled && setError(String(e)));
    return () => {
      cancelled = true;
    };
    // Only re-fetch if the repo this form is open for changes — the fields
    // themselves start empty once per mount, which is once per open (the
    // caller unmounts the form on cancel/submit rather than hiding it).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [repo.dir]);

  // Debounced preflight: is `branch` a legal git ref, and would its derived
  // slot name collide with an existing one? Cheap and read-only, so it's
  // safe to fire on every settled keystroke rather than only at submit time.
  useEffect(() => {
    if (!branch) {
      setBranchCheck(null);
      return;
    }
    let cancelled = false;
    const timer = setTimeout(() => {
      invokeOrThrow<BranchCheck>("slot_check_branch", { root: repo.dir, branch })
        .then((check) => !cancelled && setBranchCheck(check))
        .catch(() => !cancelled && setBranchCheck(null));
    }, 300);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [repo.dir, branch]);

  function cancel() {
    onCancel();
  }

  const branchProblem =
    branchCheck?.error ?? (branchCheck?.taken ? `a slot named "${branchCheck.name}" already exists` : null);

  // Manual only — never runs on a timer or keystroke. Asks claude -p (cwd =
  // the repo, so it has real repo context) to propose a better branch name
  // and a cleaned-up goal, then fills both editable fields directly. The
  // fields stay editable (or "Undo" puts back exactly what was there) —
  // that's the confirmation step, not a separate accept/reject panel.
  async function suggest() {
    if (suggesting || !goal.trim()) return;
    setSuggesting(true);
    setError(null);
    try {
      const suggestion = await invokeOrThrow<SlotSuggestion>("slot_suggest", {
        dir: repo.dir,
        goal,
      });
      setPreSuggest({ goal, branchEdit });
      setGoal(suggestion.goal);
      setBranchEdit(suggestion.branch);
    } catch (e) {
      setError(String(e));
    } finally {
      setSuggesting(false);
    }
  }

  function undoSuggest() {
    if (!preSuggest) return;
    setGoal(preSuggest.goal);
    setBranchEdit(preSuggest.branchEdit);
    setPreSuggest(null);
  }

  // Screenshots are how a lot of goals actually get described ("make it look
  // like this", "this is the error"), so the goal field takes an image paste
  // directly. The bytes are held here until submit, then staged as files
  // outside the repo (`tt_slots::pasted`) whose paths go into Claude's
  // opening prompt.
  async function pasteImages(data: DataTransfer | null) {
    try {
      const pasted = await imagesFromDataTransfer(data);
      if (pasted.length) {
        setImages((prev) => [...prev, ...pasted]);
        setError(null);
      }
    } catch (e) {
      setError(String(e));
    }
  }

  function removeImage(id: string) {
    setImages((prev) => prev.filter((img) => img.id !== id));
  }

  function submit() {
    if (!branch) {
      setError("Give a goal (or type a branch name) first.");
      return;
    }
    if (branchProblem) {
      setError(branchProblem);
      return;
    }
    onSubmit({ goal: goal.trim(), branch, base, options: { model, effort }, images });
  }

  return (
    <div className="mx-3 my-1.5 flex flex-col gap-2 rounded-lg border border-border bg-card p-2.5">
      <span className="text-[11px] font-medium text-muted-foreground">
        ⬢ New slot — {repo.name}
      </span>
      <Textarea
        autoFocus
        value={goal}
        onChange={(e) => setGoal(e.target.value)}
        onPaste={(e) => {
          // Only intercept an image paste — a text paste falls through to the
          // textarea's own handling untouched.
          const images = Array.from(e.clipboardData?.items ?? []).filter(
            (it) => it.kind === "file" && it.type.startsWith("image/"),
          );
          if (!images.length) return;
          e.preventDefault();
          // An image type we can't write (SVG, say) would otherwise vanish
          // silently — the paste is already swallowed by the preventDefault
          // above, so say why rather than looking like nothing happened.
          if (!images.some((it) => isPasteableImage(it.type))) {
            setError(`Can't attach ${images[0].type} — paste a PNG, JPEG, GIF, or WebP.`);
            return;
          }
          void pasteImages(e.clipboardData);
        }}
        onDragOver={(e) => e.preventDefault()}
        onDrop={(e) => {
          if (!Array.from(e.dataTransfer?.items ?? []).some((it) => it.kind === "file")) return;
          e.preventDefault();
          void pasteImages(e.dataTransfer);
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
            e.preventDefault();
            submit();
          }
          if (e.key === "Escape") cancel();
        }}
        placeholder="what should get built in this slot? (paste a screenshot to attach it)"
        rows={2}
        className="text-xs"
      />
      {images.length > 0 && (
        <div className="flex flex-wrap gap-1.5">
          {images.map((img) => (
            <div key={img.id} className="group relative">
              <img
                src={img.previewUrl}
                alt={img.name}
                title={`${img.name} — attached to the new slot's first prompt`}
                className="size-12 rounded border border-border object-cover"
              />
              <button
                type="button"
                aria-label={`Remove ${img.name}`}
                onClick={() => removeImage(img.id)}
                className="absolute -top-1 -right-1 rounded-full border border-border bg-background p-0.5 text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100 hover:text-foreground focus-visible:opacity-100"
              >
                <X className="size-2.5" />
              </button>
            </div>
          ))}
        </div>
      )}
      <div className="flex items-center justify-end gap-2">
        {preSuggest && (
          <Button variant="ghost" size="sm" className="h-6 gap-1 px-1.5 text-[10.5px]" onClick={undoSuggest}>
            <Undo2 className="size-3" />
            Undo
          </Button>
        )}
        <Button
          variant="outline"
          size="sm"
          className="h-6 gap-1 px-1.5 text-[10.5px]"
          disabled={suggesting || !goal.trim()}
          onClick={() => void suggest()}
        >
          <Sparkles className="size-3" />
          {suggesting ? "Asking claude…" : "Suggest name + goal"}
        </Button>
      </div>
      <div className="flex flex-col gap-1">
        <span className="text-[10.5px] text-muted-foreground">branch</span>
        <Input
          value={branch}
          onChange={(e) => setBranchEdit(e.target.value)}
          placeholder="auto-generated from your goal"
          className="min-w-0 font-mono text-xs"
        />
      </div>
      <div className="flex flex-col gap-1">
        <span className="text-[10.5px] text-muted-foreground">base</span>
        <Popover open={baseOpen} onOpenChange={setBaseOpen}>
          <PopoverTrigger asChild>
            <Button
              variant="outline"
              role="combobox"
              aria-expanded={baseOpen}
              className="min-w-0 justify-start truncate font-mono text-xs font-normal"
            >
              <span className="truncate">{base || "main"}</span>
            </Button>
          </PopoverTrigger>
          <PopoverContent className="w-(--radix-popover-trigger-width) p-0">
            <Command>
              <CommandInput placeholder="Search branches…" />
              <CommandList>
                <CommandEmpty>No branches found.</CommandEmpty>
                {sortedBranches.map((b) => (
                  <CommandItem
                    key={b}
                    value={b}
                    className="min-w-0 truncate font-mono text-xs"
                    onSelect={(value) => {
                      setBase(value);
                      setBaseOpen(false);
                    }}
                  >
                    <span className="truncate">{b}</span>
                  </CommandItem>
                ))}
              </CommandList>
            </Command>
          </PopoverContent>
        </Popover>
      </div>
      <div className="flex items-center gap-2">
        <Select value={model} onValueChange={(v) => setModel(v as ClaudeModel)}>
          <SelectTrigger className="min-w-0 flex-1 font-mono text-xs">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {MODEL_OPTIONS.map((o) => (
              <SelectItem key={o.value} value={o.value}>
                {o.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Select value={effort} onValueChange={(v) => setEffort(v as ClaudeEffort)}>
          <SelectTrigger className="min-w-0 flex-1 font-mono text-xs">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {EFFORT_OPTIONS.map((o) => (
              <SelectItem key={o.value} value={o.value}>
                {o.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      {error && <p className="text-[11px] text-red-500">{error}</p>}
      <div className="flex items-center justify-end gap-2">
        <Button variant="ghost" size="sm" onClick={cancel}>
          Cancel
        </Button>
        <Button size="sm" disabled={!branch} onClick={submit}>
          Create slot
        </Button>
      </div>
    </div>
  );
}

/** A `slot_create` call in flight (or failed), rendered inline in the rail at
 * the same tier as a `FolderHeader` — the new folder it'll become once the
 * worktree + setup finish. Never resizes the layout around a modal; the rest
 * of the rail stays fully interactive while this sits here. */
export function PendingSlotRow({
  pending,
  now,
  onRetry,
  onDismiss,
  onCreateTemplate,
}: {
  pending: PendingSlot;
  now: number;
  onRetry: (id: string) => void;
  onDismiss: (id: string) => void;
  onCreateTemplate: (id: string) => void;
}) {
  const noTemplate = pending.error?.startsWith("no template:") ?? false;
  return (
    <div
      className={cn(
        "flex flex-col gap-1 border-b border-l-2 border-border py-1.5 pr-3 pl-6",
        pending.status === "error" ? "border-l-amber-500" : "border-l-transparent",
      )}
    >
      <div className="flex min-w-0 items-center gap-2">
        {pending.status === "creating" ? (
          <RefreshCw className="size-3.5 shrink-0 animate-spin text-muted-foreground" />
        ) : (
          <AlertTriangle className="size-3.5 shrink-0 text-amber-500" />
        )}
        <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-muted-foreground">
          ⎇ {pending.branch}
        </span>
        {pending.images.length > 0 && (
          <span
            title={`${pending.images.length} pasted image${pending.images.length === 1 ? "" : "s"} — attached to this slot's first prompt, and kept for a retry`}
            className="flex shrink-0 items-center gap-0.5 font-mono text-[10.5px] text-muted-foreground/70"
          >
            <Paperclip className="size-2.5" />
            {pending.images.length}
          </span>
        )}
        <span className="shrink-0 font-mono text-[10.5px] text-muted-foreground/70">
          {fmtElapsed(now - pending.startedAt)}
        </span>
      </div>
      {pending.status === "creating" ? (
        <span className="pl-[22px] text-[11px] text-muted-foreground/70">creating slot…</span>
      ) : (
        <div className="flex flex-wrap items-center gap-2 pl-[22px]">
          <span className="text-[11px] text-red-500">{pending.error}</span>
          {noTemplate ? (
            <Button
              size="sm"
              variant="outline"
              className="h-5 gap-1 px-1.5 text-[10.5px]"
              onClick={() => onCreateTemplate(pending.id)}
            >
              Create empty slot-env.template
            </Button>
          ) : (
            <Button
              size="sm"
              variant="outline"
              className="h-5 gap-1 px-1.5 text-[10.5px]"
              onClick={() => onRetry(pending.id)}
            >
              Retry
            </Button>
          )}
          <Button
            size="sm"
            variant="ghost"
            className="h-5 px-1.5 text-[10.5px]"
            onClick={() => onDismiss(pending.id)}
          >
            Dismiss
          </Button>
        </div>
      )}
    </div>
  );
}
