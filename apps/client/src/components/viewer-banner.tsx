/**
 * The editable surfaces' amber "the disk moved underneath you" strip —
 * rendered by CodeViewer (conflict + deleted states) and MonacoMultiDiff
 * (conflict state), absolutely positioned over the editor's top edge.
 * Buttons render only when their handler is given; the deleted notice is the
 * button-less case. One component so the five copies of these classes can't
 * drift apart again.
 */
export function ViewerBanner({
  message,
  onTheirs,
  onMine,
  theirsTitle = "Discard this buffer and load the file as it is on disk",
  mineTitle = "Keep this buffer — overwrite the disk with it now",
}: {
  message: string;
  onTheirs?: () => void;
  onMine?: () => void;
  theirsTitle?: string;
  mineTitle?: string;
}) {
  return (
    <div className="absolute inset-x-0 top-0 z-10 flex items-center gap-2 border-b border-amber-500/40 bg-card px-3 py-1.5">
      <span className="min-w-0 flex-1 truncate text-xs text-amber-600 dark:text-amber-400">
        {message}
      </span>
      {onTheirs && (
        <button
          type="button"
          title={theirsTitle}
          onClick={onTheirs}
          className="shrink-0 rounded-sm px-1.5 py-0.5 font-mono text-[10.5px] text-amber-600 hover:bg-accent dark:text-amber-400"
        >
          load theirs
        </button>
      )}
      {onMine && (
        <button
          type="button"
          title={mineTitle}
          onClick={onMine}
          className="shrink-0 rounded-sm px-1.5 py-0.5 font-mono text-[10.5px] text-amber-600 hover:bg-accent dark:text-amber-400"
        >
          keep mine
        </button>
      )}
    </div>
  );
}
