import {
  ChartColumn,
  FileText,
  GitPullRequest,
  Gauge,
  KanbanSquare,
  CalendarDays,
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
  | "journal-today"
  | "journal-notes"
  | "journal-meetings"
  | "doctor"
  | "graph"
  | "gh-prs"
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
  graph: {
    id: "graph",
    title: "Graph",
    icon: ChartColumn,
    keywords: ["tokens", "usage", "sessions"],
  },
  "gh-prs": {
    id: "gh-prs",
    title: "Pull requests",
    icon: GitPullRequest,
    keywords: ["github", "gh", "branches", "review", "checks"],
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
  { label: "Focus", screens: ["cockpit", "board", "agentboard"] },
  { label: "Journal", screens: ["journal-today", "journal-notes", "journal-meetings"] },
  { label: "Tools", screens: ["doctor", "graph", "gh-prs"] },
  { label: "App", screens: ["config"] },
];
