import { useState } from "react";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Button } from "@/components/ui/button";
import { useTheme, type Theme } from "@/components/theme-provider";
import { closeCurrentWindow } from "@/lib/open-settings";

function SettingRow({
  label,
  description,
  children,
}: {
  label: string;
  description: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-center justify-between gap-4">
      <div>
        <div className="text-sm font-medium">{label}</div>
        <div className="text-sm text-muted-foreground">{description}</div>
      </div>
      {children}
    </div>
  );
}

export function SettingsWindow() {
  const { theme, setTheme } = useTheme();
  // Local-only until settings read/write goes through a Tauri command.
  const [openJournalOnLaunch, setOpenJournalOnLaunch] = useState(true);
  const [showDoctorStatus, setShowDoctorStatus] = useState(true);

  return (
    <div className="flex h-screen flex-col bg-background text-foreground">
      <header className="flex items-center border-b border-border bg-card px-4 py-3">
        <h1 className="font-heading text-sm font-semibold">Settings</h1>
      </header>

      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="flex flex-col gap-5 p-4">
          <p className="text-sm text-muted-foreground">
            Theme applies immediately. Other settings are placeholders until they persist to
            towles-tool.settings.json.
          </p>

          <SettingRow label="Theme" description="Light, dark, or follow the system.">
            <Select value={theme} onValueChange={(v) => setTheme(v as Theme)}>
              <SelectTrigger className="w-32">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="light">Light</SelectItem>
                <SelectItem value="dark">Dark</SelectItem>
                <SelectItem value="system">System</SelectItem>
              </SelectContent>
            </Select>
          </SettingRow>

          <SettingRow
            label="Open journal on launch"
            description="Show today's note when the app starts."
          >
            <Switch checked={openJournalOnLaunch} onCheckedChange={setOpenJournalOnLaunch} />
          </SettingRow>

          <SettingRow
            label="Doctor status in status bar"
            description="Show check results at the bottom of the window."
          >
            <Switch checked={showDoctorStatus} onCheckedChange={setShowDoctorStatus} />
          </SettingRow>
        </div>
      </div>

      <footer className="flex justify-end border-t border-border bg-card px-4 py-3">
        <Button variant="outline" size="sm" onClick={() => void closeCurrentWindow()}>
          Done
        </Button>
      </footer>
    </div>
  );
}
