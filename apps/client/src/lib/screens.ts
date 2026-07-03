import {
  CalendarDays,
  ChartColumn,
  FileText,
  GitPullRequest,
  Settings2,
  Stethoscope,
  Users,
  type LucideIcon,
} from "lucide-react";

export type ScreenId =
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
};

export const SCREENS: Record<ScreenId, ScreenMeta> = {
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
    keywords: ["github", "gh", "branches"],
  },
  config: {
    id: "config",
    title: "Config",
    icon: Settings2,
    keywords: ["settings", "json"],
  },
};

export const NAV_SECTIONS: { label: string; screens: ScreenId[] }[] = [
  { label: "Journal", screens: ["journal-today", "journal-notes", "journal-meetings"] },
  { label: "Tools", screens: ["doctor", "graph", "gh-prs"] },
  { label: "App", screens: ["config"] },
];
