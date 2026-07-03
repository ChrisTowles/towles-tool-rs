import { useState } from "react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { useTheme, type Theme } from "@/components/theme-provider";
import { useWorkspace } from "@/lib/workspace";

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

export function SettingsDialog() {
  const { settingsOpen, setSettingsOpen } = useWorkspace();
  const { theme, setTheme } = useTheme();
  // Local-only until settings read/write goes through a Tauri command.
  const [openJournalOnLaunch, setOpenJournalOnLaunch] = useState(true);
  const [showDoctorStatus, setShowDoctorStatus] = useState(true);

  return (
    <Dialog open={settingsOpen} onOpenChange={setSettingsOpen}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Settings</DialogTitle>
          <DialogDescription>
            Theme applies immediately. Other settings are placeholders until they persist to
            towles-tool.settings.json.
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-5 py-2">
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

        <DialogFooter showCloseButton />
      </DialogContent>
    </Dialog>
  );
}
