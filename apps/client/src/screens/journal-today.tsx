import { useEffect, useState } from "react";
import { ExternalLink, Pencil, RefreshCw } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { journalLog } from "@/lib/data";
import { NotInTauri, type IpcError } from "@/lib/errors";
import {
  JOURNAL_STALE_ERROR,
  journalGetToday,
  journalOpen,
  journalSave,
  type TodayNote,
} from "@/lib/journal";

/** Surface a failed journal command. Silent outside the Tauri shell, where the
 * screen already renders its "not available" state. */
function reportJournalError(error: IpcError) {
  if (!NotInTauri.is(error)) toast.error(error.message);
}

/** Hand an entry to the user's preferred editor, surfacing a failure (an
 * unconfigured or missing editor is the common one). */
async function openInEditor(relativePath: string) {
  const opened = await journalOpen(relativePath);
  if (opened.isErr()) reportJournalError(opened.error);
}

/** Today — today's daily note, read from, appended to, and edited in-app. */
export function JournalTodayScreen() {
  const [note, setNote] = useState<TodayNote | null>(null);
  const [loading, setLoading] = useState(true);
  const [draft, setDraft] = useState("");

  // Full-note edit mode.
  const [editing, setEditing] = useState(false);
  const [editDraft, setEditDraft] = useState("");
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [staleConflict, setStaleConflict] = useState(false);

  const dirty = editing && note !== null && editDraft !== note.content;

  async function refresh() {
    setLoading(true);
    (await journalGetToday()).match({
      ok: setNote,
      err: (e) => {
        setNote(null);
        reportJournalError(e);
      },
    });
    setLoading(false);
  }

  useEffect(() => {
    void refresh();
  }, []);

  async function submitLog() {
    const line = draft.trim();
    if (!line) return;
    setDraft("");
    (await journalLog(line)).match({
      ok: () => void refresh(),
      err: (e) => (NotInTauri.is(e) ? toast.info("not wired in browser") : toast.error(e.message)),
    });
  }

  function startEditing() {
    if (!note) return;
    setEditDraft(note.content);
    setSaveError(null);
    setStaleConflict(false);
    setEditing(true);
  }

  function cancelEditing() {
    setEditing(false);
    setEditDraft("");
    setSaveError(null);
    setStaleConflict(false);
  }

  async function save() {
    if (!note) return;
    setSaving(true);
    setSaveError(null);
    setStaleConflict(false);
    const saved = await journalSave(note.relativePath, note.content, editDraft);
    saved.match({
      ok: () => {
        // Reflect the saved bytes locally, then leave edit mode.
        setNote({ ...note, content: editDraft });
        setEditing(false);
        setEditDraft("");
      },
      err: (e) => {
        if (e.message.includes(JOURNAL_STALE_ERROR)) {
          setStaleConflict(true);
          setSaveError("This note changed on disk since you opened it. Reload to see the latest.");
        } else {
          setSaveError(e.message);
        }
      },
    });
    setSaving(false);
  }

  /** Pull the latest bytes from disk into the editor, discarding local edits. */
  async function reloadLatest() {
    const latest = await journalGetToday();
    if (latest.isErr()) {
      reportJournalError(latest.error);
      return;
    }
    setNote(latest.value);
    setEditDraft(latest.value.content);
    setSaveError(null);
    setStaleConflict(false);
  }

  return (
    <div className="flex flex-col gap-4">
      <div className="flex items-center justify-between gap-2">
        <div>
          <h2 className="font-heading text-lg font-semibold">Today</h2>
          {note && (
            <p className="font-mono text-xs text-muted-foreground">
              {note.relativePath}
              {dirty && <span className="ml-2 text-amber-600 dark:text-amber-400">• unsaved</span>}
            </p>
          )}
        </div>
        <div className="flex gap-2">
          {editing ? (
            <>
              <Button variant="ghost" size="sm" onClick={cancelEditing} disabled={saving}>
                Cancel
              </Button>
              <Button size="sm" onClick={() => void save()} disabled={saving || !dirty}>
                {saving ? "Saving…" : "Save"}
              </Button>
            </>
          ) : (
            <>
              <Button variant="outline" size="sm" onClick={() => void refresh()}>
                <RefreshCw className="size-3.5" />
                Refresh
              </Button>
              <Button variant="outline" size="sm" disabled={!note} onClick={startEditing}>
                <Pencil className="size-3.5" />
                Edit
              </Button>
              <Button
                variant="outline"
                size="sm"
                disabled={!note}
                onClick={() => note && void openInEditor(note.relativePath)}
              >
                <ExternalLink className="size-3.5" />
                Open in editor
              </Button>
            </>
          )}
        </div>
      </div>

      {!editing && (
        <Input
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void submitLog();
          }}
          placeholder="Log to today's note… (⌘J works anywhere)"
        />
      )}

      {saveError && (
        <div className="flex items-center justify-between gap-3 rounded-lg border border-destructive/50 bg-destructive/10 px-3.5 py-2 text-sm text-destructive dark:border-destructive/40">
          <span>{saveError}</span>
          {staleConflict && (
            <Button
              variant="outline"
              size="sm"
              className="shrink-0"
              onClick={() => void reloadLatest()}
            >
              Reload latest
            </Button>
          )}
        </div>
      )}

      {editing ? (
        <Textarea
          value={editDraft}
          onChange={(e) => setEditDraft(e.target.value)}
          disabled={saving}
          spellCheck={false}
          className="min-h-[60vh] font-mono text-xs leading-relaxed"
          aria-invalid={staleConflict}
        />
      ) : null}
      {!editing && (
        <div className="rounded-lg border border-border bg-card p-3.5">
          {loading && !note ? (
            <p className="text-sm text-muted-foreground">Loading…</p>
          ) : note ? (
            <pre className="overflow-x-auto whitespace-pre-wrap font-mono text-xs leading-relaxed text-foreground">
              {note.content}
            </pre>
          ) : (
            <p className="text-sm text-muted-foreground">Not available outside the app.</p>
          )}
        </div>
      )}
    </div>
  );
}
