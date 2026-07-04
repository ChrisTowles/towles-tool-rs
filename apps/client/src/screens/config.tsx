import { Copy } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { settingsJson, settingsPath } from "@/lib/mock-data";

export function ConfigScreen() {
  const json = JSON.stringify(settingsJson, null, 2);

  const copy = async () => {
    await navigator.clipboard.writeText(json);
    toast.success("Copied settings JSON");
  };

  return (
    <div className="flex flex-col gap-4">
      <div className="flex items-center justify-between">
        <div>
          <h2 className="font-heading text-lg font-semibold">Config</h2>
          <p className="font-mono text-sm text-muted-foreground">{settingsPath}</p>
        </div>
        <Button variant="outline" onClick={copy}>
          <Copy /> Copy JSON
        </Button>
      </div>

      <pre className="overflow-x-auto rounded-lg border bg-muted/30 p-4 font-mono text-xs leading-relaxed">
        {json}
      </pre>

      <p className="text-sm text-muted-foreground">
        This file is shared with the TypeScript CLI — showing mock values until the Tauri command
        that reads it lands. Each collector under <span className="font-mono">collectors</span> has
        its own enable flag and refresh cadence; the calendar collector's{" "}
        <span className="font-mono">provider</span> switches between the Google (home) and Outlook
        (work) prompt so the same app works in both places.
      </p>
    </div>
  );
}
