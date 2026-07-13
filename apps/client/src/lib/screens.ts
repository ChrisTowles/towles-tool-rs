import {
  ChartColumn,
  FileText,
  GitPullRequest,
  Gauge,
  KanbanSquare,
  CalendarDays,
  MessageCircle,
  Radio,
  Settings2,
  Stethoscope,
  TerminalSquare,
  Users,
  type LucideIcon,
} from "lucide-react";

export type ScreenId =
  | "cockpit"
  | "board"
  | "agentboard"
  | "slack"
  | "journal-today"
  | "journal-notes"
  | "journal-meetings"
  | "doctor"
  | "claude-sessions"
  | "gh-prs"
  | "mcp"
  | "config";

export type ScreenMeta = {
  id: ScreenId;
  title: string;
  icon: LucideIcon;
  /** Extra terms the command palette matches on. */
  keywords: string[];
  /** Render without the centered/scrolling content wrapper (e.g. terminals). */
  fullBleed?: boolean;
};

export const SCREENS: Record<ScreenId, ScreenMeta> = {
  cockpit: {
    id: "cockpit",
    title: "Cockpit",
    icon: Gauge,
    keywords: ["home", "day", "next meeting", "prs", "issues", "focus", "zone"],
    fullBleed: true,
  },
  board: {
    id: "board",
    title: "Board",
    icon: KanbanSquare,
    keywords: ["kanban", "todos", "tasks", "issues", "backlog"],
    fullBleed: true,
  },
  agentboard: {
    id: "agentboard",
    title: "Agentboard",
    icon: TerminalSquare,
    keywords: ["agents", "terminal", "sessions", "shell", "folder", "repos", "rail"],
    fullBleed: true,
  },
  slack: {
    id: "slack",
    title: "Messages",
    icon: MessageCircle,
    keywords: ["slack", "dm", "chat", "message", "danielle", "wife"],
    fullBleed: true,
  },
  "journal-today": {
    id: "journal-today",
    title: "Today",
    icon: CalendarDays,
    keywords: ["journal", "daily", "notes"],
  },
  "journal-notes": {
    id: "journal-notes",
    title: "Notes",
    icon: FileText,
    keywords: ["journal", "search"],
  },
  "journal-meetings": {
    id: "journal-meetings",
    title: "Meetings",
    icon: Users,
    keywords: ["journal"],
  },
  doctor: {
    id: "doctor",
    title: "Doctor",
    icon: Stethoscope,
    keywords: ["health", "checks", "tools"],
  },
  "claude-sessions": {
    id: "claude-sessions",
    title: "Claude Sessions",
    icon: ChartColumn,
    keywords: ["tokens", "usage", "sessions", "claude code", "history", "repos"],
  },
  "gh-prs": {
    id: "gh-prs",
    title: "Pull requests",
    icon: GitPullRequest,
    keywords: ["github", "gh", "branches", "review", "checks"],
    fullBleed: true,
  },
  mcp: {
    id: "mcp",
    title: "MCP server",
    icon: Radio,
    keywords: ["mcp", "server", "calls", "tools", "json-rpc", "protocol", "incoming"],
    fullBleed: true,
  },
  config: {
    id: "config",
    title: "Config",
    icon: Settings2,
    keywords: ["settings", "json", "collectors"],
  },
};

export const NAV_SECTIONS: { label: string; screens: ScreenId[] }[] = [
  { label: "Focus", screens: ["cockpit", "board", "agentboard", "slack"] },
  { label: "Journal", screens: ["journal-today", "journal-notes", "journal-meetings"] },
  { label: "Tools", screens: ["doctor", "claude-sessions", "gh-prs", "mcp"] },
  { label: "App", screens: ["config"] },
];
