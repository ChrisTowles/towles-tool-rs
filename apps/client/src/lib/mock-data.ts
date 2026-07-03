/**
 * Mock data shaped like the real `ttr` command output so screens can be wired
 * to Tauri commands later without changing their props. Sources: `ttr doctor
 * --json` (tools/ghAuth/plugins), tt-journal listings, tt-graph session
 * accounting, `gh pr list`, and towles-tool.settings.json (tt-config).
 */

export type DoctorTool = {
  name: string;
  version: string | null;
  ok: boolean;
  note?: string;
};

export type DoctorReport = {
  tools: DoctorTool[];
  ghAuth: boolean;
  plugins: { name: string; ok: boolean }[];
};

export const doctorReport: DoctorReport = {
  tools: [
    { name: "git", version: "2.47.1", ok: true },
    { name: "gh", version: "2.63.0", ok: true },
    { name: "node", version: "22.11.0", ok: true },
    { name: "bun", version: "1.1.38", ok: true },
    { name: "cargo", version: "1.83.0", ok: true },
    { name: "claude", version: "2.0.14", ok: true },
    { name: "tmux", version: "3.4", ok: true },
    { name: "ttyd", version: null, ok: false, note: "not installed" },
  ],
  ghAuth: true,
  plugins: [
    { name: "towles-tool", ok: true },
    { name: "code-simplifier", ok: true },
  ],
};

export type JournalEntry = {
  file: string;
  title: string;
  date: string;
  tags: string[];
};

export const dailyNote = {
  file: "journal/2026/07/2026-07-03.md",
  entries: [
    { time: "09:12", text: "Merged the Tailwind v4 + shadcn/ui cutover (PR #2)." },
    { time: "10:40", text: "Documented COSMIC/Wayland screenshot workflow in docs/UI-SCREENSHOTS.md." },
    { time: "13:05", text: "Started the Yaak-style shell for the desktop app." },
  ],
};

export const notes: JournalEntry[] = [
  {
    file: "notes/2026-07-01-vite-hmr-state.md",
    title: "Vite HMR keeps component state",
    date: "2026-07-01",
    tags: ["react", "vite"],
  },
  {
    file: "notes/2026-06-27-tauri-webview-quirks.md",
    title: "Tauri WebKitGTK quirks vs Chrome",
    date: "2026-06-27",
    tags: ["tauri"],
  },
  {
    file: "notes/2026-06-24-ratatui-layout.md",
    title: "Ratatui layout patterns for the agentboard TUI",
    date: "2026-06-24",
    tags: ["rust", "tui"],
  },
  {
    file: "notes/2026-06-19-cargo-workspace-features.md",
    title: "Cargo workspace feature unification gotchas",
    date: "2026-06-19",
    tags: ["rust"],
  },
];

export type Meeting = JournalEntry & { attendees: string[] };

export const meetings: Meeting[] = [
  {
    file: "meetings/2026-07-02-platform-sync.md",
    title: "Platform sync",
    date: "2026-07-02",
    tags: ["work"],
    attendees: ["Chris", "Dana", "Marcus"],
  },
  {
    file: "meetings/2026-06-30-oncall-handoff.md",
    title: "On-call handoff",
    date: "2026-06-30",
    tags: ["work"],
    attendees: ["Chris", "Priya"],
  },
  {
    file: "meetings/2026-06-26-arch-review.md",
    title: "Architecture review: session storage",
    date: "2026-06-26",
    tags: ["work", "design"],
    attendees: ["Chris", "Dana", "Sam", "Lee"],
  },
];

export type SessionUsage = {
  project: string;
  sessions: number;
  inputTokens: number;
  outputTokens: number;
};

export const tokenUsage: SessionUsage[] = [
  { project: "towles-tool-rs-slot-0", sessions: 14, inputTokens: 9_800_000, outputTokens: 412_000 },
  { project: "towles-tool-slot-1", sessions: 6, inputTokens: 4_100_000, outputTokens: 187_000 },
  { project: "dotfiles", sessions: 3, inputTokens: 1_250_000, outputTokens: 61_000 },
  { project: "blog", sessions: 2, inputTokens: 640_000, outputTokens: 38_000 },
  { project: "toolbox", sessions: 1, inputTokens: 210_000, outputTokens: 12_000 },
];

export type PullRequest = {
  number: number;
  title: string;
  branch: string;
  state: "open" | "draft" | "merged";
  checks: "passing" | "failing" | "pending";
  updated: string;
};

export const pullRequests: PullRequest[] = [
  {
    number: 4,
    title: "feat: Yaak-style app shell for the desktop UI",
    branch: "feat/app-shell",
    state: "draft",
    checks: "pending",
    updated: "2026-07-03",
  },
  {
    number: 3,
    title: "feat: agentboard tmux mode follow-ups",
    branch: "feat/agentboard-tmux-2",
    state: "open",
    checks: "passing",
    updated: "2026-07-02",
  },
  {
    number: 2,
    title: "feat: adopt Tailwind v4 + shadcn/ui, remove AgentBoard React UI",
    branch: "feat/agentboard-tmux",
    state: "merged",
    checks: "passing",
    updated: "2026-07-03",
  },
];

/** Mirrors tt-config's UserSettings as serialized to towles-tool.settings.json. */
export const settingsJson = {
  preferredEditor: "code",
  journalSettings: {
    baseFolder: "~/journal",
    dailyPathTemplate: "journal/{yyyy}/{MM}/{yyyy}-{MM}-{dd}.md",
    meetingPathTemplate: "meetings/{yyyy}-{MM}-{dd}-{slug}.md",
    notePathTemplate: "notes/{yyyy}-{MM}-{dd}-{slug}.md",
    templateDir: "~/journal/templates",
  },
  agentboard: {
    mux: "tmux",
    port: 4201,
    sidebarPosition: "left",
  },
};

export const settingsPath = "~/.config/towles-tool/towles-tool.settings.json";
