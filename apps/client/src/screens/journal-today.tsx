import { useEffect, useRef, useState } from "react";
import { ExternalLink, Mic, Pencil, RefreshCw, Square } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { useDictationForElement } from "@/lib/dictation";
import { journalLog } from "@/lib/data";
import {
  JOURNAL_STALE_ERROR,
  journalGetToday,
  journalOpen,
  journalSave,
  type TodayNote,
} from "@/lib/journal";

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
  const editRef = useRef<HTMLTextAreaElement>(null);
  const dictation = useDictationForElement(editRef);

  const dirty = editing && note !== null && editDraft !== note.content;

  async function refresh() {
    setLoading(true);
    setNote(await journalGetToday());
    setLoading(false);
  }

  useEffect(() => {
    void refresh();
  }, []);

  async function submitLog() {
    const line = draft.trim();
    if (!line) return;
    setDraft("");
    if (await journalLog(line)) void refresh();
  }

  function startEditing() {
    if (!note) return;
    setEditDraft(note.content);
    setSaveError(null);
    setStaleConflict(false);
    setEditing(true);
  }

  function cancelEditing() {
    dictation.stop();
    setEditing(false);
    setEditDraft("");
    setSaveError(null);
    setStaleConflict(false);
  }

  async function save() {
    if (!note) return;
    dictation.stop();
    setSaving(true);
    setSaveError(null);
    setStaleConflict(false);
    try {
      await journalSave(note.relativePath, note.content, editDraft);
      // Reflect the saved bytes locally, then leave edit mode.
      setNote({ ...note, content: editDraft });
      setEditing(false);
      setEditDraft("");
    } catch (e) {
      const message = String(e);
      if (message.includes(JOURNAL_STALE_ERROR)) {
        setStaleConflict(true);
        setSaveError("This note changed on disk since you opened it. Reload to see the latest.");
      } else {
        setSaveError(message);
      }
    } finally {
      setSaving(false);
    }
  }

  /** Pull the latest bytes from disk into the editor, discarding local edits. */
  async function reloadLatest() {
    const latest = await journalGetToday();
    if (!latest) return;
    setNote(latest);
    setEditDraft(latest.content);
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
                onClick={() => note && void journalOpen(note.relativePath)}
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
        <div className="relative">
          <Textarea
            ref={editRef}
            value={editDraft}
            onChange={(e) => setEditDraft(e.target.value)}
            disabled={saving}
            spellCheck={false}
            className="min-h-[60vh] pr-9 font-mono text-xs leading-relaxed"
            aria-invalid={staleConflict}
          />
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="absolute top-1 right-1 size-7"
            disabled={saving || dictation.phase === "loadingModel" || dictation.phase === "stopping"}
            title={dictation.recording ? "Stop dictation" : "Dictate into this note"}
            onClick={() => (dictation.recording ? dictation.stop() : dictation.start())}
          >
            {dictation.recording ? (
              <Square className="size-3.5 fill-current text-red-500" />
            ) : (
              <Mic className="size-3.5" />
            )}
          </Button>
        </div>
      ) : null}
      {editing && dictation.error && <p className="text-xs text-red-500">{dictation.error}</p>}
      {editing && dictation.silentCapture && (
        <p className="text-xs text-amber-600 dark:text-amber-400">
          Recording, but hearing nothing — if you&apos;re speaking, check the mic&apos;s input
          volume and device (system default, or Settings → dictation).
        </p>
      )}
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
