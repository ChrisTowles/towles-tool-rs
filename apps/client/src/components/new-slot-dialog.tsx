// New-slot modal: give a goal, pick the base branch, and a branch-named
// worktree slot is created under the repo root's slots/ dir (`slot_create` →
// tt-slots ops, shared with `ttr slot new`). The goal slugs the branch name
// (editable) and the caller launches Claude with it in the new slot's first
// session.
import { useEffect, useState } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
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

/** Goal → branch name, mirroring tt-git's slug rules (lowercase, spaces and
 * non `[0-9a-z_-]` to `-`, collapse runs, strip trailing) under a `feat/`
 * prefix. The branch field stays editable — this is just the default. */
export function goalToBranch(goal: string): string {
  let slug = goal.toLowerCase().trim().replaceAll(" ", "-");
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
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const branch = branchEdit ?? goalToBranch(goal);

  useEffect(() => {
    if (!repo) return;
    setGoal("");
    setBranchEdit(null);
    setError(null);
    setBusy(false);
    invokeOrThrow<string[]>("slot_base_branches", { root: repo.dir })
      .then((list) => {
        setBranches(list);
        setBase(list[0] ?? "main");
      })
      .catch((e) => setError(String(e)));
  }, [repo]);

  async function create() {
    if (!repo || busy) return;
    if (!branch) {
      setError("Give a goal (or type a branch name) first.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const created = await invokeOrThrow<SlotCreated>("slot_create", {
        root: repo.dir,
        branch,
        base,
      });
      for (const warning of created.warnings) toast(warning);
      onClose();
      await onCreated(created, goal.trim());
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  }

  return (
    <Dialog open={repo != null} onOpenChange={(open) => !open && !busy && onClose()}>
      <DialogContent showCloseButton={false}>
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
        <div className="flex items-center gap-2">
          <span className="w-14 shrink-0 text-[11px] text-muted-foreground">branch</span>
          <Input
            value={branch}
            onChange={(e) => setBranchEdit(e.target.value)}
            placeholder="feat/…"
            className="font-mono text-xs"
          />
        </div>
        <div className="flex items-center gap-2">
          <span className="w-14 shrink-0 text-[11px] text-muted-foreground">base</span>
          <Select value={base} onValueChange={setBase}>
            <SelectTrigger className="w-full font-mono text-xs">
              <SelectValue placeholder="main" />
            </SelectTrigger>
            <SelectContent>
              {branches.map((b) => (
                <SelectItem key={b} value={b} className="font-mono text-xs">
                  {b}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        {error && <p className="text-xs text-red-500">{error}</p>}
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
