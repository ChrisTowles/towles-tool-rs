import { useState } from "react";
import { toast } from "sonner";
import {
  CircleDot,
  FolderGit2,
  GitPullRequest,
  ListPlus,
  Moon,
  PanelLeft,
  PenLine,
  Settings,
  Sun,
  TerminalSquare,
} from "lucide-react";
import {
  Command,
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandSeparator,
  CommandShortcut,
} from "@/components/ui/command";
import { useTheme } from "@/components/theme-provider";
import { requestAgentboardNav, useAgentboardState } from "@/lib/agentboard";
import { storeAddTask, useStoreSnapshot } from "@/lib/data";
import { openExternalUrl } from "@/lib/open-url";
import {
  paletteRepoEntries,
  paletteSessionEntries,
  palettePrEntries,
  paletteIssueEntries,
  paletteQuickAddEntry,
} from "@/lib/palette";
import { SCREENS, type ScreenId } from "@/lib/screens";
import { shortcutHint } from "@/lib/shortcuts";
import { useWorkspace } from "@/lib/workspace";

/**
 * ⌘K launcher. Beyond the static "go to screen" list and app actions it pulls
 * live sections from the same read-only hooks the screens use: recent
 * screens (MRU), Agentboard checkouts/sessions to jump to, and open PRs and
 * issues to open in the browser. Shortcut hints come from `shortcutHint()` so glyphs are
 * platform-correct (Ctrl on Linux, ⌘ on mac) instead of hardcoded.
 */
export function CommandPalette() {
  const { paletteOpen, setPaletteOpen, recent, activeTab, openTab, openSettingsTab, toggleSidebar } =
    useWorkspace();
  const { theme, setTheme } = useTheme();
  const { repos } = useAgentboardState();
  const { snapshot } = useStoreSnapshot();
  const [query, setQuery] = useState("");

  const resolvedDark =
    theme === "system"
      ? window.matchMedia("(prefers-color-scheme: dark)").matches
      : theme === "dark";

  const run = (action: () => void) => {
    setPaletteOpen(false);
    action();
  };

  // Reveal a checkout/session in Agentboard: switch to the tab, then hand the
  // target off through the read-only nav mailbox (Agentboard may not be mounted
  // yet — the request is stashed for its mount effect).
  const jumpToFolder = (folderDir: string) =>
    run(() => {
      openTab("agentboard");
      requestAgentboardNav({ kind: "folder", folderDir });
    });
  const jumpToSession = (folderDir: string, sessionId: string) =>
    run(() => {
      openTab("agentboard");
      requestAgentboardNav({ kind: "session", folderDir, sessionId });
    });

  // MRU minus the screen you're already looking at — capped so the section
  // stays a shortcut, not a second full screen list.
  const recentScreens = recent
    .filter((id): id is ScreenId => id !== activeTab && id in SCREENS)
    .slice(0, 4);

  const repoEntries = paletteRepoEntries(repos);
  const sessionEntries = paletteSessionEntries(repos);
  const prEntries = palettePrEntries(snapshot.prs);
  const issueEntries = paletteIssueEntries(snapshot.issues);
  const quickAdd = paletteQuickAddEntry(query);

  const createTodo = (title: string) =>
    run(() => {
      void storeAddTask(title);
      toast.success("Todo added", { description: title });
    });

  return (
    <CommandDialog
      open={paletteOpen}
      onOpenChange={(open) => {
        setPaletteOpen(open);
        if (!open) setQuery("");
      }}
    >
      <Command>
        <CommandInput
          value={query}
          onValueChange={setQuery}
          placeholder="Search screens, repos, sessions, PRs, issues…"
        />
        <CommandList>
          <CommandEmpty>Nothing matches.</CommandEmpty>
          {recentScreens.length > 0 && (
            <>
              <CommandGroup heading="Recent">
                {recentScreens.map((id) => {
                  const screen = SCREENS[id];
                  return (
                    <CommandItem
                      key={id}
                      value={`recent ${screen.title}`}
                      keywords={screen.keywords}
                      onSelect={() => run(() => openTab(id))}
                    >
                      <screen.icon />
                      {screen.title}
                    </CommandItem>
                  );
                })}
              </CommandGroup>
              <CommandSeparator />
            </>
          )}
          <CommandGroup heading="Go to">
            {Object.values(SCREENS).map((screen) => (
              <CommandItem
                key={screen.id}
                keywords={screen.keywords}
                onSelect={() => run(() => openTab(screen.id))}
              >
                <screen.icon />
                {screen.title}
              </CommandItem>
            ))}
          </CommandGroup>
          {repoEntries.length > 0 && (
            <>
              <CommandSeparator />
              <CommandGroup heading="Agentboard repos">
                {repoEntries.map((entry) => (
                  <CommandItem
                    key={entry.key}
                    value={`repo ${entry.repoName} ${entry.folderName} ${entry.folderDir}`}
                    keywords={entry.keywords}
                    onSelect={() => jumpToFolder(entry.folderDir)}
                  >
                    <FolderGit2 />
                    <span className="truncate">{entry.repoName}</span>
                    <span className="ml-1 truncate text-muted-foreground">{entry.folderName}</span>
                    {entry.needs > 0 && (
                      <CommandShortcut className="text-blue-500">
                        {entry.needs} need you
                      </CommandShortcut>
                    )}
                  </CommandItem>
                ))}
              </CommandGroup>
            </>
          )}
          {sessionEntries.length > 0 && (
            <>
              <CommandSeparator />
              <CommandGroup heading="Agentboard sessions">
                {sessionEntries.map((entry) => (
                  <CommandItem
                    key={entry.key}
                    value={`session ${entry.label} ${entry.repoName} ${entry.folderName}`}
                    keywords={entry.keywords}
                    onSelect={() => jumpToSession(entry.folderDir, entry.sessionId)}
                  >
                    <TerminalSquare />
                    <span className="truncate">{entry.label}</span>
                    <span className="ml-1 truncate text-muted-foreground">{entry.repoName}</span>
                    {entry.needs && (
                      <CommandShortcut className="text-blue-500">needs you</CommandShortcut>
                    )}
                  </CommandItem>
                ))}
              </CommandGroup>
            </>
          )}
          {prEntries.length > 0 && (
            <>
              <CommandSeparator />
              <CommandGroup heading="Open pull request">
                {prEntries.map((entry) => (
                  <CommandItem
                    key={entry.key}
                    value={`pr ${entry.repo} ${entry.number} ${entry.title}`}
                    keywords={entry.keywords}
                    onSelect={() => run(() => void openExternalUrl(entry.url))}
                  >
                    <GitPullRequest />
                    <span className="truncate">
                      {entry.repo}
                      <span className="text-muted-foreground"> #{entry.number}</span>
                    </span>
                    <span className="ml-1 truncate text-muted-foreground">{entry.title}</span>
                  </CommandItem>
                ))}
              </CommandGroup>
            </>
          )}
          {issueEntries.length > 0 && (
            <>
              <CommandSeparator />
              <CommandGroup heading="Open issue">
                {issueEntries.map((entry) => (
                  <CommandItem
                    key={entry.key}
                    value={`issue ${entry.repo} ${entry.number} ${entry.title}`}
                    keywords={entry.keywords}
                    onSelect={() => run(() => void openExternalUrl(entry.url))}
                  >
                    <CircleDot />
                    <span className="truncate">
                      {entry.repo}
                      <span className="text-muted-foreground"> #{entry.number}</span>
                    </span>
                    <span className="ml-1 truncate text-muted-foreground">{entry.title}</span>
                  </CommandItem>
                ))}
              </CommandGroup>
            </>
          )}
          <CommandSeparator />
          <CommandGroup heading="Actions">
            <CommandItem
              keywords={["journal", "log", "note", "today"]}
              onSelect={() => run(() => window.dispatchEvent(new Event("quicklog:open")))}
            >
              <PenLine />
              Journal: log a line
              <CommandShortcut>{shortcutHint("quicklog")}</CommandShortcut>
            </CommandItem>
            <CommandItem
              keywords={["theme", "dark", "light"]}
              onSelect={() => run(() => setTheme(resolvedDark ? "light" : "dark"))}
            >
              {resolvedDark ? <Sun /> : <Moon />}
              Switch to {resolvedDark ? "light" : "dark"} theme
            </CommandItem>
            <CommandItem keywords={["sidebar", "panel"]} onSelect={() => run(toggleSidebar)}>
              <PanelLeft />
              Toggle sidebar
              <CommandShortcut>{shortcutHint("sidebar")}</CommandShortcut>
            </CommandItem>
            <CommandItem
              keywords={["settings", "preferences"]}
              onSelect={() => run(() => openSettingsTab())}
            >
              <Settings />
              Open settings
              <CommandShortcut>{shortcutHint("settings")}</CommandShortcut>
            </CommandItem>
          </CommandGroup>
          {quickAdd && (
            <>
              <CommandSeparator />
              <CommandGroup heading="Create">
                <CommandItem
                  key={quickAdd.key}
                  value={`create todo ${quickAdd.title}`}
                  keywords={["todo", "task", "add", "new", quickAdd.title]}
                  onSelect={() => createTodo(quickAdd.title)}
                >
                  <ListPlus />
                  <span className="truncate">
                    Create todo: <span className="text-muted-foreground">{quickAdd.title}</span>
                  </span>
                </CommandItem>
              </CommandGroup>
            </>
          )}
        </CommandList>
      </Command>
    </CommandDialog>
  );
}
