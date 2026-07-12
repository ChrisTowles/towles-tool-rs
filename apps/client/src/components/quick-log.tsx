import { useEffect, useState } from "react";
import { toast } from "sonner";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { journalLog } from "@/lib/data";
import { formatLogLine } from "@/lib/quick-log-format";
import { useWorkspace } from "@/lib/workspace";

/**
 * ⌘J quick log: one line straight into today's journal note. Opens on the
 * `quicklog:open` window event (dispatched from the App-level shortcut) so the
 * dialog can live anywhere without threading state through the workspace.
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

  function submit() {
    if (!text.trim()) return;
    // Reconstruct a timeline bullet — `- HH:MM [context] text` — stamped with the current
    // screen so scattered captures read back as a log. Matches `ttr journal jot`'s format
    // so app and CLI entries interleave in the same daily note.
    const line = formatLogLine(text, { now: new Date(), context: activeTab });
    void journalLog(line).then((ok) => {
      if (ok) toast.success("Logged");
    });
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
          placeholder="Log to today's note…"
        />
      </DialogContent>
    </Dialog>
  );
}
