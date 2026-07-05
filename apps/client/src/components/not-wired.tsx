import { Unplug } from "lucide-react";

/**
 * Placeholder for a screen whose feature isn't built against real data yet.
 * The app ships no mock data — screens are wired to real backends one feature
 * at a time, and until then they render this instead of anything fake.
 */
export function NotWired({ title, detail }: { title: string; detail: string }) {
  return (
    <div className="flex flex-col gap-4">
      <h2 className="font-heading text-lg font-semibold">{title}</h2>
      <div className="flex flex-col items-center gap-3 rounded-lg border border-dashed py-16 text-center">
        <Unplug className="size-8 text-muted-foreground" />
        <p className="text-sm font-medium">Not built yet</p>
        <p className="max-w-md text-xs text-muted-foreground">{detail}</p>
      </div>
    </div>
  );
}
