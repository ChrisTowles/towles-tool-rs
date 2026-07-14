import { useEffect, useState } from "react";
import { Check, ChevronsUpDown } from "lucide-react";
import { toast } from "sonner";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import { cn } from "@/lib/utils";
import {
  storeGhIssuesList,
  storeGhMilestonesList,
  storeGhTrackedRepos,
  storeImportIssues,
  type GhRepoOption,
  type IssueItem,
} from "@/lib/data";

/** One option in a repo/milestone picker combobox. */
type ComboOption = { value: string; label: string };

/** A single-select searchable dropdown shared by the repo and milestone
 * pickers below — same Popover + Command combobox shape as SlackUserPicker
 * (settings-window.tsx). */
function ComboBox({
  value,
  options,
  onSelect,
  placeholder,
  searchPlaceholder,
  disabled,
}: {
  value: string;
  options: ComboOption[];
  onSelect: (value: string) => void;
  placeholder: string;
  searchPlaceholder: string;
  disabled?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const selected = options.find((o) => o.value === value);
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          variant="outline"
          role="combobox"
          aria-expanded={open}
          disabled={disabled}
          className="w-full justify-between font-normal"
        >
          <span className="truncate">{selected?.label ?? placeholder}</span>
          <ChevronsUpDown className="ml-2 size-4 shrink-0 opacity-50" />
        </Button>
      </PopoverTrigger>
      <PopoverContent className="w-[--radix-popover-trigger-width] p-0" align="start">
        <Command>
          <CommandInput placeholder={searchPlaceholder} />
          <CommandList>
            <CommandEmpty>No match.</CommandEmpty>
            <CommandGroup>
              {options.map((o) => (
                <CommandItem
                  key={o.value}
                  value={o.label}
                  onSelect={() => {
                    onSelect(o.value);
                    setOpen(false);
                  }}
                >
                  <Check
                    className={cn("mr-2 size-4", o.value === value ? "opacity-100" : "opacity-0")}
                  />
                  <span className="truncate">{o.label}</span>
                </CommandItem>
              ))}
            </CommandGroup>
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}

/** `owner/repo#123` — the identity a linked todo and an importable issue are
 * matched on. */
function issueKey(repo: string, number: number): string {
  return `${repo}#${number}`;
}

/**
 * "Import from GitHub": pick a tracked repo, optionally scope to "assigned to
 * me" (default) vs. all open issues and a milestone, then multi-select which
 * issues become new Backlog todos. Issues already linked to a todo
 * (`linkedKeys`) are shown disabled. Reuses the same Popover+Command combobox
 * shape as `SlackUserPicker` and the plain Dialog chrome used elsewhere in
 * the app (see `agentboard-rail.tsx`'s note/issue dialogs).
 */
export function ImportIssuesDialog({
  open,
  onOpenChange,
  linkedKeys,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  linkedKeys: Set<string>;
}) {
  const [repos, setRepos] = useState<GhRepoOption[] | null>(null);
  const [reposFailed, setReposFailed] = useState(false);
  const [selectedDir, setSelectedDir] = useState("");
  const [assignedToMe, setAssignedToMe] = useState(true);
  const [milestones, setMilestones] = useState<string[] | null>(null);
  const [selectedMilestone, setSelectedMilestone] = useState("");
  const [issues, setIssues] = useState<IssueItem[] | null>(null);
  const [issuesFailed, setIssuesFailed] = useState(false);
  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  const [importing, setImporting] = useState(false);

  // Reset everything each time the dialog opens, and load the repo list.
  useEffect(() => {
    if (!open) return;
    setRepos(null);
    setReposFailed(false);
    setSelectedDir("");
    setAssignedToMe(true);
    setMilestones(null);
    setSelectedMilestone("");
    setIssues(null);
    setIssuesFailed(false);
    setSelected(new Set());

    let alive = true;
    void storeGhTrackedRepos()
      .then((list) => alive && setRepos(list))
      .catch(() => alive && setReposFailed(true));
    return () => {
      alive = false;
    };
  }, [open]);

  // Milestone list follows the selected repo.
  useEffect(() => {
    if (!selectedDir) {
      setMilestones(null);
      return;
    }
    setMilestones(null);
    setSelectedMilestone("");
    let alive = true;
    void storeGhMilestonesList(selectedDir)
      .then((list) => alive && setMilestones(list))
      .catch(() => alive && setMilestones([]));
    return () => {
      alive = false;
    };
  }, [selectedDir]);

  // Issue list follows repo + assignee toggle + milestone filter.
  useEffect(() => {
    if (!selectedDir) {
      setIssues(null);
      return;
    }
    setIssues(null);
    setIssuesFailed(false);
    let alive = true;
    void storeGhIssuesList(selectedDir, assignedToMe, selectedMilestone || undefined)
      .then((list) => alive && setIssues(list))
      .catch(() => alive && setIssuesFailed(true));
    return () => {
      alive = false;
    };
  }, [selectedDir, assignedToMe, selectedMilestone]);

  async function handleImport() {
    if (!issues) return;
    const items = issues
      .filter((i) => selected.has(issueKey(i.repo, i.number)))
      .map((i) => ({ repo: i.repo, number: i.number, title: i.title, url: i.url }));
    if (items.length === 0) return;
    setImporting(true);
    try {
      const count = await storeImportIssues(items);
      toast.success(`Imported ${count} issue${count === 1 ? "" : "s"}`);
      onOpenChange(false);
    } catch (e) {
      toast.error(String(e));
    } finally {
      setImporting(false);
    }
  }

  const repoOptions: ComboOption[] = (repos ?? []).map((r) => ({ value: r.dir, label: r.name }));
  const milestoneOptions: ComboOption[] = [
    { value: "", label: "All milestones" },
    ...(milestones ?? []).map((m) => ({ value: m, label: m })),
  ];
  const selectedCount = selected.size;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg" showCloseButton={false}>
        <DialogHeader>
          <DialogTitle>Import from GitHub</DialogTitle>
          <DialogDescription>
            Pick a repo and select which open issues to add to Backlog. Their status stays
            synced with GitHub once imported.
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-3">
          <div className="flex items-center gap-2">
            <div className="flex-1">
              {reposFailed ? (
                <div className="text-xs text-destructive">Couldn't load tracked repos.</div>
              ) : repos !== null && repos.length === 0 ? (
                <div className="text-xs text-muted-foreground">
                  No tracked repos — add one in Agentboard first.
                </div>
              ) : (
                <ComboBox
                  value={selectedDir}
                  options={repoOptions}
                  onSelect={setSelectedDir}
                  placeholder={repos === null ? "Loading repos…" : "Select a repo…"}
                  searchPlaceholder="Search repos…"
                  disabled={repos === null}
                />
              )}
            </div>
            <div className="w-44">
              <ComboBox
                value={selectedMilestone}
                options={milestoneOptions}
                onSelect={setSelectedMilestone}
                placeholder="All milestones"
                searchPlaceholder="Search milestones…"
                disabled={!selectedDir}
              />
            </div>
          </div>

          <div className="flex items-center gap-1.5">
            {(
              [
                { value: true, label: "Assigned to me" },
                { value: false, label: "All open issues" },
              ] as const
            ).map(({ value, label }) => (
              <button
                key={label}
                type="button"
                disabled={!selectedDir}
                onClick={() => setAssignedToMe(value)}
                className={cn(
                  "rounded-md border px-2.5 py-1 text-xs font-medium transition-colors disabled:opacity-50",
                  assignedToMe === value
                    ? "border-primary bg-primary text-primary-foreground"
                    : "border-border bg-background text-muted-foreground hover:bg-muted",
                )}
              >
                {label}
              </button>
            ))}
          </div>

          <ScrollArea className="h-72 rounded-md border">
            {!selectedDir ? (
              <div className="flex h-full items-center justify-center p-6 text-xs text-muted-foreground">
                Select a repo to see its issues.
              </div>
            ) : issuesFailed ? (
              <div className="flex h-full items-center justify-center p-6 text-xs text-destructive">
                Couldn't load issues for this repo.
              </div>
            ) : issues === null ? (
              <div className="flex h-full items-center justify-center p-6 text-xs text-muted-foreground">
                Loading issues…
              </div>
            ) : issues.length === 0 ? (
              <div className="flex h-full items-center justify-center p-6 text-xs text-muted-foreground">
                No open issues match this filter.
              </div>
            ) : (
              <div className="flex flex-col gap-0.5 p-1">
                {issues.map((issue) => {
                  const key = issueKey(issue.repo, issue.number);
                  const already = linkedKeys.has(key);
                  return (
                    <label
                      key={key}
                      className={cn(
                        "flex items-start gap-2 rounded-md px-2 py-1.5 text-sm",
                        already ? "opacity-50" : "cursor-pointer hover:bg-muted",
                      )}
                    >
                      <Checkbox
                        className="mt-0.5"
                        checked={already || selected.has(key)}
                        disabled={already}
                        onCheckedChange={(checked) =>
                          setSelected((prev) => {
                            const next = new Set(prev);
                            if (checked) next.add(key);
                            else next.delete(key);
                            return next;
                          })
                        }
                      />
                      <span className="min-w-0 flex-1">
                        <span className="block truncate">{issue.title}</span>
                        <span className="text-xs text-muted-foreground">
                          #{issue.number}
                          {already ? " · already on board" : ""}
                        </span>
                      </span>
                    </label>
                  );
                })}
              </div>
            )}
          </ScrollArea>
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button onClick={() => void handleImport()} disabled={selectedCount === 0 || importing}>
            {importing
              ? "Importing…"
              : `Import${selectedCount > 0 ? ` ${selectedCount}` : ""}`}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
