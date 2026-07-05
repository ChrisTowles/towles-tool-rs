import { NotWired } from "@/components/not-wired";

export function ConfigScreen() {
  return (
    <NotWired
      title="Config"
      detail="Not wired yet. Build this against a Tauri command that reads ~/.config/towles-tool/towles-tool.settings.json (tt-config crate) so it shows the real config."
    />
  );
}
