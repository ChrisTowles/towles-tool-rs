import { useEffect, useState } from "react";
import { toast } from "sonner";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { journalLog, storeAddTask } from "@/lib/data";
import { NotInTauri, type IpcError } from "@/lib/errors";
import { formatLogLine, parseQuickLog } from "@/lib/quick-log-format";
import { useWorkspace } from "@/lib/workspace";

/** Surface a failed capture. Browser dev gets the "not wired" note rather than
 * an error, since nothing is actually broken there. */
function reportCaptureError(error: IpcError) {
  if (NotInTauri.is(error)) toast.info("not wired in browser");
  else toast.error(error.message);
}

/**
 * ⌘J quick log: one line straight into today's journal note, or — with a leading
 * `/todo ` / `/t ` prefix — a new Board todo. Opens on the `quicklog:open` window
 * event (dispatched from the App-level shortcut) so the dialog can live anywhere
 * without threading state through the workspace.
 */
export function QuickLog() {
  const [open, setOpen] = useState(false);
  const [text, setText] = useState("");
  const { activeTab } = useWorkspace();

  useEffect(() => {
    const onOpen = () => setOpen(true);
    window.addEventListener("quicklog:open", onOpen);
    return () => window.removeEventListener("quicklog:open", onOpen);
  }, []);

  const parsed = parseQuickLog(text);
  const routesToTodo = parsed.kind === "todo";

  function submit() {
    if (!parsed.body) return;
    if (routesToTodo) {
      // Same add-task path the Board uses — a plain todo in the backlog column.
      void storeAddTask(parsed.body).then((added) =>
        added.match({
          ok: () => {
            toast.success("Added to Board");
          },
          err: reportCaptureError,
        }),
      );
    } else {
      // Reconstruct a timeline bullet — `- HH:MM [context] text` — stamped with the current
      // screen so scattered captures read back as a log. Matches `tt journal jot`'s format
      // so app and CLI entries interleave in the same daily note.
      const line = formatLogLine(parsed.body, { now: new Date(), context: activeTab });
      void journalLog(line).then((logged) =>
        logged.match({
          ok: () => {
            toast.success("Logged");
          },
          err: reportCaptureError,
        }),
      );
    }
    setText("");
    setOpen(false);
  }

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent showCloseButton={false}>
        <DialogHeader>
          <DialogTitle>Quick log</DialogTitle>
        </DialogHeader>
        <Input
          autoFocus
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") submit();
          }}
          placeholder="Log to today's note… (/todo for the Board)"
        />
        <p className="text-muted-foreground text-xs">
          {routesToTodo ? "→ Board" : "→ today's note"}
        </p>
      </DialogContent>
    </Dialog>
  );
}
