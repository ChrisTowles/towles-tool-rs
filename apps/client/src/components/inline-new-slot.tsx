// Inline new-slot flow: give a goal, pick the base branch, and a branch-named
// worktree slot is created under the repo's .claude/worktrees/ dir
// (`slot_create` → tt-slots ops, shared with `tt slot new`). The goal slugs the branch name
// (editable). Unlike the old modal this never blocks the rest of the rail —
// the form hands off to the caller on submit (which fires `slot_create`
// without awaiting it here) and a `PendingSlotRow` tracks the in-flight
// create until it resolves, so switching to other repos/sessions while a
// slot is being created just works.
import {
  AlertTriangle,
  Check,
  CircleDot,
  ImagePlus,
  Paperclip,
  RefreshCw,
  Sparkles,
  Undo2,
  X,
} from "lucide-react";
import { useEffect, useState } from "react";

import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
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
  PastedImage,
  clipboardImageFromHost,
  fmtElapsed,
  imagesFromDataTransfer,
  isPasteableImage,
  nextDraftScopeId,
} from "@/lib/agentboard";
import { IssueItem, storeGhIssuesList } from "@/lib/data";
import { errorMessage } from "@/lib/errors";
import { type BaseBranch, BaseBranchesSchema, PastedImagePathsSchema } from "@/lib/schemas/slots";
import { invoke } from "@/lib/tauri";
import { uiAction } from "@/lib/ui-action";
import { cn } from "@/lib/utils";
import { slugify } from "@/lib/slug";

/** The unset state of the model/effort selects: no `--model`/`--effort` is
 * passed at all, so the user's own Claude config decides. Its own option
 * (rather than an empty value) because Radix `Select` can't represent "". */
const USE_DEFAULT = "default";

type ModelChoice = ClaudeModel | typeof USE_DEFAULT;
type EffortChoice = ClaudeEffort | typeof USE_DEFAULT;

const MODEL_OPTIONS: { value: ModelChoice; label: string }[] = [
  { value: USE_DEFAULT, label: "Default model" },
  { value: "sonnet", label: "Sonnet" },
  { value: "opus", label: "Opus" },
  { value: "fable", label: "Fable" },
];

const EFFORT_OPTIONS: { value: EffortChoice; label: string }[] = [
  { value: USE_DEFAULT, label: "Default effort" },
  { value: "low", label: "Low" },
  { value: "medium", label: "Medium" },
  { value: "high", label: "High" },
  { value: "xhigh", label: "XHigh" },
  { value: "max", label: "Max" },
];

export type NewSlotRepo = {
  name: string;
  dir: string;
  key: string;
  /** The repo's git origin URL when known — parsed to `owner/name` so the
   * created task's slot binding can auto-attach PRs by branch. */
  originUrl?: string | null;
};

/** What the new-task form hands its parent on submit. */
export type NewTaskSubmit = {
  goal: string;
  branch: string;
  base: string;
  options: ClaudeLaunchOptions;
  /** Absolute paths of the already-staged images, not the bytes — they were
   * written to disk when pasted. */
  imagePaths: string[];
  /** GitHub issues to attach to the created task (multi-select). */
  issues: IssueItem[];
  /** False for "Task only": create the board task but no worktree/agent. */
  worktree: boolean;
  /** True for a dynamic task: Claude launches in plan mode, and once the
   * user approves the plan in the PTY it delivers all the way to a merged
   * PR (`dynamicFlowPrompt`) — the merge is what closes the board task. */
  dynamic: boolean;
};

/** Mirrors the Rust `SlotCreated` payload from `slot_create`. */
export type SlotCreated = {
  name: string;
  dir: string;
  branch: string;
  base: string;
  /** The ref the slot effectively branched from — see `SlotCreatedSchema`. */
  baseLabel: string;
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
  /** Set when claude couldn't answer and the fields were filled from a
   * locally derived slug instead. A note, not an error — the suggestion is
   * still usable, so it renders muted rather than red. */
  fallback: string | null;
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
   * gone by then and the user would otherwise have to re-paste. Paths, not
   * bytes: the files were staged when pasted and outlive the form. */
  imagePaths: string[];
  /** The board task created at submit (#339) — carried so a retry binds the
   * slot to the same task instead of minting a duplicate card. */
  taskId?: number;
  /** Carried so a retry launches the same flow the submit asked for. */
  dynamic: boolean;
  /** The repo's origin URL, for the task slot binding's `owner/name`. */
  repoOriginUrl?: string | null;
  startedAt: number;
  status: "creating" | "error";
  error?: string;
};

/** How much of the goal `goalToBranch` slugs into the branch name — long
 * enough to stay recognizable, short enough that the branch name doesn't
 * become a second copy of the whole goal. */
export const BRANCH_SLUG_SOURCE_CHARS = 50;

/** Per-repo "assigned to me" vs "all open issues" scope for the issue
 * picker below, persisted per `repo.key` rather than one global toggle — a
 * repo where you triage everything and a repo where only your own issues
 * are relevant want different defaults, and both should stick across opens.
 * Defaults to "all" when nothing's stored yet: unlike the Kanban board's
 * import dialog, a new slot is just as often started from someone else's
 * open issue as from one of your own. */
function issueScopeKey(repoKey: string): string {
  return `tt-new-slot-issue-mine:${repoKey}`;
}

function loadIssueScopeMine(repoKey: string): boolean {
  return localStorage.getItem(issueScopeKey(repoKey)) === "true";
}

function saveIssueScopeMine(repoKey: string, mine: boolean): void {
  localStorage.setItem(issueScopeKey(repoKey), String(mine));
}

/** Goal → branch name: the first `BRANCH_SLUG_SOURCE_CHARS` of the goal,
 * slugged, under a `feat/` prefix. The branch field stays editable — this is
 * just the default. */
export function goalToBranch(goal: string): string {
  const slug = slugify(goal.slice(0, BRANCH_SLUG_SOURCE_CHARS));
  return slug ? `feat/${slug}` : "";
}

/** Issue → branch name: `feat/<number>-<slug>`, keeping this form's own
 * `feat/` prefix (not tt-git's `feature/<number>-<slug>`, which is Cockpit's
 * issue-branch convention on an already-existing checkout, not a new slot)
 * so a picked issue produces the same shape of branch name as a typed goal. */
export function branchFromIssue(number: number, title: string): string {
  const slug = slugify(title.slice(0, BRANCH_SLUG_SOURCE_CHARS));
  return slug ? `feat/${number}-${slug}` : `feat/${number}`;
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
  onSubmit: (input: NewTaskSubmit) => void;
}) {
  const [goal, setGoal] = useState("");
  const [images, setImages] = useState<PastedImage[]>([]);
  // Attached images are written to disk as soon as they're pasted, not at
  // submit: "Suggest name + goal" needs real paths to hand `claude -p` (a
  // screenshot is often the entire brief), and staging once means create and
  // suggest reference the same files instead of writing two copies.
  const [imagePaths, setImagePaths] = useState<string[]>([]);
  const [staging, setStaging] = useState(false);
  // Stable per-form staging directory. The branch can't key it — it's still
  // being edited while images are pasted.
  const [draftScope] = useState(nextDraftScopeId);
  const [branchEdit, setBranchEdit] = useState<string | null>(null);
  const [base, setBase] = useState("");
  // Both start unset — the launched `claude` gets no --model/--effort unless
  // the user explicitly picks one, so their own defaults apply.
  const [model, setModel] = useState<ModelChoice>(USE_DEFAULT);
  const [effort, setEffort] = useState<EffortChoice>(USE_DEFAULT);
  // Off by default: a dynamic task merges its own PR, which is a bigger
  // grant than "start Claude on a goal" — opting in is per-task, never
  // remembered, so it's always a deliberate choice.
  const [dynamic, setDynamic] = useState(false);
  const [branches, setBranches] = useState<BaseBranch[]>([]);
  const [baseOpen, setBaseOpen] = useState(false);
  // One slot for whatever the form has to say — an error (nothing happened)
  // or a note (something happened, with a caveat). Modeled as one piece of
  // state because they are mutually exclusive on screen; two would mean every
  // `showError` also had to remember to clear the other one.
  const [notice, setNotice] = useState<{ text: string; kind: "error" | "note" } | null>(null);
  const showError = (text: string) => setNotice({ text, kind: "error" });
  const [branchCheck, setBranchCheck] = useState<BranchCheck | null>(null);
  const [suggesting, setSuggesting] = useState(false);
  // What the goal/branch fields held right before the last accepted
  // suggestion or picked issue overwrote them — lets "Undo" put them back
  // exactly.
  const [preOverwrite, setPreOverwrite] = useState<{
    goal: string;
    branchEdit: string | null;
  } | null>(null);
  const [issuePickerOpen, setIssuePickerOpen] = useState(false);
  // Lazy-initialized once from this repo's stored preference: the form
  // remounts fresh per open (see the base-branches effect below), so there's
  // no prop-change case to keep in sync with.
  const [issueAssignedToMe, setIssueAssignedToMeState] = useState(() =>
    loadIssueScopeMine(repo.key),
  );
  const [issues, setIssues] = useState<IssueItem[] | null>(null);
  const [issuesError, setIssuesError] = useState<string | null>(null);
  // Issues to attach to the created task — multi-select (#339); the first
  // pick also seeds the goal/branch fields.
  const [selectedIssues, setSelectedIssues] = useState<IssueItem[]>([]);

  const sortedBranches = [...branches].toSorted((a, b) => a.name.localeCompare(b.name));
  // What the closed combobox shows: the selected branch's honest label (e.g.
  // `origin/main` when that's what creation will branch from), falling back
  // to the raw value before the branch list has loaded.
  const baseLabel = branches.find((b) => b.name === base)?.label ?? (base || "main");

  const branch = branchEdit ?? goalToBranch(goal);

  useEffect(() => {
    // Guarded like the branchCheck effect below: the caller unmounts this
    // form on cancel/submit rather than reusing one instance across opens,
    // so this mainly guards a fast close-then-reopen of the same repo's form
    // against a stale fetch's `.then` landing after a fresh one already has.
    let cancelled = false;
    void invoke<BaseBranch[]>(
      "slot_base_branches",
      { root: repo.dir },
      { schema: BaseBranchesSchema },
    ).then((result) => {
      if (cancelled) return;
      result.match({
        ok: (list) => {
          setBranches(list);
          setBase(list[0]?.name ?? "main");
        },
        err: (e) => showError(e.message),
      });
    });
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
      void invoke<BranchCheck>("slot_check_branch", { root: repo.dir, branch }).then((check) => {
        if (!cancelled) setBranchCheck(check.unwrapOr(null));
      });
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
    branchCheck?.error ??
    (branchCheck?.taken ? `a slot named "${branchCheck.name}" already exists` : null);

  // Manual only — never runs on a timer or keystroke. Asks claude -p (cwd =
  // the repo, so it has real repo context) to propose a better branch name
  // and a cleaned-up goal, then fills both editable fields directly. The
  // fields stay editable (or "Undo" puts back exactly what was there) —
  // that's the confirmation step, not a separate accept/reject panel.
  async function suggest() {
    // An attached screenshot is a complete brief on its own ("make it look
    // like this"), so images alone are enough to ask — not just typed text.
    if (suggesting || (!goal.trim() && !imagePaths.length)) return;
    setSuggesting(true);
    setNotice(null);
    const suggestion = await invoke<SlotSuggestion>("slot_suggest", {
      dir: repo.dir,
      goal,
      imagePaths,
    });
    suggestion.match({
      ok: (s) => {
        setPreOverwrite({ goal, branchEdit });
        setGoal(s.goal);
        setBranchEdit(s.branch);
        if (s.fallback) {
          setNotice({ text: `Filled in without claude — ${s.fallback}`, kind: "note" });
        }
      },
      err: (e) => showError(e.message),
    });
    setSuggesting(false);
  }

  function undoOverwrite() {
    if (!preOverwrite) return;
    setGoal(preOverwrite.goal);
    setBranchEdit(preOverwrite.branchEdit);
    setPreOverwrite(null);
    setNotice(null);
  }

  function setIssueAssignedToMe(mine: boolean) {
    setIssueAssignedToMeState(mine);
    saveIssueScopeMine(repo.key, mine);
  }

  // Issue list follows the repo this form is open for and the assignee
  // toggle, and only loads once the picker is opened — a slot is created far
  // more often by typing a goal than by picking an issue, so there's no
  // reason to shell `gh` on every form mount.
  useEffect(() => {
    if (!issuePickerOpen) return;
    let cancelled = false;
    setIssues(null);
    setIssuesError(null);
    void storeGhIssuesList(repo.dir, issueAssignedToMe).then((result) => {
      if (cancelled) return;
      result.match({ ok: setIssues, err: (e) => setIssuesError(e.message) });
    });
    return () => {
      cancelled = true;
    };
  }, [issuePickerOpen, issueAssignedToMe, repo.dir]);

  // Toggle an issue in/out of the selection (multi-select, #339 — every
  // selected issue becomes a link on the created task). The *first* pick
  // additionally seeds goal + branch, no confirmation step — same "just
  // overwrite, Undo is the confirmation" shape as `suggest()` above; later
  // picks only attach, so an edited goal is never clobbered. The title (plus
  // the number, for traceability and so Claude can `gh issue view` it for
  // the rest) is all there is to seed with: the issue-list fetch this form
  // uses doesn't carry the issue body. The popover stays open for more picks.
  function toggleIssue(issue: IssueItem) {
    const already = selectedIssues.some((i) => i.repo === issue.repo && i.number === issue.number);
    if (already) {
      setSelectedIssues((prev) =>
        prev.filter((i) => !(i.repo === issue.repo && i.number === issue.number)),
      );
      return;
    }
    if (selectedIssues.length === 0) {
      setPreOverwrite({ goal, branchEdit });
      setGoal(`${issue.title} (#${issue.number})`);
      setBranchEdit(branchFromIssue(issue.number, issue.title));
    }
    setSelectedIssues((prev) => [...prev, issue]);
  }

  // Screenshots are how a lot of goals actually get described ("make it look
  // like this", "this is the error"), so the goal field takes an image paste
  // directly. The bytes are held here until submit, then staged as files
  // outside the repo (`tt_slots::pasted`) whose paths go into Claude's
  // opening prompt.
  // Two paths can attach the same image (the DOM paste event on platforms
  // that populate it, and the host-clipboard read below), so adding is
  // idempotent on the bytes — identical content is the double-path, not a
  // user asking for two copies of one screenshot.
  async function addImages(incoming: PastedImage[]) {
    if (!incoming.length) return;
    const seen = new Set(images.map((i) => i.dataBase64));
    const fresh = incoming.filter((i) => !seen.has(i.dataBase64));
    if (!fresh.length) return;
    const next = [...images, ...fresh];
    setImages(next);
    setNotice(null);
    await stageImages(next);
  }

  /** Write `list` to the staging dir and remember the paths. Failing here is
   * worth surfacing immediately — the image is visibly attached, so silently
   * having no file behind it would only show up later as a prompt pointing at
   * nothing. */
  async function stageImages(list: PastedImage[]) {
    if (!list.length) {
      setImagePaths([]);
      return;
    }
    setStaging(true);
    const staged = await invoke<string[]>(
      "slot_write_pasted_images",
      {
        repo: repo.name,
        branch: draftScope,
        images: list.map(({ mime, dataBase64 }) => ({ mime, dataBase64 })),
      },
      { schema: PastedImagePathsSchema },
    );
    staged.match({
      ok: setImagePaths,
      err: (e) => {
        setImages([]);
        setImagePaths([]);
        showError(`Couldn't attach that image: ${e.message}`);
      },
    });
    setStaging(false);
  }

  async function pasteImages(data: DataTransfer | null) {
    try {
      await addImages(await imagesFromDataTransfer(data));
    } catch (e) {
      showError(errorMessage(e));
    }
  }

  // Read the image off the OS clipboard through Rust.
  //
  // This is the *primary* path, not a fallback, because the DOM can't be
  // relied on here: WebKitGTK delivers an image paste with empty
  // `clipboardData`, and a Ctrl+V there may not fire a `paste` event at all.
  // Hanging image paste off that event would make the feature work on some
  // platforms and silently do nothing on this one. `keydown` always fires,
  // and the host clipboard is the same source the user actually copied to.
  async function pasteFromHostClipboard(): Promise<boolean> {
    const image = await clipboardImageFromHost();
    if (!image) return false;
    await addImages([image]);
    return true;
  }

  function removeImage(id: string) {
    const next = images.filter((img) => img.id !== id);
    setImages(next);
    // Restage so the staged set matches what's shown — otherwise a removed
    // image would still be on disk and still land in the prompt.
    void stageImages(next);
  }

  function submit(worktree = true) {
    if (worktree) {
      if (!branch) {
        showError("Give a goal (or type a branch name) first.");
        return;
      }
      if (branchProblem) {
        showError(branchProblem);
        return;
      }
    } else if (!goal.trim() && selectedIssues.length === 0) {
      // A task-only create still needs *something* to become the card.
      showError("Give a goal (or pick an issue) first.");
      return;
    }
    const isDynamic = worktree && dynamic;
    uiAction(
      worktree ? (isDynamic ? "task.start_dynamic" : "task.start") : "task.create_only",
      "agentboard",
    );
    onSubmit({
      goal: goal.trim(),
      branch,
      base,
      options: {
        model: model === USE_DEFAULT ? undefined : model,
        effort: effort === USE_DEFAULT ? undefined : effort,
      },
      imagePaths,
      issues: selectedIssues,
      worktree,
      dynamic: isDynamic,
    });
  }

  return (
    <div className="mx-3 my-1.5 flex flex-col gap-2 rounded-lg border border-border bg-card p-2.5">
      <span className="text-[11px] font-medium text-muted-foreground">
        ✦ New task — {repo.name}
      </span>
      <Textarea
        autoFocus
        value={goal}
        onChange={(e) => setGoal(e.target.value)}
        onPaste={(e) => {
          const items = Array.from(e.clipboardData?.items ?? []);
          const pastedImages = items.filter(
            (it) => it.kind === "file" && it.type.startsWith("image/"),
          );
          if (pastedImages.length) {
            e.preventDefault();
            // An image type we can't write (SVG, say) would otherwise vanish
            // silently — the paste is already swallowed by the preventDefault
            // above, so say why rather than looking like nothing happened.
            if (!pastedImages.some((it) => isPasteableImage(it.type))) {
              showError(`Can't attach ${pastedImages[0].type} — paste a PNG, JPEG, GIF, or WebP.`);
              return;
            }
            void pasteImages(e.clipboardData);
            return;
          }
          // Text paste: leave it to the textarea. Checked via getData rather
          // than `items` because that's the accessor WebKitGTK actually
          // populates (same reason terminal-view.tsx uses it).
          if (e.clipboardData?.getData("text")) return;
          // Nothing in the event at all. On WebKitGTK that's exactly what an
          // image paste looks like (Ctrl+V of a screenshot fires `paste` with
          // empty clipboardData), so ask the OS clipboard via Rust before
          // concluding there's nothing to attach.
          e.preventDefault();
          void pasteFromHostClipboard();
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
          // Ctrl/Cmd+V: check the OS clipboard for an image. Deliberately
          // does NOT preventDefault — a text paste must still land in the
          // textarea natively, and a clipboard holding an image has no text
          // to insert anyway, so both cases do the right thing.
          if (e.key.toLowerCase() === "v" && (e.metaKey || e.ctrlKey)) {
            void pasteFromHostClipboard();
          }
        }}
        placeholder="what should this task get done? (paste a screenshot to attach it)"
        rows={2}
        className="text-xs"
      />
      {selectedIssues.length > 0 && (
        <div className="flex flex-wrap gap-1">
          {selectedIssues.map((issue) => (
            <span
              key={`${issue.repo}#${issue.number}`}
              title={issue.title}
              className="flex items-center gap-1 rounded border border-border bg-background px-1.5 py-0.5 font-mono text-[10.5px] text-muted-foreground"
            >
              #{issue.number}
              <button
                type="button"
                aria-label={`Detach issue #${issue.number}`}
                onClick={() => toggleIssue(issue)}
                className="text-muted-foreground hover:text-foreground"
              >
                <X className="size-2.5" />
              </button>
            </span>
          ))}
        </div>
      )}
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
        <Button
          variant="outline"
          size="sm"
          className="mr-auto h-6 gap-1 px-1.5 text-[10.5px]"
          title="Attach the image currently on your clipboard"
          onClick={() => {
            void pasteFromHostClipboard().then((found) => {
              if (!found) showError("No image on the clipboard — copy one first.");
            });
          }}
        >
          <ImagePlus className="size-3" />
          Attach image
        </Button>
        <Popover open={issuePickerOpen} onOpenChange={setIssuePickerOpen}>
          <PopoverTrigger asChild>
            <Button variant="outline" size="sm" className="h-6 gap-1 px-1.5 text-[10.5px]">
              <CircleDot className="size-3" />
              Pick issue
            </Button>
          </PopoverTrigger>
          <PopoverContent className="w-80 p-0" align="start">
            <div className="flex items-center justify-between gap-2 border-b border-border px-2 py-1.5">
              <span className="text-[10.5px] text-muted-foreground">
                GitHub issues — {repo.name}
              </span>
              <button
                type="button"
                onClick={() => setIssueAssignedToMe(!issueAssignedToMe)}
                className="text-[10.5px] font-medium text-primary hover:underline"
              >
                {issueAssignedToMe ? "Show all open issues" : "Show only mine"}
              </button>
            </div>
            {issuesError ? (
              <p className="p-3 text-[11px] text-red-500">{issuesError}</p>
            ) : issues === null ? (
              <p className="p-3 text-[11px] text-muted-foreground">Loading issues…</p>
            ) : (
              <Command>
                <CommandInput placeholder="Search issues…" className="text-xs" />
                <CommandList className="max-h-64">
                  <CommandEmpty>No open issues.</CommandEmpty>
                  {issues.map((issue) => {
                    const selected = selectedIssues.some(
                      (i) => i.repo === issue.repo && i.number === issue.number,
                    );
                    return (
                      <CommandItem
                        key={issue.number}
                        value={`${issue.number} ${issue.title}`}
                        onSelect={() => toggleIssue(issue)}
                        className="flex items-start gap-2"
                      >
                        <Check className={cn("mt-0.5 size-3 shrink-0", !selected && "invisible")} />
                        <span className="flex min-w-0 flex-col gap-0.5">
                          <span className="w-full truncate text-xs">{issue.title}</span>
                          <span className="text-[10.5px] text-muted-foreground">
                            #{issue.number}
                            {issue.labels.length > 0
                              ? ` · ${issue.labels.slice(0, 2).join(", ")}`
                              : ""}
                          </span>
                        </span>
                      </CommandItem>
                    );
                  })}
                </CommandList>
              </Command>
            )}
          </PopoverContent>
        </Popover>
        {preOverwrite && (
          <Button
            variant="ghost"
            size="sm"
            className="h-6 gap-1 px-1.5 text-[10.5px]"
            onClick={undoOverwrite}
          >
            <Undo2 className="size-3" />
            Undo
          </Button>
        )}
        <Button
          variant="outline"
          size="sm"
          className="h-6 gap-1 px-1.5 text-[10.5px]"
          disabled={suggesting || staging || (!goal.trim() && !imagePaths.length)}
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
              <span className="truncate">{baseLabel}</span>
            </Button>
          </PopoverTrigger>
          <PopoverContent className="w-(--radix-popover-trigger-width) p-0">
            <Command>
              <CommandInput placeholder="Search branches…" />
              <CommandList>
                <CommandEmpty>No branches found.</CommandEmpty>
                {sortedBranches.map((b) => (
                  <CommandItem
                    key={b.name}
                    value={b.label}
                    className="min-w-0 truncate font-mono text-xs"
                    onSelect={() => {
                      setBase(b.name);
                      setBaseOpen(false);
                    }}
                  >
                    <span className="truncate">{b.label}</span>
                  </CommandItem>
                ))}
              </CommandList>
            </Command>
          </PopoverContent>
        </Popover>
      </div>
      <div className="flex items-center gap-2">
        <Select value={model} onValueChange={(v) => setModel(v as ModelChoice)}>
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
        <Select value={effort} onValueChange={(v) => setEffort(v as EffortChoice)}>
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
      <label
        htmlFor="new-slot-dynamic"
        className="flex cursor-pointer items-start gap-2"
        title="Launches Claude in plan mode. Once you approve its plan in the terminal, it implements the work, runs /code-review low --fix and /simplify, rebases on the base branch, opens the PR, and merges it — the board task closes when the merge lands."
      >
        <Checkbox
          id="new-slot-dynamic"
          checked={dynamic}
          onCheckedChange={(v) => setDynamic(v === true)}
          className="mt-0.5"
        />
        <span className="text-[11px] leading-snug text-muted-foreground">
          Dynamic — after you approve the plan: review, simplify, rebase, PR, merge
        </span>
      </label>
      {notice && (
        <p
          className={cn(
            "text-[11px]",
            notice.kind === "error" ? "text-red-500" : "text-muted-foreground",
          )}
        >
          {notice.text}
        </p>
      )}
      <div className="flex items-center justify-end gap-2">
        <Button variant="ghost" size="sm" onClick={cancel}>
          Cancel
        </Button>
        <Button
          variant="outline"
          size="sm"
          title="Create the board task without a worktree — attach a slot later by starting it again"
          disabled={!goal.trim() && selectedIssues.length === 0}
          onClick={() => submit(false)}
        >
          Task only
        </Button>
        <Button size="sm" disabled={!branch} onClick={() => submit(true)}>
          Start task
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
}: {
  pending: PendingSlot;
  now: number;
  onRetry: (id: string) => void;
  onDismiss: (id: string) => void;
}) {
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
        {pending.imagePaths.length > 0 && (
          <span
            title={`${pending.imagePaths.length} pasted image${pending.imagePaths.length === 1 ? "" : "s"} — attached to this slot's first prompt, and kept for a retry`}
            className="flex shrink-0 items-center gap-0.5 font-mono text-[10.5px] text-muted-foreground/70"
          >
            <Paperclip className="size-2.5" />
            {pending.imagePaths.length}
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
          <Button
            size="sm"
            variant="outline"
            className="h-5 gap-1 px-1.5 text-[10.5px]"
            onClick={() => onRetry(pending.id)}
          >
            Retry
          </Button>
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
