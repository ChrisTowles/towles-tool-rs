import { CircleAlert, GitPullRequest, Inbox } from "lucide-react";
import { toast } from "sonner";
import { ScrollArea } from "@/components/ui/scroll-area";
import { isItemDismissed, storeItemDismiss, useStoreSnapshot, type PrItem } from "@/lib/data";
import { NotInTauri } from "@/lib/errors";
import { useFocusTarget } from "@/lib/focus-target";
import { useNow } from "@/lib/now";
import { uiAction } from "@/lib/ui-action";
import {
  CollectorFreshness,
  DismissButton,
  Empty,
  Panel,
  PrRow,
  prNeedsYou,
  prRank,
} from "@/components/store-bits";

/**
 * Pull requests — the cross-repo PR workbench over the store snapshot (the
 * `prs` collector fills it via `gh`). Two tiers: the PRs that demand action
 * (failing checks, review requested), then everything else grouped by repo.
 * Read-only; rows open the PR in the browser.
 */
export function GhPrsScreen() {
  const { snapshot, live } = useStoreSnapshot();
  const now = useNow();
  // Deep-link focus: a "needs you" popover row scrolls its PR into view here.
  const focusRef = useFocusTarget<HTMLDivElement>("gh-prs");

  // Merged PRs live in the snapshot too (briefly, so a folder's rail chip can
  // turn purple), but this screen is the open work queue — exclude them. A
  // dismissed PR stays hidden until it changes again.
  const openPrs = snapshot.prs.filter((p) => p.state === "open" && !isItemDismissed(p));
  const needsYou = openPrs
    .filter(prNeedsYou)
    .toSorted((a, b) => prRank(b) - prRank(a) || b.updatedTs - a.updatedTs);
  const rest = openPrs
    .filter((p) => !prNeedsYou(p))
    .toSorted((a, b) => a.repo.localeCompare(b.repo) || b.updatedTs - a.updatedTs);
  const byRepo = groupByRepo(rest);
  const prsRun = snapshot.runs.find((r) => r.collector === "prs");

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex shrink-0 flex-wrap items-center gap-x-4 gap-y-1 border-b px-5 py-3">
        <h2 className="flex items-center gap-2 font-heading text-lg font-semibold">
          <GitPullRequest className="size-5 text-muted-foreground" />
          Pull requests
        </h2>
        <span className="font-mono text-xs text-muted-foreground">
          {openPrs.length} open · {needsYou.length} need you
        </span>
        <div className="ml-auto">
          <CollectorFreshness run={prsRun} now={now} />
        </div>
      </div>

      {!live && (
        <div className="flex shrink-0 items-center gap-2 border-b bg-amber-500/10 px-5 py-1.5 text-xs text-amber-700 dark:text-amber-400">
          <CircleAlert className="size-3.5 shrink-0" />
          Not connected to the store — open this window in the Towles Tool app to see live PRs.
        </div>
      )}

      <ScrollArea className="min-h-0 flex-1">
        <div ref={focusRef} className="flex flex-col gap-4 p-4">
          <Panel
            title="Needs you"
            note={`${needsYou.length}`}
            icon={<CircleAlert className="size-4 text-amber-500" />}
          >
            {needsYou.length === 0 ? (
              <Empty>
                {live ? "Nothing needs your attention. Get in the zone." : "Not connected yet."}
              </Empty>
            ) : (
              needsYou.map((pr) => (
                <div key={`${pr.repo}#${pr.number}`} className="border-l-2 border-l-amber-500">
                  <PrRow pr={pr} now={now} actions={<PrDismissButton pr={pr} />} />
                </div>
              ))
            )}
          </Panel>

          <Panel
            title="Open"
            note={`${rest.length}`}
            icon={<Inbox className="size-4 text-muted-foreground" />}
          >
            {rest.length === 0 ? (
              <Empty>{live ? "No other open PRs." : "Not connected yet."}</Empty>
            ) : (
              byRepo.map(([repo, prs]) => (
                <div key={repo} className="flex flex-col divide-y">
                  <div className="bg-muted/30 px-3 py-1 font-mono text-[11px] text-muted-foreground">
                    {repo}
                  </div>
                  {prs.map((pr) => (
                    <PrRow
                      key={`${pr.repo}#${pr.number}`}
                      pr={pr}
                      now={now}
                      actions={<PrDismissButton pr={pr} />}
                    />
                  ))}
                </div>
              ))
            )}
          </Panel>
        </div>
      </ScrollArea>
    </div>
  );
}

/** Dismiss `pr`: it drops out of this screen (and Cockpit, and the day-bar
 * feed) until it changes again. The snapshot re-emits from Rust on success. */
function PrDismissButton({ pr }: { pr: PrItem }) {
  return (
    <DismissButton
      label="Dismiss"
      onDismiss={() => {
        uiAction("gh_prs.pr_dismiss", "gh-prs");
        void storeItemDismiss("pr", pr.repo, pr.number, pr.updatedTs).then((result) => {
          if (result.isErr() && !NotInTauri.is(result.error)) toast.error(result.error.message);
        });
      }}
    />
  );
}

/** Group PRs by repo, preserving the incoming (repo-sorted) order. */
function groupByRepo(prs: PrItem[]): [string, PrItem[]][] {
  const groups = new Map<string, PrItem[]>();
  for (const pr of prs) {
    const list = groups.get(pr.repo);
    if (list) {
      list.push(pr);
    } else {
      groups.set(pr.repo, [pr]);
    }
  }
  return [...groups.entries()];
}
