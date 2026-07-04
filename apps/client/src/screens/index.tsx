import type { ComponentType } from "react";
import type { ScreenId } from "@/lib/screens";
import { AgentboardScreen } from "@/screens/agentboard";
import { BoardScreen } from "@/screens/board";
import { CockpitScreen } from "@/screens/cockpit";
import { ConfigScreen } from "@/screens/config";
import { DoctorScreen } from "@/screens/doctor";
import { GhPrsScreen } from "@/screens/gh-prs";
import { GraphScreen } from "@/screens/graph";
import { JournalMeetingsScreen } from "@/screens/journal-meetings";
import { JournalNotesScreen } from "@/screens/journal-notes";
import { JournalTodayScreen } from "@/screens/journal-today";

export const SCREEN_COMPONENTS: Record<ScreenId, ComponentType> = {
  cockpit: CockpitScreen,
  board: BoardScreen,
  agentboard: AgentboardScreen,
  "journal-today": JournalTodayScreen,
  "journal-notes": JournalNotesScreen,
  "journal-meetings": JournalMeetingsScreen,
  doctor: DoctorScreen,
  graph: GraphScreen,
  "gh-prs": GhPrsScreen,
  config: ConfigScreen,
};
