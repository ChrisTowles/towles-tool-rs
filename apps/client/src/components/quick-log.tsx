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

/**
 * ⌘J quick log: one line straight into today's journal note. Opens on the
 * `quicklog:open` window event (dispatched from the App-level shortcut) so the
 * dialog can live anywhere without threading state through the workspace.
 */
export function QuickLog() {
  const [open, setOpen] = useState(false);
  const [text, setText] = useState("");

  useEffect(() => {
    const onOpen = () => setOpen(true);
    window.addEventListener("quicklog:open", onOpen);
    return () => window.removeEventListener("quicklog:open", onOpen);
  }, []);

  function submit() {
    const line = text.trim();
    if (!line) return;
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
