// New-slot modal: give a goal, pick the base branch, and a branch-named
// worktree slot is created under the repo root's slots/ dir (`slot_create` →
// tt-slots ops, shared with `tt slot new`). The goal slugs the branch name
// (editable) and the caller launches Claude with it in the new slot's first
// session.
import { Sparkles, Undo2 } from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import {
  Command,
  CommandEmpty,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Textarea } from "@/components/ui/textarea";
import { BaseBranchesSchema, SlotCreatedSchema } from "@/lib/schemas/slots";
import { invokeOrThrow } from "@/lib/tauri";

export type NewSlotRepo = { name: string; dir: string };

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

export function NewSlotDialog({
  repo,
  onClose,
  onCreated,
}: {
  /** The repo to create a slot for (any of its slot dirs); null = closed. */
  repo: NewSlotRepo | null;
  onClose: () => void;
  /** Called after the slot exists; the caller opens a session + launches Claude. */
  onCreated: (created: SlotCreated, goal: string) => void | Promise<void>;
}) {
  const [goal, setGoal] = useState("");
  const [branchEdit, setBranchEdit] = useState<string | null>(null);
  const [base, setBase] = useState("");
  const [branches, setBranches] = useState<string[]>([]);
  const [baseOpen, setBaseOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [branchCheck, setBranchCheck] = useState<BranchCheck | null>(null);
  const [suggesting, setSuggesting] = useState(false);
  const [creatingTemplate, setCreatingTemplate] = useState(false);
  // What the goal/branch fields held right before the last accepted
  // suggestion overwrote them — lets "Undo" put them back exactly.
  const [preSuggest, setPreSuggest] = useState<{ goal: string; branchEdit: string | null } | null>(
    null,
  );

  const sortedBranches = [...branches].sort((a, b) => a.localeCompare(b));

  const branch = branchEdit ?? goalToBranch(goal);

  useEffect(() => {
    if (!repo) return;
    setGoal("");
    setBranchEdit(null);
    setError(null);
    setBranchCheck(null);
    setBusy(false);
    setSuggesting(false);
    setPreSuggest(null);
    setCreatingTemplate(false);
    invokeOrThrow<string[]>("slot_base_branches", { root: repo.dir }, BaseBranchesSchema)
      .then((list) => {
        setBranches(list);
        setBase(list[0] ?? "main");
      })
      .catch((e) => setError(String(e)));
  }, [repo]);

  // Debounced preflight: is `branch` a legal git ref, and would its derived
  // slot name collide with an existing one? Cheap and read-only, so it's
  // safe to fire on every settled keystroke rather than only at submit time.
  useEffect(() => {
    if (!repo || !branch) {
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
  }, [repo, branch]);

  const branchProblem =
    branchCheck?.error ?? (branchCheck?.taken ? `a slot named "${branchCheck.name}" already exists` : null);

  // The setup step (npm install/etc.) can fail without invalidating the slot
  // itself — `slot_create`'s warning already says so. Give it a one-click
  // retry rather than making the user remember to re-run it from a terminal.
  async function retrySetup(dir: string) {
    try {
      const warning = await invokeOrThrow<string | null>("slot_run_setup", { dir });
      if (warning) toast(warning, { action: retryAction(dir) });
      else toast("setup succeeded");
    } catch (e) {
      toast(String(e));
    }
  }

  function retryAction(dir: string) {
    return { label: "Retry", onClick: () => void retrySetup(dir) };
  }

  // Manual only — never runs on a timer or keystroke. Asks claude -p (cwd =
  // the repo, so it has real repo context) to propose a better branch name
  // and a cleaned-up goal, then fills both editable fields directly. The
  // fields stay editable (or "Undo" puts back exactly what was there) —
  // that's the confirmation step, not a separate accept/reject panel.
  async function suggest() {
    if (!repo || suggesting || !goal.trim()) return;
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

  // `slot_create`'s "no template" error means this repo has neither a
  // tokenized .env.example nor the root-side sidecar — offer to create an
  // empty sidecar (comment-only, no ${tt:...} tokens) right from the dialog
  // instead of sending the user to a terminal, then retry immediately.
  const noTemplate = error?.startsWith("no template:") ?? false;

  async function createTemplateAndRetry() {
    if (!repo || creatingTemplate) return;
    setCreatingTemplate(true);
    try {
      await invokeOrThrow("slot_init_template", { root: repo.dir });
      setError(null);
      await create();
    } catch (e) {
      setError(String(e));
    } finally {
      setCreatingTemplate(false);
    }
  }

  async function create() {
    if (!repo || busy) return;
    if (!branch) {
      setError("Give a goal (or type a branch name) first.");
      return;
    }
    if (branchProblem) {
      setError(branchProblem);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const created = await invokeOrThrow<SlotCreated>(
        "slot_create",
        { root: repo.dir, branch, base },
        SlotCreatedSchema,
      );
      for (const warning of created.warnings) {
        toast(warning, warning.startsWith("setup `") ? { action: retryAction(created.dir) } : undefined);
      }
      onClose();
      await onCreated(created, goal.trim());
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  }

  return (
    <Dialog open={repo != null} onOpenChange={(open) => !open && !busy && onClose()}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>⬢ New slot{repo ? ` — ${repo.name}` : ""}</DialogTitle>
          <DialogDescription>
            Creates a worktree slot named after the branch, claims its ports, and starts
            Claude on your goal.
          </DialogDescription>
        </DialogHeader>
        <Textarea
          autoFocus
          value={goal}
          onChange={(e) => setGoal(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
              e.preventDefault();
              void create();
            }
          }}
          placeholder="what should get built in this slot?"
          rows={3}
        />
        <div className="flex items-center justify-end gap-2">
          {preSuggest && (
            <Button variant="ghost" size="sm" className="gap-1 text-xs" onClick={undoSuggest}>
              <Undo2 className="size-3" />
              Undo suggestion
            </Button>
          )}
          <Button
            variant="outline"
            size="sm"
            className="gap-1 text-xs"
            disabled={suggesting || !goal.trim()}
            onClick={() => void suggest()}
          >
            <Sparkles className="size-3" />
            {suggesting ? "Asking claude…" : "Suggest name + goal"}
          </Button>
        </div>
        <div className="flex min-w-0 flex-col gap-1">
          <div className="flex min-w-0 items-center gap-2">
            <span className="w-14 shrink-0 text-[11px] text-muted-foreground">branch</span>
            <Input
              value={branch}
              onChange={(e) => setBranchEdit(e.target.value)}
              placeholder={`leave blank to auto-generate from your goal (first ${BRANCH_SLUG_SOURCE_CHARS} chars, made branch-safe)`}
              className="min-w-0 flex-1 font-mono text-xs"
            />
          </div>
          {!branchEdit && (
            <p className="pl-16 text-[11px] text-muted-foreground">
              auto-generated from the first {BRANCH_SLUG_SOURCE_CHARS} characters of your goal —
              type here to override
            </p>
          )}
        </div>
        <div className="flex min-w-0 items-center gap-2">
          <span className="w-14 shrink-0 text-[11px] text-muted-foreground">base</span>
          <Popover open={baseOpen} onOpenChange={setBaseOpen}>
            <PopoverTrigger asChild>
              <Button
                variant="outline"
                role="combobox"
                aria-expanded={baseOpen}
                className="min-w-0 flex-1 shrink justify-start truncate font-mono text-xs font-normal"
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
        {error && (
          <div className="flex flex-wrap items-center gap-2">
            <p className="text-xs text-red-500">{error}</p>
            {noTemplate && (
              <Button
                variant="outline"
                size="sm"
                className="h-6 gap-1 px-2 text-[11px]"
                disabled={creatingTemplate}
                onClick={() => void createTemplateAndRetry()}
              >
                {creatingTemplate ? "Creating template…" : "Create empty slot-env.template"}
              </Button>
            )}
          </div>
        )}
        <div className="flex items-center justify-end gap-2">
          <Button variant="ghost" size="sm" disabled={busy} onClick={onClose}>
            Cancel
          </Button>
          <Button size="sm" disabled={busy || !branch} onClick={() => void create()}>
            {busy ? "Creating… (setup can take a minute)" : "Create slot"}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
