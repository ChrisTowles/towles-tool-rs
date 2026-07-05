import { NotWired } from "@/components/not-wired";

export function GhPrsScreen() {
  return (
    <NotWired
      title="Pull requests"
      detail="Not wired yet. Real PR data already flows through the store snapshot (`prs` collector) on the Cockpit — build this screen against that, or retire it."
    />
  );
}
